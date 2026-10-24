//! A drawing: the persisted shape of a canvas.
//!
//! Vector-only by design: every stroke is a sequence of quantised
//! [`InkPoint`](crate::brush::InkPoint)s referencing a [`BrushPreset`]
//! in the shared registry. Strokes live inside [`Layer`]s — paint
//! order is bottom-to-top by layer, then insertion order within each
//! layer.
//!
//! The plain fields are the materialised state the renderer reads
//! every frame. Behind them a [`store::DocStore`] keeps the same state
//! as a mergeable history: every mutation goes through [`Doc::emit`],
//! which records the change in the store and mirrors it into the
//! fields, and changes made elsewhere arrive through
//! [`Doc::import_updates`], after which the fields are rebuilt from
//! the store.

mod store;

use std::fmt;

use crate::bookmark::Bookmark;
use crate::brush::{BrushId, BrushPreset, Stroke};
use crate::doc_op::DocOp;
use crate::ids::{BookmarkId, StrokeId};
use crate::ink_glyph::{InkGlyph, InkGlyphId};
use crate::text_box::{TextBox, TextBoxId};

use self::store::DocStore;

/// One editable layer.
///
/// Strokes belong to exactly one layer; [`Layer::opacity`] multiplies
/// every stroke's opacity at paint time, `visible = false` skips the
/// layer entirely, and `locked = true` blocks new strokes / erases
/// from touching it.
#[istmo::message]
#[derive(Clone, PartialEq, Debug)]
pub struct Layer {
    pub id: u32,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    pub strokes: Vec<Stroke>,
    /// Text boxes painted above this layer's strokes, in insertion order.
    pub texts: Vec<TextBox>,
    /// Recognized handwriting redrawn from a font, painted with the strokes.
    pub glyphs: Vec<InkGlyph>,
}

impl Layer {
    #[must_use]
    pub fn new(id: u32, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            visible: true,
            locked: false,
            opacity: 1.0,
            strokes: Vec::new(),
            texts: Vec::new(),
            glyphs: Vec::new(),
        }
    }
}

/// Failure inside the document store.
#[derive(Debug)]
pub enum DocError {
    /// The underlying store rejected the operation.
    Store(String),
    /// A domain payload could not be encoded or decoded.
    Codec(String),
    /// The store holds a container shape this build does not read.
    Shape(&'static str),
}

impl fmt::Display for DocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(msg) => write!(f, "doc store error: {msg}"),
            Self::Codec(msg) => write!(f, "doc codec error: {msg}"),
            Self::Shape(what) => write!(f, "doc store has an unexpected shape at {what}"),
        }
    }
}

impl std::error::Error for DocError {}

/// A drawing. Brush presets are interned into `brushes`; strokes carry
/// only a [`BrushId`] into that table and their own colour override.
#[derive(Debug)]
pub struct Doc {
    pub brushes: Vec<BrushPreset>,
    /// Bottom-to-top paint order. `layers[0]` renders first (below);
    /// `layers[last]` renders last (on top).
    pub layers: Vec<Layer>,
    /// Id of the layer new strokes commit into.
    pub active_layer: u32,
    pub next_layer_id: u32,
    /// Bookmarks pinned to this canvas. Flat list, order is insertion
    /// order (not z-sorted) — the pin overlay sorts by position / age
    /// at paint time. UUID ids mean cross-device merges never collide.
    pub bookmarks: Vec<Bookmark>,
    store: DocStore,
}

impl Default for Doc {
    fn default() -> Self {
        let store = match DocStore::seeded() {
            Ok(store) => store,
            Err(err) => {
                // The seed only touches containers this build created;
                // failure would mean the store itself is broken.
                log::error!("seed doc store: {err}");
                DocStore::unseeded()
            }
        };
        Self {
            brushes: Vec::new(),
            layers: vec![Layer::new(0, "Layer 1")],
            active_layer: 0,
            next_layer_id: 1,
            bookmarks: Vec::new(),
            store,
        }
    }
}

impl Clone for Doc {
    /// Deep copy: the clone owns an independent history.
    fn clone(&self) -> Self {
        Self {
            brushes: self.brushes.clone(),
            layers: self.layers.clone(),
            active_layer: self.active_layer,
            next_layer_id: self.next_layer_id,
            bookmarks: self.bookmarks.clone(),
            store: self.store.fork(),
        }
    }
}

impl PartialEq for Doc {
    /// Compares the materialised state only; two docs with the same
    /// content but different histories are equal.
    fn eq(&self, other: &Self) -> bool {
        self.brushes == other.brushes
            && self.layers == other.layers
            && self.active_layer == other.active_layer
            && self.next_layer_id == other.next_layer_id
            && self.bookmarks == other.bookmarks
    }
}

