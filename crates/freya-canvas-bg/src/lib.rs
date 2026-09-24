//! Pluggable background layer for Freya canvas widgets.
//!
//! A [`CanvasBackground`] paints whatever lives *under* the user's
//! strokes on a Freya `canvas` element: solid color, a single image, a
//! stack of blank drawing pages, a rendered PDF, etc.
//!
//! The trait is intentionally executor-agnostic and framework-neutral
//! above `freya-engine` — a PDF backend crate can implement it once and
//! any Freya app that owns a paint routine can accept
//! `Arc<dyn CanvasBackground>` and swap backgrounds at runtime.
//!
//! # Contract
//!
//! - [`viewport_backdrop`](CanvasBackground::viewport_backdrop) returns
//!   the color the surrounding freya element paints *outside* the world
//!   transform. `None` means "let the app decide".
//! - [`paint`](CanvasBackground::paint) draws in **world coordinates**;
//!   the caller has already applied the pan/zoom transform on the Skia
//!   canvas.
//! - [`content_bounds`](CanvasBackground::content_bounds) tells the app
//!   how far the content extends so viewport panning can be clamped.
//!   `None` = infinite (no clamp).
//! - [`pages`](CanvasBackground::pages) enumerates page rectangles so
//!   the app can drive page-based UI (thumbnails, "page N of M",
//!   snap-to-page) without knowing the backend.
//! - [`attribution_mode`](CanvasBackground::attribution_mode) tells the
//!   host whether new strokes should be anchored to world coordinates
//!   (infinite canvas) or attached to the page they land on
//!   (document workflows).

#![warn(missing_docs)]

use std::sync::Arc;

pub use freya_engine::prelude::{Canvas, Color};

/// Stable identifier for a page inside a [`CanvasBackground`].
///
/// Backends pick their own numbering; the host must treat these as
/// opaque tokens. A PDF backend uses page index; a stacked-paper
/// backend uses insertion order; a single-image backend returns one
/// `PageId(0)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PageId(pub u64);

/// Axis-aligned rectangle in world coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Minimum x (inclusive).
    pub min_x: f32,
    /// Minimum y (inclusive).
    pub min_y: f32,
    /// Maximum x (exclusive).
    pub max_x: f32,
    /// Maximum y (exclusive).
    pub max_y: f32,
}

impl Rect {
    /// Build from corners; auto-normalises so `min ≤ max` per axis.
    #[must_use]
    pub fn from_corners(a: (f32, f32), b: (f32, f32)) -> Self {
        Self {
            min_x: a.0.min(b.0),
            min_y: a.1.min(b.1),
            max_x: a.0.max(b.0),
            max_y: a.1.max(b.1),
        }
    }

    /// Width along x.
    #[must_use]
    pub fn width(&self) -> f32 {
        self.max_x - self.min_x
    }

    /// Height along y.
    #[must_use]
    pub fn height(&self) -> f32 {
        self.max_y - self.min_y
    }

    /// `true` when the two rects share any interior area.
    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        self.min_x < other.max_x
            && other.min_x < self.max_x
            && self.min_y < other.max_y
            && other.min_y < self.max_y
    }

    /// `true` when `(x, y)` falls inside `[min, max)` on both axes.
    #[must_use]
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.min_x && x < self.max_x && y >= self.min_y && y < self.max_y
    }
}

/// Geometry entry for one page exposed by a [`CanvasBackground`].
#[derive(Debug, Clone, Copy)]
pub struct PageLayout {
    /// Backend-assigned identifier — stable across paint calls.
    pub id: PageId,
    /// World-space rectangle the page occupies.
    pub rect: Rect,
    /// Natural page size (points / pixels — backend-defined units).
    /// Diverges from `rect.width()/height()` when the layout applies
    /// per-page zoom (e.g. fitting a landscape PDF page inside a
    /// portrait stack).
    pub natural_size: (f32, f32),
}

/// How a background wants new strokes to be attributed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttributionMode {
    /// Strokes live in world coordinates, independent of any page.
    /// Right for infinite-canvas backgrounds (whiteboard, sketching).
    World,
    /// Strokes attach to the page the pen sample lands on. Right for
    /// document backgrounds (PDF annotation, per-page notebooks) so
    /// reordering / exporting pages carries strokes with them.
    PerPage,
}

/// Attachment produced when a background operates in
/// [`AttributionMode::PerPage`] and the host asks
/// [`CanvasBackground::locate`] where a world point belongs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageAttachment {
    /// Which page the point is on.
    pub page: PageId,
    /// Page-local coordinates (origin = top-left of the page's rect).
    pub local: (f32, f32),
}

/// Opaque wake-up handle a background calls when its async pipeline
/// (e.g. a freshly rendered PDF tile) produces a new frame the app
/// should repaint.
///
/// The host constructs this over its own redraw mechanism (Freya
/// window redraw request, a channel wake, etc.) and passes it through
/// [`BgPaintCtx`] on every paint so backends can capture it lazily.
#[derive(Clone)]
pub struct RedrawHandle(Arc<dyn Fn() + Send + Sync>);

