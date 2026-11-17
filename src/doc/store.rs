//! Storage backend of a [`Doc`]: a Loro document holding the same
//! state the materialised fields expose.
//!
//! Nothing outside this module names a Loro type. Every [`DocOp`] is
//! translated into container edits here, and [`DocStore::materialize`]
//! rebuilds the plain Rust state from the containers after an import.
//!
//! Layout inside the document:
//!
//! ```text
//! meta      Map  { active_layer: i64, next_layer_id: i64 }
//! brushes   List [ Binary(BrushPreset) … ]        // positional, append-only
//! layers    MovableList [ Map {
//!               id: i64, name: String, visible: Bool, locked: Bool, opacity: Double,
//!               strokes: MovableList [ Binary(Stroke) … ],
//!               texts:   MovableList [ Binary(TextBox) … ],
//!               images:  MovableList [ Binary(ImageBox) … ],
//!           } … ]
//! bookmarks Map  { "<uuid>": Binary(Bookmark) … }
//! body      Text                                   // markdown of a note, merged per character
//! ```
//!
//! Domain payloads stay encoded blobs: Loro only needs identity and
//! order to merge them, and the renderer decodes them once at
//! materialisation.

use std::borrow::Cow;
use std::fmt;

use loro::{
    Container, ExportMode, Frontiers, LoroDoc, LoroEncodeError, LoroError, LoroMap,
    LoroMovableList, LoroValue, UndoManager, UpdateOptions, ValueOrContainer, VersionVector,
};

use super::{BodyView, Doc, DocError, Layer};
use crate::bookmark::Bookmark;
use crate::brush::{BrushPreset, Stroke};
use crate::doc_op::DocOp;
use crate::ids::{BookmarkId, StrokeId};
use crate::image_box::ImageBox;
use crate::text_box::TextBox;

// Bounded, so a forged length in what is read back is an error.

const META: &str = "meta";
const BRUSHES: &str = "brushes";
const LAYERS: &str = "layers";
const BOOKMARKS: &str = "bookmarks";
const BODY: &str = "body";
/// Records a plugin keeps in the document, merged per key.
const RECORDS: &str = "records";
/// Origin of commits that are no edit of the body, which body undo leaves
/// out: records and drawing ops.
const NOT_BODY: &str = "not-body";
/// Body edits this close together (ms) undo as one step, like typing
/// grouped in the editor.
const UNDO_MERGE_MS: i64 = 900;
const UNDO_STEPS: usize = 256;

const ACTIVE_LAYER: &str = "active_layer";
const NEXT_LAYER_ID: &str = "next_layer_id";

const LAYER_ID: &str = "id";
const LAYER_NAME: &str = "name";
const LAYER_VISIBLE: &str = "visible";
const LAYER_LOCKED: &str = "locked";
const LAYER_OPACITY: &str = "opacity";
const LAYER_STROKES: &str = "strokes";
const LAYER_TEXTS: &str = "texts";
const LAYER_IMAGES: &str = "images";

/// Plain state read back from the containers.
pub(super) struct State {
    pub brushes: Vec<BrushPreset>,
    pub layers: Vec<Layer>,
    pub active_layer: u32,
    pub next_layer_id: u32,
    pub bookmarks: Vec<Bookmark>,
}

/// Loro document behind a [`Doc`].
pub(super) struct DocStore {
    doc: LoroDoc,
    /// Undo of the body edits made on this device, once a reader asked
    /// for it ([`Self::track_body_undo`]).
    undo: Option<UndoManager>,
}

impl fmt::Debug for DocStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocStore")
            .field("peer", &self.doc.peer_id())
            .field("tracks_undo", &self.undo.is_some())
            .finish()
    }
}

impl From<LoroError> for DocError {
    fn from(err: LoroError) -> Self {
        Self::Store(err.to_string())
    }
}

impl From<LoroEncodeError> for DocError {
    fn from(err: LoroEncodeError) -> Self {
        Self::Store(err.to_string())
    }
}