impl Doc {
    /// Rebuild a doc from bytes produced by [`Self::snapshot_bytes`].
    ///
    /// # Errors
    /// Corrupt bytes or a container shape this build does not read.
    pub fn from_snapshot(bytes: &[u8]) -> Result<Self, DocError> {
        let store = DocStore::from_snapshot(bytes)?;
        let mut doc = Self {
            brushes: Vec::new(),
            layers: Vec::new(),
            active_layer: 0,
            next_layer_id: 0,
            bookmarks: Vec::new(),
            store,
        };
        doc.refresh()?;
        Ok(doc)
    }

    /// Full history + state, ready for [`Self::from_snapshot`].
    ///
    /// # Errors
    /// Store export failure.
    pub fn snapshot_bytes(&self) -> Result<Vec<u8>, DocError> {
        self.store.snapshot()
    }

    /// Opaque marker of everything this doc has seen. Hand it to
    /// another replica's [`Self::updates_since`] to get what is
    /// missing here.
    #[must_use]
    pub fn version(&self) -> Vec<u8> {
        self.store.version()
    }

    /// Changes this doc knows that a replica at `version` does not.
    ///
    /// # Errors
    /// Malformed `version` or store export failure.
    pub fn updates_since(&self, version: &[u8]) -> Result<Vec<u8>, DocError> {
        self.store.updates_since(version)
    }

    /// Merge changes made on another replica and rebuild the
    /// materialised state.
    ///
    /// # Errors
    /// Corrupt bytes or a container shape this build does not read.
    pub fn import_updates(&mut self, bytes: &[u8]) -> Result<(), DocError> {
        self.store.import(bytes)?;
        self.refresh()
    }

    fn refresh(&mut self) -> Result<(), DocError> {
        let state = self.store.materialize()?;
        self.brushes = state.brushes;
        self.layers = state.layers;
        self.active_layer = state.active_layer;
        self.next_layer_id = state.next_layer_id;
        self.bookmarks = state.bookmarks;
        Ok(())
    }

    /// The markdown body, for documents that are text. Empty for a
    /// drawing.
    #[must_use]
    pub fn body(&self) -> String {
        self.store.body()
    }

    /// Replace the markdown body; returns whether it changed. The store
    /// records the smallest edit, so two people typing merge per
    /// character.
    pub fn set_body(&mut self, text: &str) -> bool {
        match self.store.set_body(text) {
            Ok(changed) => changed,
            Err(err) => {
                log::error!("doc body update: {err}");
                false
            }
        }
    }

    /// Record `op` in the store and mirror it into the materialised
    /// state. A store failure is logged and the materialised state
    /// still moves, so the user never loses the edit on screen.
    #[allow(clippy::needless_pass_by_value)] // callers hand the op over; it has landed once this returns
    pub fn emit(&mut self, op: DocOp) {
        if let Err(err) = self.store.apply(&op, self) {
            log::error!("doc store apply: {err}");
        }
        self.apply(&op);
    }

