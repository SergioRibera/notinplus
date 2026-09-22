//! Undo log — currently scoped to erase sessions per PLAN Phase 3.
//!
//! A [`EraseSession`] snapshots the originals of every stroke the
//! session mutated (fully removed OR split) plus the fresh fragment
//! ids it introduced. Undo drops the fragments, restores the originals
//! back into their source layer, and lets the spatial index
//! re-populate from the restored state.
//!
//! Regular stroke undo is future work (not in PLAN Phase 3 scope).

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
}

impl EraseSession {
    /// Record `original` (from `layer_id`) iff its id hasn't been
    /// captured yet. `added` lists the fragment ids that replaced it
    /// (may be empty when the stroke was fully consumed).
    pub fn record(&mut self, layer_id: u32, original: Stroke, added: &[u32]) {
        if !self.originals.iter().any(|o| o.stroke.id == original.id) {
            self.originals.push(EraseOriginal {
                layer_id,
                stroke: original,
            });
        }
        self.added_fragments.extend_from_slice(added);
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
