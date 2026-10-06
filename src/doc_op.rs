//! CRDT op shape for the stroke-canvas doc.
//!
//! Phase 1.1 scaffold — the enum lists every mutation `Doc` currently
//! exposes so Phase 1.2 is a mechanical rewrite from `self.layers.push`
//! style call sites to `self.emit(DocOp::…)`. Phase 1.4 appends the
//! `AddBookmark` / `UpdateBookmark` / `DeleteBookmark` / `StickBookmark`
//! variants once the bookmark model lands. Phase 10 transports the
//! same bytes over the wire — no further shape change is expected.
//!
//! The log is append-only; `DeleteStroke` keeps the stroke payload in
//! the matching `PushStroke` op so an undo (inverse op) can rebuild it
//! without the stroke needing to survive outside the log.

use crate::bookmark::{Bookmark, Rgba, SourceRef, StrokeAnchor, TimestampMs};
use crate::brush::{BrushPreset, Stroke};
use crate::ids::{BookmarkId, StrokeId};
use crate::op::OpId;

/// Wrapper carrying the `OpId` stamp alongside the op payload.
#[istmo::message]
#[derive(Clone, PartialEq, Debug)]
pub struct OpRecord {
    pub id: OpId,
    pub op: DocOp,
}

/// Mutation the stroke canvas plugin understands.
///
/// Every variant round-trips through bincode and carries enough state
/// for a receiving replica to apply it deterministically. Layer ids
/// stay `u32` (library-scoped monotonic) until workspaces land in
/// Phase 4; stroke ids are UUIDs so cross-device merges never collide.
#[istmo::message]
#[derive(Clone, PartialEq, Debug)]
pub enum DocOp {
    RegisterBrush {
        preset: BrushPreset,
    },
    PushStroke {
        layer_id: u32,
        stroke: Stroke,
    },
    RemoveStroke {
        id: StrokeId,
    },
    Clear,
    AddLayer {
        id: u32,
        name: String,
    },
    RemoveLayer {
        id: u32,
    },
    SetActiveLayer {
        id: u32,
    },
    SetLayerVisible {
        id: u32,
        visible: bool,
    },
    SetLayerLocked {
        id: u32,
        locked: bool,
    },
    SetLayerOpacity {
        id: u32,
        opacity: f32,
    },
    /// Insert a bookmark. Full struct carried so a replay from the
    /// log is deterministic — no out-of-band state needed.
    AddBookmark {
        bookmark: Bookmark,
    },
    /// LWW on the mutable text + chip fields. `updated_at` travels
    /// alongside for display; merge order still falls back to the
    /// op's lamport stamp.
    UpdateBookmark {
        id: BookmarkId,
        body: String,
        refs: Vec<SourceRef>,
        color: Option<Rgba>,
        updated_at: TimestampMs,
    },
    DeleteBookmark {
        id: BookmarkId,
    },
    /// Set / clear the sticky stroke link. `None` promotes the pin to
    /// free world-coord mode; `Some(anchor)` repegs it to a stroke.
    StickBookmark {
        id: BookmarkId,
        to: Option<StrokeAnchor>,
    },
}
