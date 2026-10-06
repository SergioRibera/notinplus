//! Undo log — currently scoped to erase sessions per PLAN Phase 3.
//!
//! An [`EraseSession`] tracks every stroke a single eraser gesture
//! touched (originals snapshot at first hit) plus the union of every
//! sample's clip circle. Doc mutation is deferred until pen-up: the
//! canvas paints an in-flight `DstOut` mask so the user sees the cut
//! immediately, but the actual per-stroke split runs once against the
//! accumulated circles when the session commits. This keeps the layer
//! stroke count minimal (one final split per touched stroke instead of
//! a cascade of per-sample intermediates) and makes cancel a no-op.
//!
//! Regular stroke undo is future work (not in PLAN Phase 3 scope).

use std::collections::HashSet;

use crate::brush::Stroke;
use crate::ids::StrokeId;

/// Stroke snapshot captured before an erase touched it.
///
/// Also carries the id of the layer it belonged to — rollback needs
/// to put each original back into the same layer, not into whatever
/// layer happens to be active at undo time.
#[derive(Debug, Clone)]
pub struct EraseOriginal {
    pub layer_id: u32,
    pub stroke: Stroke,
}

/// One user-visible erase gesture (pen-down → pen-up on an eraser).
/// Undoing it puts the doc back exactly as it was at pen-down.
#[derive(Debug, Default, Clone)]
pub struct EraseSession {
    /// Snapshots of every stroke the gesture will replace on commit.
    /// Populated at first-hit during accumulation; each id appears at
    /// most once.
    pub originals: Vec<EraseOriginal>,
    /// Ids of fragment strokes introduced when the session commits;
    /// removed on undo. Empty during accumulation — populated by the
    /// canvas at pen-up.
    pub added_fragments: Vec<StrokeId>,
    /// Clip circles collected across every sample of the gesture
    /// (world coordinates). The commit-time split derives final
    /// fragments by folding these in order against each snapshotted
    /// original.
    pub circles: Vec<(f32, f32, f32)>,
    /// Ids already snapshotted into `originals` — O(1) dedup so a
    /// stroke that gets grazed by many consecutive samples isn't
    /// snapshotted twice.
    touched_ids: HashSet<StrokeId>,
}

impl EraseSession {
    /// Record a clip circle (world coords, radius) sampled at pen-down
    /// or extend.
    pub fn push_circle(&mut self, cx: f32, cy: f32, r: f32) {
        self.circles.push((cx, cy, r));
    }

    /// Snapshot `stroke` iff its id hasn't been captured yet. Returns
    /// `true` when a fresh snapshot was taken.
    pub fn snapshot(&mut self, layer_id: u32, stroke: Stroke) -> bool {
        if !self.touched_ids.insert(stroke.id) {
            return false;
        }
        self.originals.push(EraseOriginal { layer_id, stroke });
        true
    }

    /// Ask whether `id` has already been snapshotted — cheap early-out
    /// so callers can skip the split-touched probe entirely.
    #[must_use]
    pub fn contains(&self, id: StrokeId) -> bool {
        self.touched_ids.contains(&id)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.originals.is_empty() && self.added_fragments.is_empty()
    }
}

#[derive(Debug, Clone)]
pub enum HistoryOp {
    Erase(EraseSession),
}