impl DocStore {
    /// Fresh store holding the state of [`Doc::default`].
    #[sismo::function("doc_store::seeded")]
    pub(super) fn seeded() -> Result<Self, DocError> {
        let store = Self {
            doc: LoroDoc::new(),
            undo: None,
        };
        store.reset_layers()?;
        store.doc.commit();
        Ok(store)
    }

    /// An empty store, without the default layer; only for when
    /// [`Self::seeded`] fails.
    pub(super) fn unseeded() -> Self {
        Self {
            doc: LoroDoc::new(),
            undo: None,
        }
    }

    /// Store decoded from a full snapshot.
    #[sismo::function("doc_store::from_snapshot")]
    pub(super) fn from_snapshot(bytes: &[u8]) -> Result<Self, DocError> {
        let doc = LoroDoc::new();
        doc.import(bytes)?;
        Ok(Self { doc, undo: None })
    }

    /// Independent copy with the same history.
    #[sismo::function("doc_store::fork")]
    pub(super) fn fork(&self) -> Self {
        Self {
            doc: self.doc.fork(),
            undo: None,
        }
    }

    /// Full history + state.
    #[sismo::function("doc_store::snapshot")]
    pub(super) fn snapshot(&self) -> Result<Vec<u8>, DocError> {
        Ok(self.doc.export(ExportMode::Snapshot)?)
    }

    /// Opaque version marker for [`Self::updates_since`].
    #[sismo::function("doc_store::version")]
    pub(super) fn version(&self) -> Vec<u8> {
        self.doc.oplog_vv().encode()
    }

    /// Every change this store knows that `version` does not.
    #[sismo::function("doc_store::updates_since")]
    pub(super) fn updates_since(&self, version: &[u8]) -> Result<Vec<u8>, DocError> {
        let from = VersionVector::decode(version)?;
        Ok(self.doc.export(ExportMode::Updates {
            from: Cow::Owned(from),
        })?)
    }

    /// Merge changes produced elsewhere.
    #[sismo::function("doc_store::import")]
    pub(super) fn import(&self, bytes: &[u8]) -> Result<(), DocError> {
        self.doc.import(bytes)?;
        Ok(())
    }

    /// Commit what is not an edit of the body, so body undo skips it.
    fn commit_not_body(&self) {
        self.doc.set_next_commit_origin(NOT_BODY);
        self.doc.commit();
    }

    /// Keep the body edits made on this device from now on, for
    /// [`Self::undo_body`]. Text that arrives from other devices is
    /// never undone; the steps made here move around it.
    #[sismo::function("doc_store::track_body_undo")]
    pub(super) fn track_body_undo(&mut self) {
        if self.undo.is_some() {
            return;
        }
        let mut undo = UndoManager::new(&self.doc);
        undo.set_merge_interval(UNDO_MERGE_MS);
        undo.set_max_undo_steps(UNDO_STEPS);
        undo.add_exclude_origin_prefix(NOT_BODY);
        self.undo = Some(undo);
    }

    /// Undo the last body edit made here. Returns whether there was one.
    #[sismo::function("doc_store::undo_body")]
    pub(super) fn undo_body(&mut self) -> Result<bool, DocError> {
        match &mut self.undo {
            Some(undo) => Ok(undo.undo()?),
            None => Ok(false),
        }
    }

    /// Make again the last body edit undone here. Returns whether there
    /// was one.
    #[sismo::function("doc_store::redo_body")]
    pub(super) fn redo_body(&mut self) -> Result<bool, DocError> {
        match &mut self.undo {
            Some(undo) => Ok(undo.redo()?),
            None => Ok(false),
        }
    }

    pub(super) fn can_undo_body(&self) -> bool {
        self.undo.as_ref().is_some_and(UndoManager::can_undo)
    }

    pub(super) fn can_redo_body(&self) -> bool {
        self.undo.as_ref().is_some_and(UndoManager::can_redo)
    }

