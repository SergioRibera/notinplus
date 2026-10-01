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
use freya_pdf::{PdfBackground, PdfDocument};

use crate::app::root as canvas_root;
use crate::canvas::{Board, lock};
use crate::doc::Doc;
use crate::home::Home;
use crate::library::{BackgroundStyle, ItemId, LibraryError, bodies};

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
        use_hook(load_current_doc_into_board);
        canvas_root()
    }
}

/// PDF-backed canvas. Reads the attached PDF for
/// [`current_canvas_item`], builds a [`PdfBackground`] from it, and
/// installs that as the board's background so strokes drawn on top of
/// the pages share the same world coordinates as the PDF geometry.
#[derive(Debug, PartialEq)]
pub struct CanvasPdfView;

impl Component for CanvasPdfView {
    fn render(&self) -> impl IntoElement {
        use_hook(load_current_doc_into_board);
        use_hook(|| {
            let Some(id) = current_canvas_item() else {
                log::warn!("CanvasPdfView mounted without current_canvas_item");
                return;
            };
            // Spawn so pdfium init + file read run off the UI thread
            // — the paint pass can start on the solid-color placeholder
            // the caller queued, then swap to the real pages once the
            // document is ready.
            spawn(async move {
                match load_pdf_background(id) {
                    Ok(bg) => lock(&Board::shared()).set_background(bg),
                    Err(err) => log::error!("load pdf id={id:?}: {err}"),
                }
            });
        });
        canvas_root()
    }
}

fn load_pdf_background(id: ItemId) -> Result<Arc<dyn CanvasBackground>, String> {
    let bytes = bodies::read_pdf(id).map_err(|e| format!("read pdf: {e}"))?;
    let doc = PdfDocument::open_bytes(bytes).map_err(|e| format!("open pdf: {e:?}"))?;
    Ok(Arc::new(PdfBackground::new(doc)))
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

/// Swap the shared [`Board`]'s doc for the one belonging to the mounting
/// view. Without this the process-wide `Board` keeps the previous doc's
/// strokes — a cross-document leak that also masks the lack of load on
/// cold start. `None` (quick blank-canvas flow) resets to [`Doc::default`]
/// so a fresh scratch surface never inherits the last doc's strokes.
fn load_current_doc_into_board() {
    let id = current_canvas_item();
    spawn(async move {
        let doc = match id {
            Some(id) => match crate::home::open_library().await {
                Ok(handle) => {
                    let lib = handle.lock().await;
                    match lib.load_doc(id).await {
                        Ok(doc) => doc,
                        // First open of a freshly-created item has no body
                        // on disk yet — start from a blank doc instead of
                        // leaving the previous view's strokes in place.
                        Err(LibraryError::Io(ref e))
                            if e.kind() == std::io::ErrorKind::NotFound =>
                        {
                            Doc::default()
                        }
                        Err(err) => {
                            log::error!("load_doc id={id:?}: {err}");
                            return;
                        }
                    }
                }
                Err(err) => {
                    log::error!("open library on canvas enter: {err}");
                    return;
                }
            },
            None => Doc::default(),
        };
        lock(&Board::shared()).replace_doc(doc);
    });
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
