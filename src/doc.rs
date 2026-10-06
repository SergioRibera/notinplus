//! `.notinplus` document — the persisted shape of a drawing.
//!
//! Vector-only by design: every stroke is a sequence of quantised
//! [`InkPoint`]s referencing a [`BrushPreset`] in the shared registry.
//! Strokes live inside [`Layer`]s — paint order is bottom-to-top by
//! layer, then insertion order within each layer. Bincode 2 is the
//! wire codec (`istmo::message` prepends the derives); no serde in the
//! tree.

use std::io;
use std::path::Path;

use bincode::config::{self, Configuration};

use crate::brush::{BrushId, BrushPreset, Stroke};
use crate::doc_op::{DocOp, OpRecord};
use crate::identity::global_actor;
use crate::ids::StrokeId;
use crate::op::OpId;

const CODEC: Configuration = config::standard();

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
        }
    }
}

/// A drawing. Brush presets are interned into `brushes`; strokes carry
/// only a [`BrushId`] into that table and their own colour override.
#[istmo::message]
#[derive(Clone, PartialEq, Debug)]
pub struct Doc {
    pub brushes: Vec<BrushPreset>,
    /// Bottom-to-top paint order. `layers[0]` renders first (below);
    /// `layers[last]` renders last (on top).
    pub layers: Vec<Layer>,
    /// Id of the layer new strokes commit into.
    pub active_layer: u32,
    pub next_layer_id: u32,
    /// Highest Lamport observed on this doc. Local emission in
    /// Phase 1.2 bumps this before stamping an `OpId`; Phase 10
    /// `observe_remote` fast-forwards it past an incoming op.
    pub lamport: u64,
    /// Append-only CRDT op record log. Empty until Phase 1.2 wires
    /// mutations to emit; shape is fixed now so persisted docs don't
    /// need another breaking migration mid-phase.
    pub log: Vec<OpRecord>,
}

impl Default for Doc {
    fn default() -> Self {
        Self {
            brushes: Vec::new(),
            layers: vec![Layer::new(0, "Layer 1")],
            active_layer: 0,
            next_layer_id: 1,
            lamport: 0,
            log: Vec::new(),
        }
    }
}

/// Compact the log into the materialised snapshot once it crosses
/// this threshold. Peers that are more than `SNAPSHOT_EVERY` ops
/// behind cannot catch up from a diff after a compaction — Phase 10
/// will formalise a sync-aware policy that only trims up to the
/// slowest known peer's `OpId`.
const SNAPSHOT_EVERY: usize = 512;

impl Doc {
    /// Stamp an `OpId`, mutate the materialised state via [`apply`],
    /// and append the record to the log. Returns the stamped `OpId`
    /// so callers that need to tag follow-up work (undo-stack entries
    /// in Phase 1.3, bookmark backrefs in Phase 1.5) can keep the
    /// identifier without a second lookup.
    pub fn emit(&mut self, op: DocOp) -> OpId {
        self.lamport = self.lamport.saturating_add(1);
        let id = OpId {
            lamport: self.lamport,
            actor: global_actor(),
        };
        self.apply(&op);
        self.log.push(OpRecord { id, op });
        id
    }

