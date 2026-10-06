//! Local undo/redo stacks + inverse-op grouping.
//!
//! Phase 1.3 of `SOURCES_PLAN`. Replaces the erase-only `history.rs`
//! shape with a stack-of-transactions model that every `Doc`-touching
//! mutation flows through. One user-visible action (stroke, erase,
//! layer attr change, clear) is one transaction — a run of [`DocOp`]s
//! bracketed by [`UndoStack::begin`] + [`UndoStack::commit`]. Undo
//! applies the pre-captured inverse ops in order; redo re-applies the
//! original forward ops. Both land through `Doc::emit` so the op log
//! stays append-only — the CRDT shape promised to Phase 10 does not
//! change at undo time.
//!
//! Rationale (see `SOURCES_PLAN` §4): popping the log would diverge
//! peers once Phase 10 wires sync; Figma / Docs style undo is a fresh
//! forward op authored by this actor, carried by a brand new `OpId`.
//! The pre-captured inverse list stays local — it never crosses the
//! wire.

use crate::doc_op::DocOp;

/// Hard cap on the retained user actions. Oldest entry evicted when
/// a new transaction overflows the cap — tracked in `SOURCES_PLAN`
/// §10 as the undo-stack blow-up mitigation. 100 user actions covers
/// any realistic ink session.
pub const MAX_DEPTH: usize = 100;

/// One undoable user action.
///
/// `forward` is the op sequence as emitted; `inverse` is the op
/// sequence that cancels it. The inverse list is already stored in
/// the order it should be applied — iterate front-to-back and emit
/// each op through the normal `Doc::emit` path to roll back.
#[derive(Debug, Clone, Default)]
pub struct UndoEntry {
    pub forward: Vec<DocOp>,
    pub inverse: Vec<DocOp>,
}

impl UndoEntry {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }
}

/// Undo + redo stacks with transaction grouping.
///
/// A transaction coalesces every op recorded between a `begin` and
/// its matching `commit` into a single `UndoEntry`. `begin`/`commit`
/// nest safely — only the outermost pair publishes the entry.
#[derive(Debug, Default)]
pub struct UndoStack {
    undo: Vec<UndoEntry>,
    redo: Vec<UndoEntry>,
    pending: Option<UndoEntry>,
    depth: u32,
}

impl UndoStack {
    /// Open a transaction. Nested calls increase the depth without
    /// restarting the pending entry so inner groupings fold into the
    /// outer action — a stroke gesture that triggers an auto-layer
    /// AddLayer stays one undo entry.
    pub fn begin(&mut self) {
        if self.depth == 0 {
            self.pending = Some(UndoEntry::default());
        }
        self.depth = self.depth.saturating_add(1);
    }

    /// Record one op + its pre-computed inverse list.
    ///
    /// `inverse` is accepted in the order it must be re-emitted to
    /// undo `forward`: `inverse[0]` applies first, `inverse[last]`
    /// applies last. The stack preserves that order within a single
    /// op, and prepends newer ops' inverses so a transaction's final
    /// inverse list reads newest-first across ops — iterate front to
    /// back and the state unwinds in reverse of how it was built.
    ///
    /// Without an open transaction the op is sealed into a one-op
    /// entry directly — e.g. toolbar toggles that aren't part of a
    /// larger gesture.
    pub fn record(&mut self, forward: DocOp, inverse: Vec<DocOp>) {
        if let Some(pending) = self.pending.as_mut() {
            pending.forward.push(forward);
            // Prepend this op's inverse block ahead of anything
            // already queued so the final list is
            // `concat(newest_inverse, ..., oldest_inverse)` while
            // preserving the application order inside each block.
            for inv in inverse.into_iter().rev() {
                pending.inverse.insert(0, inv);
            }
            return;
        }
        let entry = UndoEntry {
            forward: vec![forward],
            inverse,
        };
        if entry.is_empty() {
            return;
        }
        self.push_undo_entry(entry);
    }

    /// Close one `begin`. Only the outermost close publishes the
    /// pending entry; empty transactions (begin → commit with no
    /// recorded ops) are dropped silently.
    pub fn commit(&mut self) {
        if self.depth == 0 {
            return;
        }
        self.depth -= 1;
        if self.depth > 0 {
            return;
        }
        let Some(entry) = self.pending.take() else {
            return;
        };
        if entry.is_empty() {
            return;
        }
        self.push_undo_entry(entry);
    }

    /// Discard any in-flight transaction without publishing it. Used
    /// by cancel paths where the gesture never produced ops.
    pub fn abort(&mut self) {
        self.pending = None;
        self.depth = 0;
    }

    #[must_use]
    pub const fn in_tx(&self) -> bool {
        self.depth > 0
    }

    /// Pop the top undo entry. Caller is expected to apply each op
    /// in `entry.inverse` (in order) via `Doc::emit`, then hand the
    /// entry to [`Self::push_redo`] so the user can redo it.
    pub fn pop_undo(&mut self) -> Option<UndoEntry> {
        self.undo.pop()
    }

    /// Pop the top redo entry. Caller applies `entry.forward` in
    /// order, then hands the entry back via [`Self::push_undo_raw`].
    pub fn pop_redo(&mut self) -> Option<UndoEntry> {
        self.redo.pop()
    }