    /// The text body, for documents that are text (notes).
    #[sismo::function("doc_store::body")]
    pub(super) fn body(&self) -> String {
        self.doc.get_text(BODY).to_string()
    }

    /// Replace the text body with `text`, recorded as the smallest edit
    /// that gets there so concurrent typing on other devices merges per
    /// character. Returns whether anything changed.
    #[sismo::function("doc_store::set_body")]
    pub(super) fn set_body(&self, text: &str) -> Result<bool, DocError> {
        let body = self.doc.get_text(BODY);
        if body.to_string() == text {
            return Ok(false);
        }
        body.update(text, UpdateOptions::default())
            .map_err(|err| DocError::Store(err.to_string()))?;
        self.doc.commit();
        Ok(true)
    }

    /// The text body together with the version it was read at.
    #[sismo::function("doc_store::body_view")]
    pub(super) fn body_view(&self) -> BodyView {
        BodyView {
            text: self.body(),
            version: self.doc.state_frontiers().encode(),
        }
    }

    /// Turn the body seen as `base` into `text`, keeping what arrived
    /// from other devices since `base` was read: the edit is recorded
    /// on a fork at `base` and merged back like a concurrent one.
    /// Returns the view the editor now holds, `None` when `text` is no
    /// edit of `base`.
    #[sismo::function("doc_store::edit_body")]
    pub(super) fn edit_body(
        &self,
        base: &BodyView,
        text: &str,
    ) -> Result<Option<BodyView>, DocError> {
        if base.text == text {
            return Ok(None);
        }
        let at = Frontiers::decode(&base.version)?;
        if at == self.doc.state_frontiers() {
            self.set_body(text)?;
            return Ok(Some(self.body_view()));
        }
        sismo::instant!("doc_store::edit_body_merge");
        let fork = self.doc.fork_at(&at)?;
        fork.get_text(BODY)
            .update(text, UpdateOptions::default())
            .map_err(|err| DocError::Store(err.to_string()))?;
        fork.commit();
        let edit = fork.export(ExportMode::Updates {
            from: Cow::Owned(self.doc.oplog_vv()),
        })?;
        self.doc.import(&edit)?;
        Ok(Some(BodyView {
            text: text.to_owned(),
            version: fork.state_frontiers().encode(),
        }))
    }