    /// Apply a single op to the materialised state. Idempotent on the
    /// shapes that matter (duplicate `PushStroke`, duplicate
    /// `RegisterBrush`, repeated `SetActiveLayer` on the same id) so
    /// Phase 10 replay from a shared log converges.
    fn apply(&mut self, op: &DocOp) {
        match op {
            DocOp::RegisterBrush { preset } => {
                if !self.brushes.iter().any(|b| b == preset) {
                    self.brushes.push(preset.clone());
                }
            }
            DocOp::PushStroke { layer_id, stroke } => {
                if let Some(layer) = self.layers.iter_mut().find(|l| l.id == *layer_id)
                    && !layer.strokes.iter().any(|s| s.id == stroke.id)
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
        }
    }

    /// Drop the op log while keeping the materialised state and the
    /// lamport counter. Future ops continue past the retained lamport
    /// so causal ordering survives compaction.
    pub fn compact_log(&mut self) {
        self.log.clear();
    }

    /// Trim the log if it has grown past [`SNAPSHOT_EVERY`]. Invoked
    /// before every persist so disk files never grow unbounded. Pure
    /// in-memory — writes happen elsewhere.
    pub fn maybe_compact_log(&mut self) {
        if self.log.len() >= SNAPSHOT_EVERY {
            self.compact_log();
        }
    }

    /// Insert `preset` if not already registered and return its
    /// [`BrushId`]. Deduping by structural equality keeps identical
    /// palette entries from bloating the registry when a doc is loaded
    /// and edited across sessions.
    /// # Panics
    /// If the registry already holds `u16::MAX` presets and the new
    /// entry would overflow. In practice a document never approaches
    /// this ceiling.
    pub fn register_brush(&mut self, preset: BrushPreset) -> BrushId {
        if let Some(idx) = self.brushes.iter().position(|b| *b == preset) {
            return BrushId(u16::try_from(idx).expect("brush registry overflow"));
        }
        let id = BrushId(u16::try_from(self.brushes.len()).expect("brush registry overflow"));
        self.emit(DocOp::RegisterBrush { preset });
        id
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

    /// Mint a fresh stroke id without pushing anything. Replaces the
    /// pre-UUID monotonic counter — UUID v4 is globally unique, so no
    /// per-doc state is needed.
    #[must_use]
    pub fn allocate_stroke_id(&self) -> StrokeId {
        StrokeId::new_v4()
    }

    #[must_use]
    pub fn find_stroke(&self, id: StrokeId) -> Option<&Stroke> {
        for layer in &self.layers {
            for stroke in &layer.strokes {
                if stroke.id == id {
                    return Some(stroke);
                }
            }
        }
        None
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
    /// Materialised view intended for read-only consumers (render,
    /// search). Mutations stay on dedicated methods so Phase 1.2 can
    /// make every one of them emit a [`DocOp`](crate::doc_op::DocOp).
    pub fn strokes(&self) -> impl Iterator<Item = &Stroke> {
        self.layers.iter().flat_map(|l| l.strokes.iter())
    }

    /// Count of committed strokes across every layer.
    #[must_use]
    pub fn stroke_count(&self) -> usize {
        self.layers.iter().map(|l| l.strokes.len()).sum()
    }

    #[must_use]
    pub fn log(&self) -> &[OpRecord] {
        &self.log
    }

    #[must_use]
    pub const fn lamport(&self) -> u64 {
        self.lamport
    }

    /// # Errors
    /// I/O or bincode encode failures.
    pub fn save(&mut self, path: impl AsRef<Path>) -> io::Result<()> {
        self.maybe_compact_log();
        let bytes = bincode::encode_to_vec(&*self, CODEC)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, bytes)
    }

    /// # Errors
    /// I/O or bincode decode failures.
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let bytes = std::fs::read(path)?;
        let (doc, _) = bincode::decode_from_slice::<Self, _>(&bytes, CODEC)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::InkPoint;

    #[test]
    fn every_mutation_appends_an_op_with_monotonic_lamport() {
        let mut d = Doc::default();
        let before = d.lamport();
        let bid = d.register_brush(BrushPreset::pen());
        let sid = d.push_stroke(bid, [1, 2, 3, 4], vec![InkPoint::new(0.0, 0.0, 0, 0, 0)]);
        let l2 = d.add_layer("L2");
        d.set_active_layer(l2);
        d.set_layer_visible(l2, false);
        d.set_layer_locked(l2, true);
        d.set_layer_opacity(l2, 0.5);
        d.remove_stroke(sid);
        d.remove_layer(l2);
        d.clear();
        // 9 ops: RegisterBrush, PushStroke, AddLayer, SetActiveLayer,
        // SetLayerVisible, SetLayerLocked, SetLayerOpacity,
        // RemoveStroke, RemoveLayer, Clear = 10. Count:
        assert_eq!(d.log().len(), 10);
        assert_eq!(d.lamport(), before + 10);
        let lamports: Vec<u64> = d.log().iter().map(|r| r.id.lamport).collect();
        assert!(lamports.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn apply_replay_rebuilds_state() {
        use crate::doc_op::OpRecord;
        let mut d = Doc::default();
        let bid = d.register_brush(BrushPreset::marker());
        d.push_stroke(bid, [10; 4], vec![InkPoint::new(0.0, 0.0, 0, 0, 0)]);
        let l2 = d.add_layer("Layer 2");
        d.set_active_layer(l2);
        d.push_stroke(bid, [20; 4], vec![InkPoint::new(1.0, 2.0, 0, 0, 0)]);
        d.set_layer_opacity(l2, 0.3);

        let log: Vec<OpRecord> = d.log().to_vec();
        let mut replay = Doc::default();
        for record in &log {
            replay.apply(&record.op);
        }
        assert_eq!(replay.brushes, d.brushes);
        assert_eq!(replay.layers, d.layers);
        assert_eq!(replay.active_layer, d.active_layer);
    }

    #[test]
    fn compact_log_preserves_state_and_lamport() {
        let mut d = Doc::default();
        let bid = d.register_brush(BrushPreset::pen());
        d.push_stroke(bid, [0; 4], vec![InkPoint::new(0.0, 0.0, 0, 0, 0)]);
        let lamport_before = d.lamport();
        let layers_before = d.layers.clone();
        d.compact_log();
        assert!(d.log().is_empty());
        assert_eq!(d.lamport(), lamport_before);
        assert_eq!(d.layers, layers_before);
    }

    #[test]
    fn register_dedupes() {
        let mut d = Doc::default();
        let a = d.register_brush(BrushPreset::pen());
        let b = d.register_brush(BrushPreset::pen());
        assert_eq!(a, b);
        assert_eq!(d.brushes.len(), 1);
    }

    #[test]
    fn strokes_iterator_flattens_layers() {
        let mut d = Doc::default();
        let bid = d.register_brush(BrushPreset::pen());
        d.push_stroke(bid, [0; 4], vec![InkPoint::new(0.0, 0.0, 0, 0, 0)]);
        let l2 = d.add_layer("Layer 2");
        d.set_active_layer(l2);
        d.push_stroke(bid, [0; 4], vec![InkPoint::new(1.0, 1.0, 0, 0, 0)]);
        assert_eq!(d.strokes().count(), 2);
        assert_eq!(d.stroke_count(), 2);
    }

    #[test]
    fn roundtrip_preserves_log_and_lamport() {
        use crate::doc_op::{DocOp, OpRecord};
        use crate::identity::{ActorId, DeviceId, UserId};
        use crate::op::OpId;
        let mut d = Doc::default();
        d.lamport = 7;
        d.log.push(OpRecord {
            id: OpId {
                lamport: 7,
                actor: ActorId {
                    user: UserId::new_v4(),
                    device: DeviceId::new_v4(),
                },
            },
            op: DocOp::Clear,
        });
        let bytes = bincode::encode_to_vec(&d, CODEC).unwrap();
        let (d2, _) = bincode::decode_from_slice::<Doc, _>(&bytes, CODEC).unwrap();
        assert_eq!(d, d2);
        assert_eq!(d2.lamport(), 7);
        assert_eq!(d2.log().len(), 1);
    }

    #[test]
    fn roundtrip() {
        let mut d = Doc::default();
        let bid = d.register_brush(BrushPreset::marker());
        let id = d.push_stroke(
            bid,
            [200, 20, 20, 255],
            vec![InkPoint::new(1.0, 2.0, 128, 32, 0)],
        );
        let bytes = bincode::encode_to_vec(&d, CODEC).unwrap();
        let (d2, _) = bincode::decode_from_slice::<Doc, _>(&bytes, CODEC).unwrap();
        assert_eq!(d, d2);
        assert!(d2.find_stroke(id).is_some());
    }

    #[test]
    fn add_and_switch_layer() {
        let mut d = Doc::default();
        let l2 = d.add_layer("Layer 2");
        assert!(d.set_active_layer(l2));
        assert_eq!(d.active_layer, l2);
        let bid = d.register_brush(BrushPreset::pen());
        let sid = d.push_stroke(bid, [0, 0, 0, 255], vec![InkPoint::new(0.0, 0.0, 0, 0, 0)]);
        // stroke lands in Layer 2, not Layer 1.
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
    }
}
