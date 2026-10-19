//! Mutations of a stroke-canvas [`Doc`](crate::doc::Doc).
//!
//! Every edit the canvas makes is one of these values. The doc mirrors
//! it into its materialised state and its store, and the undo stack
//! keeps the inverse list computed before the op landed.

use crate::bookmark::{Bookmark, Rgba, SourceRef, StrokeAnchor, TimestampMs};
use crate::brush::{BrushPreset, Stroke};
use crate::ids::{BookmarkId, StrokeId};
use crate::ink_glyph::{InkGlyph, InkGlyphId};
use crate::text_box::{TextBox, TextBoxId, TextFrame};

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
    /// Insert a text box on top of `layer_id`'s other texts.
    AddText {
        layer_id: u32,
        text: TextBox,
    },
    /// LWW on the body and its base colour.
    UpdateText {
        id: TextBoxId,
        markdown: String,
        color: [u8; 4],
    },
    /// Move, resize or rotate a text box.
    SetTextFrame {
        id: TextBoxId,
        frame: TextFrame,
    },
    DeleteText {
        id: TextBoxId,
    },
    /// Insert a recognized handwritten character on `layer_id`.
    AddGlyph {
        layer_id: u32,
        glyph: InkGlyph,
    },
    DeleteGlyph {
        id: InkGlyphId,
    },
}
