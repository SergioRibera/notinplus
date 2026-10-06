//! Bookmark model.
//!
//! Phase 1.4 of `SOURCES_PLAN`. Bookmarks are the user-facing pins
//! that annotate a canvas (and, in later phases, point at source
//! anchors + other bookmarks). Scope here is the on-disk shape + the
//! `DocOp` variants that mutate it — the pin UI (long-press to create,
//! chip rendering, inline `[[…]]` body parser) lands in Phase 1.5+.
//!
//! Identity is a UUID v4 ([`BookmarkId`]) so a bookmark survives
//! cross-device merges and deeplinks without colliding. The on-disk
//! layout stays flat (`Doc::bookmarks: Vec<Bookmark>`) — a tree /
//! index can lift on top later without a wire break.

use crate::ids::{BookmarkId, StrokeId};

/// `(x, y)` in canvas world coordinates. Separate type over the raw
/// tuple so API signatures communicate intent — bookmark positions
/// are world-space, not surface-pixel.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct WorldPoint {
    pub x: f32,
    pub y: f32,
}

impl WorldPoint {
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Non-premultiplied RGBA — same shape every stroke / brush uses.
pub type Rgba = [u8; 4];

/// Wallclock millis since Unix epoch. Display-only — merge order
/// relies on the lamport stamp carried by the op record, never on
/// `created_at` / `updated_at`.
pub type TimestampMs = u64;

/// Sticky link from a bookmark to a stroke. The pin re-derives its
/// world position from the stroke's current AABB + a stored offset
/// so moving / re-scaling the stroke drags the pin along.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StrokeAnchor {
    pub stroke: StrokeId,
    /// Offset relative to the stroke's AABB origin, in world units.
    /// Stays in world units (not normalised) so the pin keeps a stable
    /// physical offset even when the stroke's bbox grows.
    pub offset_in_bbox: (f32, f32),
}

/// Where a bookmark points.
///
/// Three variants cover every target that Phase 1-plus bookmarks can
/// name: another bookmark in the library, an anchor inside a source
/// plugin, or an arbitrary URL. The `Plugin` variant carries an opaque
/// bincode blob the owning plugin alone can decode — core never peeks
/// inside, which keeps third-party source plugins additive without a
/// wire break.
#[istmo::message]
#[derive(Clone, PartialEq, Debug)]
pub enum SourceRef {
    /// Another bookmark in the library.
    Canvas { bookmark: BookmarkId },
    /// An anchor inside a source plugin.
    Plugin {
        plugin_id: String,
        anchor: Vec<u8>,
        selection: Option<String>,
    },
    /// Arbitrary URL / mailto / external file.
    External { url: String },
}

/// A pinned annotation on a canvas.
///
/// `anchor` is the world-space position the pin paints at. When
/// `stuck_to` is `Some`, the live position is recomputed from the
/// stroke's current AABB; `anchor` is only the fallback snapshot used
/// when the sticky target disappears (chip ⚠ "ancla rota").
#[istmo::message]
#[derive(Clone, PartialEq, Debug)]
pub struct Bookmark {
    pub id: BookmarkId,
    pub anchor: WorldPoint,
    pub stuck_to: Option<StrokeAnchor>,
    /// Markdown-lite body. Phase 1.5+ parses `[[…]]` chips out of it.
    pub body: String,
    /// Explicit refs rendered as chips alongside the body.
    pub refs: Vec<SourceRef>,
    pub color: Option<Rgba>,
    pub created_at: TimestampMs,
    pub updated_at: TimestampMs,
    /// Lamport stamp of the latest op that touched this bookmark. Used
    /// by remote peers to decide LWW winners for `UpdateBookmark`.
    pub lamport: u64,
}

impl Bookmark {
    /// Fresh bookmark anchored at `anchor` with the given creation
    /// timestamp. Body + refs start empty; color unset; `stuck_to`
    /// `None` so a bare `Add` doesn't attach to a stroke until a
    /// matching `StickBookmark` lands.
    #[must_use]
    pub const fn new(id: BookmarkId, anchor: WorldPoint, created_at: TimestampMs) -> Self {
        Self {
            id,
            anchor,
            stuck_to: None,
            body: String::new(),
            refs: Vec::new(),
            color: None,
            created_at,
            updated_at: created_at,
            lamport: 0,
        }
    }
}