    /// Every record, by key.
    #[sismo::function("doc_store::records")]
    pub(super) fn records(&self) -> Vec<(String, Vec<u8>)> {
        let mut out = Vec::new();
        self.doc.get_map(RECORDS).for_each(|key, value| {
            if let ValueOrContainer::Value(value) = value
                && let Some(bytes) = value.as_binary()
            {
                out.push((key.to_owned(), bytes.to_vec()));
            }
        });
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Set the record `key`; two devices setting different keys both
    /// keep theirs, the same key keeps one. Returns whether it changed.
    #[sismo::function("doc_store::set_record")]
    pub(super) fn set_record(&self, key: &str, bytes: &[u8]) -> Result<bool, DocError> {
        let map = self.doc.get_map(RECORDS);
        if let Some(ValueOrContainer::Value(value)) = map.get(key)
            && value
                .as_binary()
                .is_some_and(|held| held.as_slice() == bytes)
        {
            return Ok(false);
        }
        map.insert(key, bytes.to_vec())?;
        self.commit_not_body();
        Ok(true)
    }

    /// Remove the record `key`. Returns whether it was there.
    #[sismo::function("doc_store::remove_record")]
    pub(super) fn remove_record(&self, key: &str) -> Result<bool, DocError> {
        let map = self.doc.get_map(RECORDS);
        if map.get(key).is_none() {
            return Ok(false);
        }
        map.delete(key)?;
        self.commit_not_body();
        Ok(true)
    }

    /// Translate `op` into container edits. `state` is the
    /// materialised state *before* the op so positional lookups match
    /// the containers.
    #[sismo::function("doc_store::apply")]
    pub(super) fn apply(&self, op: &DocOp, state: &Doc) -> Result<(), DocError> {
        self.apply_inner(op, state)?;
        self.commit_not_body();
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn apply_inner(&self, op: &DocOp, state: &Doc) -> Result<(), DocError> {
        match op {
            DocOp::RegisterBrush { preset } => {
                if !state.brushes.iter().any(|b| b == preset) {
                    self.doc.get_list(BRUSHES).push(encode(preset)?)?;
                }
            }
            DocOp::PushStroke { layer_id, stroke } => {
                if state.find_stroke(stroke.id).is_some() {
                    return Ok(());
                }
                if let Some(layer) = self.layer_by_id(state, *layer_id) {
                    child_list(&layer, LAYER_STROKES)?.push(encode(stroke)?)?;
                }
            }
            DocOp::RemoveStroke { id } => {
                let found = state.layers.iter().enumerate().find_map(|(pos, l)| {
                    l.strokes
                        .iter()
                        .position(|s| s.id == *id)
                        .map(|idx| (pos, idx))
                });
                if let Some((pos, idx)) = found {
                    let layer = self.layer_at(pos)?;
                    child_list(&layer, LAYER_STROKES)?.delete(idx, 1)?;
                }
            }
            DocOp::Clear => self.reset_layers()?,
            DocOp::AddLayer { id, name } => {
                if state.layer(*id).is_none() {
                    self.push_layer(*id, name)?;
                }
                let next = state.next_layer_id.max(id.saturating_add(1));
                self.meta().insert(NEXT_LAYER_ID, i64::from(next))?;
            }
            DocOp::RemoveLayer { id } => {
                if state.layers.len() <= 1 {
                    return Ok(());
                }
                let Some(pos) = state.layers.iter().position(|l| l.id == *id) else {
                    return Ok(());
                };
                self.doc.get_movable_list(LAYERS).delete(pos, 1)?;
                if state.active_layer == *id {
                    let remaining: Vec<&Layer> =
                        state.layers.iter().filter(|l| l.id != *id).collect();
                    let fallback = remaining[pos.min(remaining.len().saturating_sub(1))].id;
                    self.meta().insert(ACTIVE_LAYER, i64::from(fallback))?;
                }
            }
            DocOp::SetActiveLayer { id } => {
                if state.layer(*id).is_some() {
                    self.meta().insert(ACTIVE_LAYER, i64::from(*id))?;
                }
            }
            DocOp::SetLayerVisible { id, visible } => {
                if let Some(layer) = self.layer_by_id(state, *id) {
                    layer.insert(LAYER_VISIBLE, *visible)?;
                }
            }
            DocOp::SetLayerLocked { id, locked } => {
                if let Some(layer) = self.layer_by_id(state, *id) {
                    layer.insert(LAYER_LOCKED, *locked)?;
                }
            }
            DocOp::SetLayerOpacity { id, opacity } => {
                if let Some(layer) = self.layer_by_id(state, *id) {
                    layer.insert(LAYER_OPACITY, f64::from(opacity.clamp(0.0, 1.0)))?;
                }
            }
            DocOp::RenameLayer { id, name } => {
                if let Some(layer) = self.layer_by_id(state, *id) {
                    layer.insert(LAYER_NAME, name.as_str())?;
                }
            }
            DocOp::MoveLayer { id, to } => {
                if let Some((from, to)) = state.layer_move(*id, *to) {
                    self.doc.get_movable_list(LAYERS).mov(from, to)?;
                }
            }
            DocOp::AddBookmark { bookmark } => {
                if state.bookmark(bookmark.id).is_none() {
                    self.put_bookmark(bookmark)?;
                }
            }
            DocOp::UpdateBookmark {
                id,
                body,
                refs,
                color,
                updated_at,
            } => {
                if let Some(bm) = state.bookmark(*id) {
                    let mut next = bm.clone();
                    next.body.clone_from(body);
                    next.refs.clone_from(refs);
                    next.color = *color;
                    next.updated_at = *updated_at;
                    self.put_bookmark(&next)?;
                }
            }
            DocOp::DeleteBookmark { id } => {
                if state.bookmark(*id).is_some() {
                    self.doc.get_map(BOOKMARKS).delete(&bookmark_key(*id))?;
                }
            }
            DocOp::StickBookmark { id, to } => {
                if let Some(bm) = state.bookmark(*id) {
                    let mut next = bm.clone();
                    next.stuck_to = *to;
                    self.put_bookmark(&next)?;
                }
            }
            DocOp::AddText { layer_id, text } => {
                if state.text(text.id).is_some() {
                    return Ok(());
                }
                if let Some(layer) = self.layer_by_id(state, *layer_id) {
                    child_list(&layer, LAYER_TEXTS)?.push(encode(text)?)?;
                }
            }
            DocOp::UpdateText {
                id,
                markdown,
                color,
            } => {
                if let Some((pos, idx, text)) = locate_text(state, *id) {
                    let mut next = text.clone();
                    next.markdown.clone_from(markdown);
                    next.color = *color;
                    let layer = self.layer_at(pos)?;
                    child_list(&layer, LAYER_TEXTS)?.set(idx, encode(&next)?)?;
                }
            }
            DocOp::SetTextFrame { id, frame } => {
                if let Some((pos, idx, text)) = locate_text(state, *id) {
                    let mut next = text.clone();
                    next.frame = *frame;
                    let layer = self.layer_at(pos)?;
                    child_list(&layer, LAYER_TEXTS)?.set(idx, encode(&next)?)?;
                }
            }
            DocOp::DeleteText { id } => {
                if let Some((pos, idx, _)) = locate_text(state, *id) {
                    let layer = self.layer_at(pos)?;
                    child_list(&layer, LAYER_TEXTS)?.delete(idx, 1)?;
                }
            }
            DocOp::AddImage { layer_id, image } => {
                if state.image(image.id).is_some() {
                    return Ok(());
                }
                if let Some(layer) = self.layer_by_id(state, *layer_id) {
                    child_list(&layer, LAYER_IMAGES)?.push(encode(image)?)?;
                }
            }
            DocOp::SetImageFrame { id, frame, height } => {
                if let Some((pos, idx, image)) = locate_image(state, *id) {
                    let mut next = image.clone();
                    next.frame = *frame;
                    next.height = *height;
                    let layer = self.layer_at(pos)?;
                    child_list(&layer, LAYER_IMAGES)?.set(idx, encode(&next)?)?;
                }
            }
            DocOp::DeleteImage { id } => {
                if let Some((pos, idx, _)) = locate_image(state, *id) {
                    let layer = self.layer_at(pos)?;
                    child_list(&layer, LAYER_IMAGES)?.delete(idx, 1)?;
                }
            }
            DocOp::SetStrokeFix { id, fix } => {
                if let Some((pos, idx, stroke)) = locate_stroke(state, *id) {
                    let next = Stroke {
                        fix: *fix,
                        ..stroke.clone()
                    };
                    let layer = self.layer_at(pos)?;
                    child_list(&layer, LAYER_STROKES)?.set(idx, encode(&next)?)?;
                }
            }
            DocOp::SetStrokeShape { id, shape } => {
                if let Some((pos, idx, stroke)) = locate_stroke(state, *id) {
                    let next = Stroke {
                        shape: shape.clone(),
                        ..stroke.clone()
                    };
                    let layer = self.layer_at(pos)?;
                    child_list(&layer, LAYER_STROKES)?.set(idx, encode(&next)?)?;
                }
            }
            DocOp::MoveStroke { id, dx, dy } => {
                if let Some((pos, idx, stroke)) = locate_stroke(state, *id) {
                    let next = stroke.clone().translated(*dx, *dy);
                    let layer = self.layer_at(pos)?;
                    child_list(&layer, LAYER_STROKES)?.set(idx, encode(&next)?)?;
                }
            }
        }
        Ok(())
    }

    /// Read the plain state back from the containers.
    #[sismo::function("doc_store::materialize")]
    pub(super) fn materialize(&self) -> Result<State, DocError> {
        let meta = self.meta();
        let active_layer = read_u32(&meta, ACTIVE_LAYER)?;
        let next_layer_id = read_u32(&meta, NEXT_LAYER_ID)?;

        let brushes = self
            .doc
            .get_list(BRUSHES)
            .to_vec()
            .iter()
            .map(decode::<BrushPreset>)
            .collect::<Result<Vec<_>, _>>()?;

        let list = self.doc.get_movable_list(LAYERS);
        let mut layers = Vec::with_capacity(list.len());
        for pos in 0..list.len() {
            let map = self.layer_at(pos)?;
            let mut layer = Layer::new(read_u32(&map, LAYER_ID)?, read_string(&map, LAYER_NAME)?);
            layer.visible = read_bool(&map, LAYER_VISIBLE)?;
            layer.locked = read_bool(&map, LAYER_LOCKED)?;
            layer.opacity = read_f32(&map, LAYER_OPACITY)?;
            layer.strokes = read_blobs::<Stroke>(&map, LAYER_STROKES)?;
            layer.texts = read_blobs::<TextBox>(&map, LAYER_TEXTS)?;
            layer.images = read_blobs::<ImageBox>(&map, LAYER_IMAGES)?;
            layers.push(layer);
        }

        let mut bookmarks = self
            .doc
            .get_map(BOOKMARKS)
            .values()
            .map(|v| match v {
                ValueOrContainer::Value(value) => decode::<Bookmark>(&value),
                ValueOrContainer::Container(_) => Err(DocError::Shape("bookmark container")),
            })
            .collect::<Result<Vec<_>, _>>()?;
        bookmarks.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.0.cmp(&b.id.0)));

        Ok(State {
            brushes,
            layers,
            active_layer,
            next_layer_id,
            bookmarks,
        })
    }

