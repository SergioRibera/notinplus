//! Mobile navigation shell.
//!
//! Wires [`freya_router`] into the mobile entry point so the shell can
//! flip between a landing page and either an infinite canvas or a
//! PDF-backed one. Desktop still uses [`crate::app::root`] directly
//! since the router adds no value until we grow more views.
//!
//! Route ↔ component convention: `#[derive(Routable)]` materialises
//! each variant by constructing a struct of the same name — so
//! `Route::Home` builds `Home { }`, `Route::CanvasView` builds
//! `CanvasView { }`, etc. All three component structs live below.
//!
//! Background handoff: routes carry no arguments, so per-item background
//! selection travels through [`queue_canvas_background`]. `home` sets it
//! immediately before pushing `Route::CanvasView` and the mount hook on
//! [`CanvasView`] drains the slot and installs the matching background.
//! Falls back to a blank off-white surface when nothing is queued (fresh
//! start / cold URL entry).

use std::sync::{Arc, Mutex};

use freya::prelude::*;
use freya::router::*;
use freya_canvas_bg::{
    CanvasBackground, DotGridBackground, GridBackground, LinedBackground, SolidColorBackground,
};
use freya_engine::prelude::Color as SkColor;

use crate::app::root as canvas_root;
use crate::canvas::{Board, lock};
use crate::home::Home;
use crate::library::{BackgroundStyle, ItemId};

/// Off-white paper used when the caller does not override the surface.
pub const DEFAULT_PAPER: SkColor = SkColor::from_rgb(250, 250, 248);

/// Mobile app router. `Home` is the initial route; the two buttons on
/// the landing page push either [`Route::CanvasView`] (blank infinite
/// canvas) or [`Route::CanvasPdfView`] (canvas overlaid on a PDF
/// previously loaded via the file-picker plugin).
#[derive(Routable, Clone, Debug, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[route("/")]
    Home,
    #[route("/canvas")]
    CanvasView,
    #[route("/pdf")]
    CanvasPdfView,
}

/// Infinite canvas — installs whichever background was queued by the
/// caller (or the default off-white when nothing was queued) so freshly
/// opened items pick up their persisted pattern.
#[derive(Debug, PartialEq)]
pub struct CanvasView;

impl Component for CanvasView {
    fn render(&self) -> impl IntoElement {
        use_hook(apply_pending_canvas_background);
        canvas_root()
    }
}

/// PDF-backed canvas. The picker flow in [`home`] already sets the
/// board background before pushing this route, so the component's
/// only job is to render the canvas over it.
#[derive(Debug, PartialEq)]
pub struct CanvasPdfView;

impl Component for CanvasPdfView {
    fn render(&self) -> impl IntoElement {
        canvas_root()
    }
}

/// Queue a background for the next [`Route::CanvasView`] mount. Callers
/// invoke this immediately before `router.push(Route::CanvasView)` — the
/// mount hook drains the queue and installs the matching backend.
pub fn queue_canvas_background(style: BackgroundStyle, surface: SkColor) {
    if let Ok(mut slot) = pending().lock() {
        *slot = Some(PendingBackground { style, surface });
    }
}

/// Record which `Item` the canvas view is currently editing. `home.rs`
/// sets this before `router.push(Route::CanvasView)` so the canvas back
/// button knows where to save the doc on exit. `None` for the "blank
/// canvas" quick flow that doesn't have a persistent item yet.
pub fn set_current_canvas_item(item: Option<ItemId>) {
    if let Ok(mut slot) = current_item().lock() {
        *slot = item;
    }
}

/// Which item is currently open in the canvas view, if any.
#[must_use]
pub fn current_canvas_item() -> Option<ItemId> {
    current_item().lock().ok().and_then(|slot| *slot)
}

#[derive(Clone, Copy)]
struct PendingBackground {
    style: BackgroundStyle,
    surface: SkColor,
}

fn pending() -> &'static Mutex<Option<PendingBackground>> {
    static SLOT: std::sync::OnceLock<Mutex<Option<PendingBackground>>> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn current_item() -> &'static Mutex<Option<ItemId>> {
    static SLOT: std::sync::OnceLock<Mutex<Option<ItemId>>> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn apply_pending_canvas_background() {
    let queued = pending().lock().ok().and_then(|mut slot| slot.take());
    let (style, surface) = queued.map_or((BackgroundStyle::default(), DEFAULT_PAPER), |p| {
        (p.style, p.surface)
    });
    let bg: Arc<dyn CanvasBackground> = match style {
        BackgroundStyle::Blank => Arc::new(SolidColorBackground::new(surface)),
        BackgroundStyle::Line => Arc::new(LinedBackground::new(surface)),
        BackgroundStyle::Grid => Arc::new(GridBackground::new(surface)),
        BackgroundStyle::DotGrid => Arc::new(DotGridBackground::new(surface)),
    };
    lock(&Board::shared()).set_background(bg);
}