    /// Push an already-formed entry back onto the undo stack without
    /// clearing redo. Used by [`Self::pop_redo`] callers after they
    /// finish re-applying the forward ops.
    pub fn push_undo_raw(&mut self, entry: UndoEntry) {
        if entry.is_empty() {
            return;
        }
        if self.undo.len() >= MAX_DEPTH {
            self.undo.remove(0);
        }
        self.undo.push(entry);
    }

    /// Push an entry onto the redo stack. Used by undo-apply callers
    /// to let the user retrace.
    pub fn push_redo(&mut self, entry: UndoEntry) {
        if entry.is_empty() {
            return;
        }
        if self.redo.len() >= MAX_DEPTH {
            self.redo.remove(0);
        }
        self.redo.push(entry);
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.pending = None;
        self.depth = 0;
    }

    #[must_use]
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    #[must_use]
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    fn push_undo_entry(&mut self, entry: UndoEntry) {
        if self.undo.len() >= MAX_DEPTH {
            self.undo.remove(0);
        }
        self.undo.push(entry);
        // Any new user action invalidates the redo stack — standard
        // editor semantics. If the user wants both branches, they fork
        // the doc.
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::StrokeId;

    fn remove_op() -> DocOp {
        DocOp::RemoveStroke {
            id: StrokeId::new_v4(),
        }
    }

    #[test]
    fn record_outside_tx_seals_one_op_entry() {
        let mut s = UndoStack::default();
        let fwd = remove_op();
        let inv = vec![remove_op()];
        s.record(fwd, inv);
        assert_eq!(s.undo_len(), 1);
        assert_eq!(s.redo_len(), 0);
    }

    #[test]
    fn tx_coalesces_multiple_ops() {
        let mut s = UndoStack::default();
        s.begin();
        s.record(remove_op(), vec![remove_op()]);
        s.record(remove_op(), vec![remove_op()]);
        assert_eq!(s.undo_len(), 0, "pending until commit");
        s.commit();
        assert_eq!(s.undo_len(), 1, "single entry per tx");
        let entry = s.pop_undo().unwrap();
        assert_eq!(entry.forward.len(), 2);
        assert_eq!(entry.inverse.len(), 2);
    }

    #[test]
    fn nested_tx_folds_into_outer() {
        let mut s = UndoStack::default();
        s.begin();
        s.record(remove_op(), vec![remove_op()]);
        s.begin();
        s.record(remove_op(), vec![remove_op()]);
        s.commit();
        s.record(remove_op(), vec![remove_op()]);
        s.commit();
        assert_eq!(s.undo_len(), 1);
        let entry = s.pop_undo().unwrap();
        assert_eq!(entry.forward.len(), 3);
    }

    #[test]
    fn inverse_blocks_are_prepended_newest_first() {
        // Three ops a, b, c with single-element inverses A, B, C.
        // The final inverse list should start with C (newest op's
        // inverse applied first) then B, then A — unwinds in reverse
        // of how the ops were applied.
        let mut s = UndoStack::default();
        let a = DocOp::SetActiveLayer { id: 1 };
        let b = DocOp::SetActiveLayer { id: 2 };
        let c = DocOp::SetActiveLayer { id: 3 };
        let a_inv = DocOp::SetActiveLayer { id: 0 };
        let b_inv = DocOp::SetActiveLayer { id: 1 };
        let c_inv = DocOp::SetActiveLayer { id: 2 };
        s.begin();
        s.record(a, vec![a_inv.clone()]);
        s.record(b, vec![b_inv.clone()]);
        s.record(c, vec![c_inv.clone()]);
        s.commit();
        let entry = s.pop_undo().unwrap();
        assert_eq!(entry.inverse, vec![c_inv, b_inv, a_inv]);
    }

    #[test]
    fn multi_op_inverse_preserves_internal_order() {
        // A single forward op whose inverse spans multiple ops must
        // keep that internal order intact — e.g. undoing a `Clear`
        // must `AddLayer` before `PushStroke` into that layer.
        let mut s = UndoStack::default();
        let fwd = DocOp::Clear;
        let inv0 = DocOp::AddLayer {
            id: 7,
            name: "L".into(),
        };
        let inv1 = DocOp::SetActiveLayer { id: 7 };
        s.record(fwd, vec![inv0.clone(), inv1.clone()]);
        let entry = s.pop_undo().unwrap();
        assert_eq!(entry.inverse, vec![inv0, inv1]);
    }

    #[test]
    fn new_action_clears_redo() {
        let mut s = UndoStack::default();
        s.record(remove_op(), vec![remove_op()]);
        let entry = s.pop_undo().unwrap();
        s.push_redo(entry);
        assert_eq!(s.redo_len(), 1);
        s.record(remove_op(), vec![remove_op()]);
        assert_eq!(s.redo_len(), 0, "new action invalidates redo");
    }

    #[test]
    fn abort_discards_pending() {
        let mut s = UndoStack::default();
        s.begin();
        s.record(remove_op(), vec![remove_op()]);
        s.abort();
        assert_eq!(s.undo_len(), 0);
        assert!(!s.in_tx());
    }

    #[test]
    fn depth_cap_evicts_oldest() {
        let mut s = UndoStack::default();
        for _ in 0..MAX_DEPTH + 5 {
            s.record(remove_op(), vec![remove_op()]);
        }
        assert_eq!(s.undo_len(), MAX_DEPTH);
    }
}