    fn meta(&self) -> LoroMap {
        self.doc.get_map(META)
    }

    /// Drop every layer and seed the single default one.
    #[sismo::function("doc_store::reset_layers")]
    fn reset_layers(&self) -> Result<(), DocError> {
        self.doc.get_movable_list(LAYERS).clear()?;
        self.push_layer(0, "Layer 1")?;
        let meta = self.meta();
        meta.insert(ACTIVE_LAYER, 0_i64)?;
        meta.insert(NEXT_LAYER_ID, 1_i64)?;
        Ok(())
    }

    fn push_layer(&self, id: u32, name: &str) -> Result<(), DocError> {
        let map = self
            .doc
            .get_movable_list(LAYERS)
            .push_container(LoroMap::new())?;
        map.insert(LAYER_ID, i64::from(id))?;
        map.insert(LAYER_NAME, name)?;
        map.insert(LAYER_VISIBLE, true)?;
        map.insert(LAYER_LOCKED, false)?;
        map.insert(LAYER_OPACITY, 1.0_f64)?;
        map.insert_container(LAYER_STROKES, LoroMovableList::new())?;
        map.insert_container(LAYER_TEXTS, LoroMovableList::new())?;
        map.insert_container(LAYER_IMAGES, LoroMovableList::new())?;
        Ok(())
    }

