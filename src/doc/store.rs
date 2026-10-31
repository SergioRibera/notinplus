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
//!               glyphs:  MovableList [ Binary(InkGlyph) … ],
//!           } … ]
//! bookmarks Map  { "<uuid>": Binary(Bookmark) … }
//! body      Text                                   // markdown of a note, merged per character
//! ```
//!
//! Domain payloads stay bincode blobs: Loro only needs identity and
//! order to merge them, and the renderer decodes them once at
//! materialisation.

use std::borrow::Cow;
use std::fmt;

use bincode::config::{self, Configuration};
use loro::{
    Container, ExportMode, LoroDoc, LoroEncodeError, LoroError, LoroMap, LoroMovableList,
    LoroValue, UpdateOptions, ValueOrContainer, VersionVector,
};

use super::{Doc, DocError, Layer};
use crate::bookmark::Bookmark;
use crate::brush::{BrushPreset, Stroke};
use crate::doc_op::DocOp;
use crate::ids::BookmarkId;
use crate::ink_glyph::InkGlyph;
use crate::text_box::TextBox;

const CODEC: Configuration = config::standard();

const META: &str = "meta";
const BRUSHES: &str = "brushes";
const LAYERS: &str = "layers";
const BOOKMARKS: &str = "bookmarks";
const BODY: &str = "body";
/// Records a plugin keeps in the document, merged per key.
const RECORDS: &str = "records";

const ACTIVE_LAYER: &str = "active_layer";
const NEXT_LAYER_ID: &str = "next_layer_id";

const LAYER_ID: &str = "id";
const LAYER_NAME: &str = "name";
const LAYER_VISIBLE: &str = "visible";
const LAYER_LOCKED: &str = "locked";
const LAYER_OPACITY: &str = "opacity";
const LAYER_STROKES: &str = "strokes";
const LAYER_TEXTS: &str = "texts";
const LAYER_GLYPHS: &str = "glyphs";

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
}

impl fmt::Debug for DocStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DocStore")
            .field("peer", &self.doc.peer_id())
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
        }
    }

    /// Store decoded from a full snapshot.
    #[sismo::function("doc_store::from_snapshot")]
    pub(super) fn from_snapshot(bytes: &[u8]) -> Result<Self, DocError> {
        let doc = LoroDoc::new();
        doc.import(bytes)?;
        Ok(Self { doc })
    }

    /// Independent copy with the same history.
    #[sismo::function("doc_store::fork")]
    pub(super) fn fork(&self) -> Self {
        Self {
            doc: self.doc.fork(),
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
        self.doc.commit();
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
        self.doc.commit();
        Ok(true)
    }

    /// Translate `op` into container edits. `state` is the
    /// materialised state *before* the op so positional lookups match
    /// the containers.
    #[sismo::function("doc_store::apply")]
    pub(super) fn apply(&self, op: &DocOp, state: &Doc) -> Result<(), DocError> {
        self.apply_inner(op, state)?;
        self.doc.commit();
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
            DocOp::AddGlyph { layer_id, glyph } => {
                if state.locate_glyph(glyph.id).is_some() {
                    return Ok(());
                }
                if let Some(layer) = self.layer_by_id(state, *layer_id) {
                    child_list(&layer, LAYER_GLYPHS)?.push(encode(glyph)?)?;
                }
            }
            DocOp::DeleteGlyph { id } => {
                let found = state.layers.iter().enumerate().find_map(|(pos, l)| {
                    l.glyphs
                        .iter()
                        .position(|g| g.id == *id)
                        .map(|idx| (pos, idx))
                });
                if let Some((pos, idx)) = found {
                    let layer = self.layer_at(pos)?;
                    child_list(&layer, LAYER_GLYPHS)?.delete(idx, 1)?;
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
            layer.glyphs = read_blobs::<InkGlyph>(&map, LAYER_GLYPHS)?;
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
        map.insert_container(LAYER_GLYPHS, LoroMovableList::new())?;
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

fn child_list(map: &LoroMap, key: &str) -> Result<LoroMovableList, DocError> {
    match map.get(key) {
        Some(ValueOrContainer::Container(Container::MovableList(list))) => Ok(list),
        _ => Err(DocError::Shape("layer child list")),
    }
}

fn encode<T: bincode::Encode>(value: &T) -> Result<Vec<u8>, DocError> {
    bincode::encode_to_vec(value, CODEC).map_err(|e| DocError::Codec(e.to_string()))
}

fn decode<T: bincode::Decode<()>>(value: &LoroValue) -> Result<T, DocError> {
    let bytes = value.as_binary().ok_or(DocError::Shape("binary payload"))?;
    bincode::decode_from_slice::<T, _>(bytes, CODEC)
        .map(|(v, _)| v)
        .map_err(|e| DocError::Codec(e.to_string()))
}

fn read_blobs<T: bincode::Decode<()>>(map: &LoroMap, key: &str) -> Result<Vec<T>, DocError> {
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