    /// Apply a single op to the materialised state. Idempotent on the
    /// shapes that matter (duplicate `PushStroke`, duplicate
    /// `RegisterBrush`, repeated `SetActiveLayer` on the same id).
    #[allow(clippy::too_many_lines)]
    fn apply(&mut self, op: &DocOp) {
        match op {
            DocOp::RegisterBrush { preset } => {
                if !self.brushes.iter().any(|b| b == preset) {
                    self.brushes.push(*preset);
                }
            }
            DocOp::PushStroke { layer_id, stroke } => {
                if self.find_stroke(stroke.id).is_none()
                    && let Some(layer) = self.layers.iter_mut().find(|l| l.id == *layer_id)
                {
                    layer.strokes.push(stroke.clone());
                }
            }
            DocOp::RemoveStroke { id } => {
                for layer in &mut self.layers {
                    if let Some(pos) = layer.strokes.iter().position(|s| s.id == *id) {
                        layer.strokes.remove(pos);
                        return;
                    }
                }
            }
            DocOp::Clear => {
                self.layers.clear();
                self.layers.push(Layer::new(0, "Layer 1"));
                self.active_layer = 0;
                self.next_layer_id = 1;
            }
            DocOp::AddLayer { id, name } => {
                if !self.layers.iter().any(|l| l.id == *id) {
                    self.layers.push(Layer::new(*id, name.clone()));
                }
                self.next_layer_id = self.next_layer_id.max(id.saturating_add(1));
            }
            DocOp::RemoveLayer { id } => {
                if self.layers.len() <= 1 {
                    return;
                }
                let Some(idx) = self.layers.iter().position(|l| l.id == *id) else {
                    return;
                };
                self.layers.remove(idx);
                if self.active_layer == *id {
                    let fallback = idx.min(self.layers.len().saturating_sub(1));
                    self.active_layer = self.layers[fallback].id;
                }
            }
            DocOp::SetActiveLayer { id } => {
                if self.layers.iter().any(|l| l.id == *id) {
                    self.active_layer = *id;
                }
            }
            DocOp::SetLayerVisible { id, visible } => {
                if let Some(l) = self.layers.iter_mut().find(|l| l.id == *id) {
                    l.visible = *visible;
                }
            }
            DocOp::SetLayerLocked { id, locked } => {
                if let Some(l) = self.layers.iter_mut().find(|l| l.id == *id) {
                    l.locked = *locked;
                }
            }
            DocOp::SetLayerOpacity { id, opacity } => {
                if let Some(l) = self.layers.iter_mut().find(|l| l.id == *id) {
                    l.opacity = opacity.clamp(0.0, 1.0);
                }
            }
            DocOp::AddBookmark { bookmark } => {
                if !self.bookmarks.iter().any(|b| b.id == bookmark.id) {
                    self.bookmarks.push(bookmark.clone());
                }
            }
            DocOp::UpdateBookmark {
                id,
                body,
                refs,
                color,
                updated_at,
            } => {
                if let Some(bm) = self.bookmarks.iter_mut().find(|b| b.id == *id) {
                    bm.body.clone_from(body);
                    bm.refs.clone_from(refs);
                    bm.color = *color;
                    bm.updated_at = *updated_at;
                }
            }
            DocOp::DeleteBookmark { id } => {
                if let Some(pos) = self.bookmarks.iter().position(|b| b.id == *id) {
                    self.bookmarks.remove(pos);
                }
            }
            DocOp::StickBookmark { id, to } => {
                if let Some(bm) = self.bookmarks.iter_mut().find(|b| b.id == *id) {
                    bm.stuck_to = *to;
                }
            }
            DocOp::AddText { layer_id, text } => {
                if self.text(text.id).is_none()
                    && let Some(layer) = self.layers.iter_mut().find(|l| l.id == *layer_id)
                {
                    layer.texts.push(text.clone());
                }
            }
            DocOp::UpdateText {
                id,
                markdown,
                color,
            } => {
                if let Some(text) = self.text_mut(*id) {
                    text.markdown.clone_from(markdown);
                    text.color = *color;
                }
            }
            DocOp::SetTextFrame { id, frame } => {
                if let Some(text) = self.text_mut(*id) {
                    text.frame = *frame;
                }
            }
            DocOp::DeleteText { id } => {
                for layer in &mut self.layers {
                    if let Some(pos) = layer.texts.iter().position(|t| t.id == *id) {
                        layer.texts.remove(pos);
                        return;
                    }
                }
            }
            DocOp::AddGlyph { layer_id, glyph } => {
                if self.locate_glyph(glyph.id).is_none()
                    && let Some(layer) = self.layers.iter_mut().find(|l| l.id == *layer_id)
                {
                    layer.glyphs.push(*glyph);
                }
            }
            DocOp::DeleteGlyph { id } => {
                for layer in &mut self.layers {
                    if let Some(pos) = layer.glyphs.iter().position(|g| g.id == *id) {
                        layer.glyphs.remove(pos);
                        return;
                    }
                }
            }
        }
    }