    fn layer_at(&self, pos: usize) -> Result<LoroMap, DocError> {
        match self.doc.get_movable_list(LAYERS).get(pos) {
            Some(ValueOrContainer::Container(Container::Map(map))) => Ok(map),
            _ => Err(DocError::Shape("layer")),
        }
    }

    fn layer_by_id(&self, state: &Doc, id: u32) -> Option<LoroMap> {
        let pos = state.layers.iter().position(|l| l.id == id)?;
        self.layer_at(pos).ok()
    }

    fn put_bookmark(&self, bookmark: &Bookmark) -> Result<(), DocError> {
        self.doc
            .get_map(BOOKMARKS)
            .insert(&bookmark_key(bookmark.id), encode(bookmark)?)?;
        Ok(())
    }
}

fn bookmark_key(id: BookmarkId) -> String {
    uuid::Uuid::from_bytes(id.0).to_string()
}

fn locate_text(state: &Doc, id: crate::text_box::TextBoxId) -> Option<(usize, usize, &TextBox)> {
    state.layers.iter().enumerate().find_map(|(pos, l)| {
        l.texts
            .iter()
            .enumerate()
            .find(|(_, t)| t.id == id)
            .map(|(idx, t)| (pos, idx, t))
    })
}

fn locate_stroke(state: &Doc, id: StrokeId) -> Option<(usize, usize, &Stroke)> {
    state.layers.iter().enumerate().find_map(|(pos, l)| {
        l.strokes
            .iter()
            .enumerate()
            .find(|(_, s)| s.id == id)
            .map(|(idx, s)| (pos, idx, s))
    })
}

