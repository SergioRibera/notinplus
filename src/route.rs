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

use std::sync::Arc;

use freya::prelude::*;
use freya::router::*;
use freya_engine::prelude::Color as SkColor;

use crate::app::root as canvas_root;
use crate::canvas::{Board, lock};
use crate::home::Home;

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


/// Infinite canvas — resets the board background to the default solid
/// fill on mount so navigating back from a PDF view starts fresh.
#[derive(Debug, PartialEq)]
pub struct CanvasView;

impl Component for CanvasView {
    fn render(&self) -> impl IntoElement {
        use_hook(reset_to_solid_background);
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

fn reset_to_solid_background() {
    use freya_canvas_bg::SolidColorBackground;
    let board = Board::shared();
    lock(&board).set_background(Arc::new(SolidColorBackground::new(SkColor::from_rgb(
        250, 250, 248,
    ))));
}