impl RedrawHandle {
    /// Wrap any callable as a redraw handle.
    #[must_use]
    pub fn new<F>(f: F) -> Self
    where
        F: Fn() + Send + Sync + 'static,
    {
        Self(Arc::new(f))
    }

    /// A no-op handle. Fine for backgrounds that never do async work.
    #[must_use]
    pub fn noop() -> Self {
        Self(Arc::new(|| {}))
    }

    /// Request a repaint. Backends may call this from any thread.
    pub fn ping(&self) {
        (self.0)();
    }
}

impl std::fmt::Debug for RedrawHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedrawHandle").finish_non_exhaustive()
    }
}

/// Per-paint context handed to [`CanvasBackground::paint`].
///
/// The Skia canvas already has the world transform applied — draw in
/// world coordinates, no additional pan/zoom bookkeeping needed.
#[derive(Debug)]
pub struct BgPaintCtx<'a> {
    /// Skia canvas to draw on. Already positioned in world coordinates.
    pub canvas: &'a Canvas,
    /// Visible world rect — anything outside can be culled.
    pub visible: Rect,
    /// Current viewport scale factor (world → screen). Backends use
    /// this to pick a LOD level for cached bitmaps.
    pub scale: f32,
    /// Wake handle for async pipelines. Backends clone this into
    /// worker threads and `ping()` when a new frame lands.
    pub redraw: RedrawHandle,
}

/// A pluggable background layer for a Freya canvas.
///
/// Implementations must be cheap to clone (`Arc<dyn Self>` is the
/// expected shared shape) and thread-safe so async render pools can
/// hold references across worker threads.
pub trait CanvasBackground: Send + Sync + std::fmt::Debug {
    /// World-space extent of the content. `None` = unbounded canvas.
    fn content_bounds(&self) -> Option<Rect> {
        None
    }

    /// Enumerate every page this background exposes. Empty slice for
    /// non-paginated backgrounds (solid color, single image).
    fn pages(&self) -> &[PageLayout] {
        &[]
    }

    /// Color the freya element should paint *outside* the world
    /// transform (surface backdrop). `None` = host default.
    fn viewport_backdrop(&self) -> Option<Color> {
        None
    }

    /// How the host should attribute new strokes.
    fn attribution_mode(&self) -> AttributionMode {
        AttributionMode::World
    }

    /// Resolve a world point to a page attachment.
    ///
    /// Only meaningful when [`Self::attribution_mode`] returns
    /// [`AttributionMode::PerPage`]; the default implementation returns
    /// `None` unconditionally.
    fn locate(&self, _world_x: f32, _world_y: f32) -> Option<PageAttachment> {
        None
    }

    /// Draw the background in world coordinates.
    fn paint(&self, cx: &mut BgPaintCtx<'_>);

    /// Optional per-frame prefetch / eviction hook.
    ///
    /// Called by the host once per paint pass with the current viewport
    /// so async backends can queue tiles ahead of scroll and cancel
    /// out-of-range work.
    fn tick(&self, _visible: Rect, _scale: f32) {}
}

/// Uniform-color background — reproduces the "infinite paper" look.
///
/// Paints nothing in world space; the color travels via
/// [`viewport_backdrop`](CanvasBackground::viewport_backdrop) so the
/// host's outer element handles the fill in one pass.
#[derive(Debug, Clone, Copy)]
pub struct SolidColorBackground {
    color: Color,
}

impl SolidColorBackground {
    /// Build with the given surface color.
    #[must_use]
    pub const fn new(color: Color) -> Self {
        Self { color }
    }
}

impl CanvasBackground for SolidColorBackground {
    fn viewport_backdrop(&self) -> Option<Color> {
        Some(self.color)
    }

    fn paint(&self, _cx: &mut BgPaintCtx<'_>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_reports_backdrop_and_no_pages() {
        let bg = SolidColorBackground::new(Color::from_rgb(10, 20, 30));
        assert_eq!(bg.viewport_backdrop(), Some(Color::from_rgb(10, 20, 30)));
        assert!(bg.pages().is_empty());
        assert!(bg.content_bounds().is_none());
        assert_eq!(bg.attribution_mode(), AttributionMode::World);
    }

    #[test]
    fn rect_intersects_and_contains() {
        let a = Rect { min_x: 0.0, min_y: 0.0, max_x: 10.0, max_y: 10.0 };
        let b = Rect { min_x: 5.0, min_y: 5.0, max_x: 20.0, max_y: 20.0 };
        let c = Rect { min_x: 20.0, min_y: 20.0, max_x: 30.0, max_y: 30.0 };
        assert!(a.intersects(&b));
        assert!(!a.intersects(&c));
        assert!(a.contains(1.0, 1.0));
        assert!(!a.contains(10.0, 10.0));
    }

    #[test]
    fn redraw_handle_pings() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let counter = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&counter);
        let h = RedrawHandle::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        });
        h.ping();
        h.ping();
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
}