    /// Produce the op list that undoes `op` when applied in order.
    /// Must be called *before* `op` lands in [`Self::emit`] — LWW
    /// variants read the previous value from the current state.
    ///
    /// Empty vec = non-invertible in isolation. `RegisterBrush` is the
    /// canonical case: `BrushId` is positional and removing an entry
    /// would renumber every later stroke's brush pointer, so palette
    /// registrations are additive forever.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn inverse_before_apply(&self, op: &DocOp) -> Vec<DocOp> {
        match op {
            DocOp::RegisterBrush { .. } => Vec::new(),
            DocOp::PushStroke { stroke, .. } => vec![DocOp::RemoveStroke { id: stroke.id }],
            DocOp::RemoveStroke { id } => self
                .locate_stroke(*id)
                .map(|(layer_id, stroke)| vec![DocOp::PushStroke { layer_id, stroke }])
                .unwrap_or_default(),
            DocOp::Clear => {
                let mut ops: Vec<DocOp> = Vec::new();
                let had_zero = self.layers.iter().any(|l| l.id == 0);
                for layer in &self.layers {
                    // Clear re-seeds a lone `Layer { id: 0, "Layer 1" }`.
                    // Skip adding id==0 when the prior state also had
                    // it — the apply arm is a no-op on duplicate ids
                    // but, more importantly, we must not rely on
                    // AddLayer resetting the name back to the prior
                    // value (no rename op exists yet).
                    if layer.id != 0 {
                        ops.push(DocOp::AddLayer {
                            id: layer.id,
                            name: layer.name.clone(),
                        });
                    }
                    ops.push(DocOp::SetLayerVisible {
                        id: layer.id,
                        visible: layer.visible,
                    });
                    ops.push(DocOp::SetLayerLocked {
                        id: layer.id,
                        locked: layer.locked,
                    });
                    ops.push(DocOp::SetLayerOpacity {
                        id: layer.id,
                        opacity: layer.opacity,
                    });
                    for stroke in &layer.strokes {
                        ops.push(DocOp::PushStroke {
                            layer_id: layer.id,
                            stroke: stroke.clone(),
                        });
                    }
                    for text in &layer.texts {
                        ops.push(DocOp::AddText {
                            layer_id: layer.id,
                            text: text.clone(),
                        });
                    }
                    for glyph in &layer.glyphs {
                        ops.push(DocOp::AddGlyph {
                            layer_id: layer.id,
                            glyph: *glyph,
                        });
                    }
                }
                if !had_zero {
                    // The clear-seeded default layer was not in the
                    // prior state — strip it after rebuilding so the
                    // post-undo layer list matches exactly.
                    ops.push(DocOp::RemoveLayer { id: 0 });
                }
                ops.push(DocOp::SetActiveLayer {
                    id: self.active_layer,
                });
                // `next_layer_id` cannot be fully restored — no op
                // targets it directly. Future AddLayer calls will
                // reuse higher ids than before, which is harmless
                // because layer ids are not user-visible identifiers.
                ops
            }
            DocOp::AddLayer { id, .. } => vec![DocOp::RemoveLayer { id: *id }],
            DocOp::RemoveLayer { id } => {
                let Some(layer) = self.layer(*id) else {
                    return Vec::new();
                };
                let mut ops = vec![
                    DocOp::AddLayer {
                        id: *id,
                        name: layer.name.clone(),
                    },
                    DocOp::SetLayerVisible {
                        id: *id,
                        visible: layer.visible,
                    },
                    DocOp::SetLayerLocked {
                        id: *id,
                        locked: layer.locked,
                    },
                    DocOp::SetLayerOpacity {
                        id: *id,
                        opacity: layer.opacity,
                    },
                ];
                for stroke in &layer.strokes {
                    ops.push(DocOp::PushStroke {
                        layer_id: *id,
                        stroke: stroke.clone(),
                    });
                }
                for text in &layer.texts {
                    ops.push(DocOp::AddText {
                        layer_id: *id,
                        text: text.clone(),
                    });
                }
                for glyph in &layer.glyphs {
                    ops.push(DocOp::AddGlyph {
                        layer_id: *id,
                        glyph: *glyph,
                    });
                }
                if self.active_layer == *id {
                    // RemoveLayer's apply arm moves `active_layer` to
                    // a neighbour. Restoring the removed layer must
                    // also restore the active pointer.
                    ops.push(DocOp::SetActiveLayer { id: *id });
                }
                ops
            }
            DocOp::SetActiveLayer { .. } => vec![DocOp::SetActiveLayer {
                id: self.active_layer,
            }],
            DocOp::SetLayerVisible { id, .. } => self
                .layer(*id)
                .map(|l| {
                    vec![DocOp::SetLayerVisible {
                        id: *id,
                        visible: l.visible,
                    }]
                })
                .unwrap_or_default(),
            DocOp::SetLayerLocked { id, .. } => self
                .layer(*id)
                .map(|l| {
                    vec![DocOp::SetLayerLocked {
                        id: *id,
                        locked: l.locked,
                    }]
                })
                .unwrap_or_default(),
            DocOp::SetLayerOpacity { id, .. } => self
                .layer(*id)
                .map(|l| {
                    vec![DocOp::SetLayerOpacity {
                        id: *id,
                        opacity: l.opacity,
                    }]
                })
                .unwrap_or_default(),
            DocOp::AddBookmark { bookmark } => {
                vec![DocOp::DeleteBookmark { id: bookmark.id }]
            }
            DocOp::DeleteBookmark { id } => self
                .bookmark(*id)
                .map(|bm| {
                    vec![DocOp::AddBookmark {
                        bookmark: bm.clone(),
                    }]
                })
                .unwrap_or_default(),
            DocOp::UpdateBookmark { id, .. } => self
                .bookmark(*id)
                .map(|bm| {
                    vec![DocOp::UpdateBookmark {
                        id: *id,
                        body: bm.body.clone(),
                        refs: bm.refs.clone(),
                        color: bm.color,
                        updated_at: bm.updated_at,
                    }]
                })
                .unwrap_or_default(),
            DocOp::StickBookmark { id, .. } => self
                .bookmark(*id)
                .map(|bm| {
                    vec![DocOp::StickBookmark {
                        id: *id,
                        to: bm.stuck_to,
                    }]
                })
                .unwrap_or_default(),
            DocOp::AddText { text, .. } => vec![DocOp::DeleteText { id: text.id }],
            DocOp::UpdateText { id, .. } => self
                .text(*id)
                .map(|text| {
                    vec![DocOp::UpdateText {
                        id: *id,
                        markdown: text.markdown.clone(),
                        color: text.color,
                    }]
                })
                .unwrap_or_default(),
            DocOp::SetTextFrame { id, .. } => self
                .text(*id)
                .map(|text| {
                    vec![DocOp::SetTextFrame {
                        id: *id,
                        frame: text.frame,
                    }]
                })
                .unwrap_or_default(),
            DocOp::DeleteText { id } => self
                .locate_text(*id)
                .map(|(layer_id, text)| {
                    vec![DocOp::AddText {
                        layer_id,
                        text: text.clone(),
                    }]
                })
                .unwrap_or_default(),
            DocOp::AddGlyph { glyph, .. } => vec![DocOp::DeleteGlyph { id: glyph.id }],
            DocOp::DeleteGlyph { id } => self
                .locate_glyph(*id)
                .map(|(layer_id, glyph)| vec![DocOp::AddGlyph { layer_id, glyph }])
                .unwrap_or_default(),
        }
    }

    /// `(layer_id, glyph)` for the ink glyph carrying `id`.
    #[must_use]
    pub fn locate_glyph(&self, id: InkGlyphId) -> Option<(u32, InkGlyph)> {
        self.layers.iter().find_map(|layer| {
            layer
                .glyphs
                .iter()
                .find(|g| g.id == id)
                .map(|glyph| (layer.id, *glyph))
        })
    }

    /// `(layer_id, text)` for the text box carrying `id`.
    #[must_use]
    pub fn locate_text(&self, id: TextBoxId) -> Option<(u32, &TextBox)> {
        self.layers.iter().find_map(|layer| {
            layer
                .texts
                .iter()
                .find(|t| t.id == id)
                .map(|text| (layer.id, text))
        })
    }

    #[must_use]
    pub fn text(&self, id: TextBoxId) -> Option<&TextBox> {
        self.locate_text(id).map(|(_, text)| text)
    }

    fn text_mut(&mut self, id: TextBoxId) -> Option<&mut TextBox> {
        self.layers
            .iter_mut()
            .find_map(|layer| layer.texts.iter_mut().find(|t| t.id == id))
    }

    /// Immutable lookup of a bookmark by id.
    #[must_use]
    pub fn bookmark(&self, id: BookmarkId) -> Option<&Bookmark> {
        self.bookmarks.iter().find(|b| b.id == id)
    }

    /// Bookmarks pinned to this doc, flat list in insertion order.
    #[must_use]
    pub fn bookmarks(&self) -> &[Bookmark] {
        &self.bookmarks
    }

    /// `(layer_id, cloned_stroke)` for the stroke carrying `id`.
    #[must_use]
    pub fn locate_stroke(&self, id: StrokeId) -> Option<(u32, Stroke)> {
        for layer in &self.layers {
            if let Some(stroke) = layer.strokes.iter().find(|s| s.id == id) {
                return Some((layer.id, stroke.clone()));
            }
        }
        None
    }

    /// Insert `preset` if not already registered and return its
    /// [`BrushId`]. Deduping by structural equality keeps identical
    /// palette entries from bloating the registry when a doc is loaded
    /// and edited across sessions.
    /// `None` once the registry holds `u16::MAX` presets, which no real
    /// document reaches.
    pub fn register_brush(&mut self, preset: BrushPreset) -> Option<BrushId> {
        if let Some(idx) = self.brushes.iter().position(|b| *b == preset) {
            return u16::try_from(idx).ok().map(BrushId);
        }
        let id = BrushId(u16::try_from(self.brushes.len()).ok()?);
        self.emit(DocOp::RegisterBrush { preset });
        Some(id)
    }

    #[must_use]
    pub fn preset(&self, id: BrushId) -> Option<&BrushPreset> {
        self.brushes.get(id.0 as usize)
    }

    /// Append a fully-formed stroke into the active layer, assigning
    /// it a fresh UUID. Returns the assigned id so callers can update
    /// the spatial index.
    /// # Panics
    /// If `active_layer` doesn't refer to an existing layer — invariant
    /// upheld by [`Self::set_active_layer`] and [`Self::remove_layer`].
    pub fn push_stroke(
        &mut self,
        brush: BrushId,
        color: [u8; 4],
        points: Vec<crate::brush::InkPoint>,
    ) -> StrokeId {
        let id = StrokeId::new_v4();
        let layer_id = self.active_layer;
        assert!(
            self.layer(layer_id).is_some(),
            "active_layer must refer to an existing layer"
        );
        let stroke = Stroke {
            id,
            brush,
            color,
            cap_start: crate::brush::CapStyle::Round,
            cap_end: crate::brush::CapStyle::Round,
            points,
        };
        self.emit(DocOp::PushStroke { layer_id, stroke });
        id
    }

    /// Re-insert a pre-built stroke into `layer_id`, respecting its
    /// `id`. Returns `false` if the layer is missing.
    pub fn insert_stroke_into(&mut self, layer_id: u32, stroke: Stroke) -> bool {
        if self.layer(layer_id).is_none() {
            return false;
        }
        self.emit(DocOp::PushStroke { layer_id, stroke });
        true
    }

    /// Mint a fresh stroke id without pushing anything.
    #[must_use]
    #[allow(clippy::unused_self)]
    pub fn allocate_stroke_id(&self) -> StrokeId {
        StrokeId::new_v4()
    }

    #[must_use]
    pub fn find_stroke(&self, id: StrokeId) -> Option<&Stroke> {
        self.layers
            .iter()
            .find_map(|layer| layer.strokes.iter().find(|s| s.id == id))
    }

    /// Remove the stroke with `id` from whichever layer holds it.
    /// Preserves within-layer ordering.
    pub fn remove_stroke(&mut self, id: StrokeId) -> Option<Stroke> {
        let snapshot = self.find_stroke(id)?.clone();
        self.emit(DocOp::RemoveStroke { id });
        Some(snapshot)
    }

    /// Reset to a single empty layer.
    pub fn clear(&mut self) {
        self.emit(DocOp::Clear);
    }

    /// Add a new empty layer at the top of the paint order (topmost)
    /// and return its id.
    pub fn add_layer(&mut self, name: impl Into<String>) -> u32 {
        let id = self.next_layer_id;
        self.emit(DocOp::AddLayer {
            id,
            name: name.into(),
        });
        id
    }

    /// Remove the layer with `id`, unless it is the only layer.
    /// Reassigns `active_layer` to a neighbour if the removed layer
    /// was active. Returns the removed layer (with its strokes) so
    /// the caller can purge derived state.
    pub fn remove_layer(&mut self, id: u32) -> Option<Layer> {
        if self.layers.len() <= 1 {
            return None;
        }
        let snapshot = self.layer(id)?.clone();
        self.emit(DocOp::RemoveLayer { id });
        Some(snapshot)
    }

    #[must_use]
    pub fn layer(&self, id: u32) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    pub fn layer_mut(&mut self, id: u32) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    /// Set the active layer if `id` refers to an existing layer.
    /// Returns `true` on success.
    pub fn set_active_layer(&mut self, id: u32) -> bool {
        if self.layer(id).is_none() {
            return false;
        }
        self.emit(DocOp::SetActiveLayer { id });
        true
    }

    /// Toggle layer visibility. Returns `false` if `id` is unknown.
    pub fn set_layer_visible(&mut self, id: u32, visible: bool) -> bool {
        if self.layer(id).is_none() {
            return false;
        }
        self.emit(DocOp::SetLayerVisible { id, visible });
        true
    }

    /// Toggle layer edit lock. Returns `false` if `id` is unknown.
    pub fn set_layer_locked(&mut self, id: u32, locked: bool) -> bool {
        if self.layer(id).is_none() {
            return false;
        }
        self.emit(DocOp::SetLayerLocked { id, locked });
        true
    }

    /// Set layer paint-time opacity (clamped `[0.0, 1.0]`). Returns
    /// `false` if `id` is unknown.
    pub fn set_layer_opacity(&mut self, id: u32, opacity: f32) -> bool {
        if self.layer(id).is_none() {
            return false;
        }
        self.emit(DocOp::SetLayerOpacity { id, opacity });
        true
    }

    /// Flat iterator over every committed stroke across all layers in
    /// paint order (bottom layer first, then insertion order within).
    pub fn strokes(&self) -> impl Iterator<Item = &Stroke> {
        self.layers.iter().flat_map(|l| l.strokes.iter())
    }

    /// Count of committed strokes across every layer.
    #[must_use]
    pub fn stroke_count(&self) -> usize {
        self.layers.iter().map(|l| l.strokes.len()).sum()
    }

    /// Nothing written on it: no strokes, text, glyphs or bookmarks.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.bookmarks.is_empty()
            && self
                .layers
                .iter()
                .all(|l| l.strokes.is_empty() && l.texts.is_empty() && l.glyphs.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bookmark::{SourceRef, StrokeAnchor, WorldPoint};
    use crate::brush::InkPoint;

    fn point(x: f32, y: f32) -> InkPoint {
        InkPoint::new(x, y, 0, 0, 0)
    }

    #[test]
    fn snapshot_round_trips_every_kind_of_content() {
        let mut d = Doc::default();
        let bid = d.register_brush(BrushPreset::pen()).expect("brush slot");
        let sid = d.push_stroke(bid, [1, 2, 3, 4], vec![point(0.0, 0.0)]);
        let l2 = d.add_layer("L2");
        d.set_active_layer(l2);
        d.set_layer_visible(l2, false);
        d.set_layer_locked(l2, true);
        d.set_layer_opacity(l2, 0.5);
        d.push_stroke(bid, [9; 4], vec![point(1.0, 2.0)]);
        let bm = BookmarkId::new_v4();
        d.emit(DocOp::AddBookmark {
            bookmark: Bookmark::new(bm, WorldPoint::new(3.0, 4.0), 10),
        });
        d.emit(DocOp::AddText {
            layer_id: 0,
            text: TextBox {
                id: TextBoxId::new_v4(),
                frame: crate::text_box::TextFrame::at(1.0, 1.0),
                markdown: "hola".into(),
                color: [0, 0, 0, 255],
            },
        });

        let bytes = d.snapshot_bytes().unwrap();
        let back = Doc::from_snapshot(&bytes).unwrap();
        assert_eq!(back, d);
        assert!(back.find_stroke(sid).is_some());
        assert_eq!(back.active_layer, l2);
        assert_eq!(back.next_layer_id, d.next_layer_id);
        assert_eq!(back.bookmark(bm).unwrap().anchor, WorldPoint::new(3.0, 4.0));
    }

    #[test]
    fn concurrent_edits_converge_in_either_order() {
        let mut a = Doc::default();
        let base = a.snapshot_bytes().unwrap();
        let mut b = Doc::from_snapshot(&base).unwrap();
        let va = a.version();
        let vb = b.version();

        let brush_a = a.register_brush(BrushPreset::pen()).expect("brush slot");
        let sa = a.push_stroke(brush_a, [1; 4], vec![point(0.0, 0.0)]);
        let brush_b = b.register_brush(BrushPreset::marker()).expect("brush slot");
        let sb = b.push_stroke(brush_b, [2; 4], vec![point(5.0, 5.0)]);
        let lb = b.add_layer("from b");

        let from_a = a.updates_since(&vb).unwrap();
        let from_b = b.updates_since(&va).unwrap();
        a.import_updates(&from_b).unwrap();
        b.import_updates(&from_a).unwrap();

        assert_eq!(a, b);
        assert!(a.find_stroke(sa).is_some());
        assert!(a.find_stroke(sb).is_some());
        assert!(a.layer(lb).is_some());
        assert_eq!(a.brushes.len(), 2);
    }

    #[test]
    fn remote_removal_drops_the_stroke_here() {
        let mut a = Doc::default();
        let bid = a.register_brush(BrushPreset::pen()).expect("brush slot");
        let sid = a.push_stroke(bid, [1; 4], vec![point(0.0, 0.0)]);
        let mut b = Doc::from_snapshot(&a.snapshot_bytes().unwrap()).unwrap();
        let before = a.version();
        b.remove_stroke(sid);
        a.import_updates(&b.updates_since(&before).unwrap())
            .unwrap();
        assert!(a.find_stroke(sid).is_none());
        assert_eq!(a, b);
    }

    #[test]
    fn clone_is_an_independent_replica() {
        let mut a = Doc::default();
        let bid = a.register_brush(BrushPreset::pen()).expect("brush slot");
        let mut c = a.clone();
        let only_in_clone = c.push_stroke(bid, [1; 4], vec![point(0.0, 0.0)]);
        assert!(a.find_stroke(only_in_clone).is_none());
        let before = a.version();
        a.import_updates(&c.updates_since(&before).unwrap())
            .unwrap();
        assert!(a.find_stroke(only_in_clone).is_some());
    }

    #[test]
    fn register_dedupes() {
        let mut d = Doc::default();
        let a = d.register_brush(BrushPreset::pen()).expect("brush slot");
        let b = d.register_brush(BrushPreset::pen()).expect("brush slot");
        assert_eq!(a, b);
        assert_eq!(d.brushes.len(), 1);
    }

    #[test]
    fn strokes_iterator_flattens_layers() {
        let mut d = Doc::default();
        let bid = d.register_brush(BrushPreset::pen()).expect("brush slot");
        d.push_stroke(bid, [0; 4], vec![point(0.0, 0.0)]);
        let l2 = d.add_layer("Layer 2");
        d.set_active_layer(l2);
        d.push_stroke(bid, [0; 4], vec![point(1.0, 1.0)]);
        assert_eq!(d.strokes().count(), 2);
        assert_eq!(d.stroke_count(), 2);
    }

    #[test]
    fn add_and_switch_layer() {
        let mut d = Doc::default();
        let l2 = d.add_layer("Layer 2");
        assert!(d.set_active_layer(l2));
        assert_eq!(d.active_layer, l2);
        let bid = d.register_brush(BrushPreset::pen()).expect("brush slot");
        let sid = d.push_stroke(bid, [0, 0, 0, 255], vec![point(0.0, 0.0)]);
        assert!(d.layer(l2).unwrap().strokes.iter().any(|s| s.id == sid));
        assert!(d.layer(0).unwrap().strokes.is_empty());
    }

    #[test]
    fn remove_layer_keeps_at_least_one() {
        let mut d = Doc::default();
        assert!(d.remove_layer(0).is_none(), "cannot remove sole layer");
        let l2 = d.add_layer("Layer 2");
        assert!(d.remove_layer(l2).is_some());
        assert_eq!(d.layers.len(), 1);
        let back = Doc::from_snapshot(&d.snapshot_bytes().unwrap()).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn clear_resets_store_and_state_alike() {
        let mut d = Doc::default();
        let bid = d.register_brush(BrushPreset::pen()).expect("brush slot");
        d.push_stroke(bid, [0; 4], vec![point(0.0, 0.0)]);
        d.add_layer("Layer 2");
        d.clear();
        assert_eq!(d.layers.len(), 1);
        assert_eq!(d.stroke_count(), 0);
        let back = Doc::from_snapshot(&d.snapshot_bytes().unwrap()).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn bookmark_add_update_delete_roundtrip() {
        let mut d = Doc::default();
        let bid = BookmarkId::new_v4();
        d.emit(DocOp::AddBookmark {
            bookmark: Bookmark::new(bid, WorldPoint::new(10.0, 20.0), 1000),
        });
        assert_eq!(d.bookmarks().len(), 1);
        assert_eq!(d.bookmark(bid).unwrap().anchor, WorldPoint::new(10.0, 20.0));

        d.emit(DocOp::UpdateBookmark {
            id: bid,
            body: "hola".into(),
            refs: vec![SourceRef::External {
                url: "https://e.x".into(),
            }],
            color: Some([255, 0, 0, 255]),
            updated_at: 2000,
        });
        let after = d.bookmark(bid).unwrap();
        assert_eq!(after.body, "hola");
        assert_eq!(after.refs.len(), 1);
        assert_eq!(after.updated_at, 2000);
        let back = Doc::from_snapshot(&d.snapshot_bytes().unwrap()).unwrap();
        assert_eq!(back.bookmark(bid).unwrap().body, "hola");

        d.emit(DocOp::DeleteBookmark { id: bid });
        assert!(d.bookmark(bid).is_none());
        let back = Doc::from_snapshot(&d.snapshot_bytes().unwrap()).unwrap();
        assert!(back.bookmark(bid).is_none());
    }

    #[test]
    fn bookmark_add_delete_inverses_roundtrip() {
        let mut d = Doc::default();
        let bid = BookmarkId::new_v4();
        let add = DocOp::AddBookmark {
            bookmark: Bookmark::new(bid, WorldPoint::new(1.0, 2.0), 0),
        };
        let inv_add = d.inverse_before_apply(&add);
        d.emit(add);
        assert_eq!(inv_add, vec![DocOp::DeleteBookmark { id: bid }]);

        let del = DocOp::DeleteBookmark { id: bid };
        let inv_del = d.inverse_before_apply(&del);
        assert_eq!(
            inv_del,
            vec![DocOp::AddBookmark {
                bookmark: d.bookmark(bid).unwrap().clone()
            }]
        );
        d.emit(del);
        for op in inv_del {
            d.emit(op);
        }
        assert_eq!(d.bookmark(bid).unwrap().id, bid);
    }

    #[test]
    fn stick_bookmark_inverse_restores_previous_target() {
        let mut d = Doc::default();
        let bid = BookmarkId::new_v4();
        d.emit(DocOp::AddBookmark {
            bookmark: Bookmark::new(bid, WorldPoint::new(0.0, 0.0), 0),
        });
        let anchor = StrokeAnchor {
            stroke: StrokeId::new_v4(),
            offset_in_bbox: (3.0, 4.0),
        };
        let stick = DocOp::StickBookmark {
            id: bid,
            to: Some(anchor),
        };
        let inv = d.inverse_before_apply(&stick);
        d.emit(stick);
        assert_eq!(d.bookmark(bid).unwrap().stuck_to, Some(anchor));
        for op in inv {
            d.emit(op);
        }
        assert!(d.bookmark(bid).unwrap().stuck_to.is_none());
    }
    /// Two people typing in different places of the same note both keep
    /// their words after the exchange.
    #[test]
    fn body_edits_from_two_replicas_merge_per_character() {
        let mut a = Doc::default();
        assert!(a.set_body("hola mundo"));
        let mut b = Doc::from_snapshot(&a.snapshot_bytes().unwrap()).unwrap();
        assert_eq!(b.body(), "hola mundo");

        assert!(a.set_body("hola querido mundo"));
        assert!(b.set_body("hola mundo cruel"));
        let from_a = a.updates_since(&b.version()).unwrap();
        let from_b = b.updates_since(&a.version()).unwrap();
        a.import_updates(&from_b).unwrap();
        b.import_updates(&from_a).unwrap();
        assert_eq!(a.body(), "hola querido mundo cruel");
        assert_eq!(b.body(), a.body());
        assert!(!a.set_body(&a.body()), "same text is not an edit");
    }
}