fn locate_image(
    state: &Doc,
    id: crate::image_box::ImageBoxId,
) -> Option<(usize, usize, &ImageBox)> {
    state.layers.iter().enumerate().find_map(|(pos, l)| {
        l.images
            .iter()
            .enumerate()
            .find(|(_, i)| i.id == id)
            .map(|(idx, i)| (pos, idx, i))
    })
}

fn child_list(map: &LoroMap, key: &str) -> Result<LoroMovableList, DocError> {
    match map.get(key) {
        Some(ValueOrContainer::Container(Container::MovableList(list))) => Ok(list),
        _ => Err(DocError::Shape("layer child list")),
    }
}

fn encode<T: istmo::codec::Encode>(value: &T) -> Result<Vec<u8>, DocError> {
    notin_plugin::encode(value).map_err(|e| DocError::Codec(e.to_string()))
}

fn decode<T: istmo::codec::Decode>(value: &LoroValue) -> Result<T, DocError> {
    let bytes = value.as_binary().ok_or(DocError::Shape("binary payload"))?;
    notin_plugin::decode_bounded::<T>(bytes, notin_plugin::WIRE_LIMIT)
        .map_err(|e| DocError::Codec(e.to_string()))
}

fn read_blobs<T: istmo::codec::Decode>(map: &LoroMap, key: &str) -> Result<Vec<T>, DocError> {
    child_list(map, key)?
        .to_vec()
        .iter()
        .map(decode::<T>)
        .collect()
}

fn read_value(map: &LoroMap, key: &str) -> Result<LoroValue, DocError> {
    match map.get(key) {
        Some(ValueOrContainer::Value(value)) => Ok(value),
        _ => Err(DocError::Shape("map value")),
    }
}

fn read_u32(map: &LoroMap, key: &str) -> Result<u32, DocError> {
    let raw = *read_value(map, key)?
        .as_i64()
        .ok_or(DocError::Shape("integer"))?;
    u32::try_from(raw).map_err(|_| DocError::Shape("integer range"))
}

fn read_bool(map: &LoroMap, key: &str) -> Result<bool, DocError> {
    read_value(map, key)?
        .as_bool()
        .copied()
        .ok_or(DocError::Shape("bool"))
}

#[allow(clippy::cast_possible_truncation)]
fn read_f32(map: &LoroMap, key: &str) -> Result<f32, DocError> {
    read_value(map, key)?
        .as_double()
        .map(|v| *v as f32)
        .ok_or(DocError::Shape("double"))
}

fn read_string(map: &LoroMap, key: &str) -> Result<String, DocError> {
    read_value(map, key)?
        .as_string()
        .map(|s| s.to_string())
        .ok_or(DocError::Shape("string"))
}
