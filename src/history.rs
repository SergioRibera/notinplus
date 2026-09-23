//! Undo log — currently scoped to erase sessions per PLAN Phase 3.
//!
//! A [`EraseSession`] snapshots the originals of every stroke the
//! session mutated (fully removed OR split) plus the fresh fragment
//! ids it introduced. Undo drops the fragments, restores the originals
//! back into their source layer, and lets the spatial index
//! re-populate from the restored state.
//!
//! Regular stroke undo is future work (not in PLAN Phase 3 scope).

use std::collections::HashSet;

use crate::brush::Stroke;

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
    /// Snapshots of every stroke the session mutated. A stroke that
    /// gets clipped twice inside the same session records only its
    /// very first snapshot.
    pub originals: Vec<EraseOriginal>,
    /// Ids of fragment strokes introduced during the session; removed
    /// on undo.
    pub added_fragments: Vec<u32>,
    /// Same ids as `added_fragments`, in a set for O(1) intermediate
    /// detection. Skipped by rollback (the vec drives that).
    added_set: HashSet<u32>,
}

impl EraseSession {
    /// Record `original` (from `layer_id`) iff its id hasn't been
    /// captured yet. `added` lists the fragment ids that replaced it
    /// (may be empty when the stroke was fully consumed).
    ///
    /// When `original.id` was itself introduced earlier in this
    /// session (an intermediate fragment being re-split by a later
    /// eraser sample), skip the `originals` push — restoring it on
    /// undo would resurrect a stroke that never existed at pen-down
    /// and inflate the layer's stroke count.
    pub fn record(&mut self, layer_id: u32, original: Stroke, added: &[u32]) {
        let is_intermediate = self.added_set.contains(&original.id);
        if !is_intermediate && !self.originals.iter().any(|o| o.stroke.id == original.id) {
            self.originals.push(EraseOriginal {
                layer_id,
                stroke: original,
            });
        }
        for &id in added {
            self.added_fragments.push(id);
            self.added_set.insert(id);
        }
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
