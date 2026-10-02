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
use crate::canvas::{Board, Viewport, lock};
use crate::doc::Doc;
use crate::home::Home;
use crate::library::{BackgroundStyle, ItemId, LibraryError, bodies};
use crate::pen_pump;
use crate::prefs;

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
        use_hook(enable_pen_capture);
        use_drop(disable_pen_capture);
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
        use_hook(enable_pen_capture);
        use_drop(disable_pen_capture);
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
///
/// Also wires the autosave worker: a bounded channel ping fires at every
/// doc-mutating commit (stroke end, erase finalize, undo, layer ops) and
/// the worker drains any burst into a single `save_doc`. Blank-canvas
/// flows (`current_canvas_item() == None`) skip the write; there is no
/// persistent item to target yet.
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
        let saved_view = id.and_then(prefs::load_view);
        let (tx, rx) = flume::unbounded::<()>();
        let (view_tx, view_rx) = flume::unbounded::<()>();
        {
            let board = Board::shared();
            let mut guard = lock(&board);
            guard.replace_doc(doc);
            // Reset viewport first so a blank-canvas / fresh-doc flow
            // never inherits the previous doc's pan/zoom. Then apply
            // the saved view when present. Both run before the sink is
            // installed so the restore does not re-ping disk at mount.
            let next_view = saved_view
                .map(|v| Viewport {
                    tx: v.tx,
                    ty: v.ty,
                    scale: v.scale,
                })
                .unwrap_or_default();
            guard.set_viewport(next_view);
            // Installing after `replace_doc` is deliberate — `replace_doc`
            // itself does not ping the commit sink, but any prior sender
            // (from an earlier mount) is dropped here. Its worker's
            // receiver sees `Disconnected` on the next recv and exits.
            guard.set_commit_sink(tx);
            guard.set_view_sink(view_tx);
        }
        spawn_autosave_worker(rx);
        spawn_view_worker(view_rx);
    });
}

/// Drive one autosave iteration per ping. `recv_async` blocks until a
/// commit fires, then `try_recv` drains any additional pings that landed
/// while this worker was awaiting the library mutex — a tight pen-up /
/// pen-down burst collapses into one write instead of several.
fn spawn_autosave_worker(rx: flume::Receiver<()>) {
    spawn(async move {
        while rx.recv_async().await.is_ok() {
            while rx.try_recv().is_ok() {}
            let Some(id) = current_canvas_item() else {
                continue;
            };
            let doc_snapshot = {
                let board = Board::shared();
                let guard = lock(&board);
                guard.doc().clone()
            };
            match crate::home::open_library().await {
                Ok(handle) => {
                    let mut lib = handle.lock().await;
                    if let Err(err) = lib.save_doc(id, &doc_snapshot).await {
                        log::error!("autosave save_doc id={id:?}: {err}");
                    }
                }
                Err(err) => log::error!("autosave open library: {err}"),
            }
        }
    });
}

/// Drive one view-sidecar write per ping. Same drain pattern as the doc
/// autosave worker — a pan burst collapses into a single write.
fn spawn_view_worker(rx: flume::Receiver<()>) {
    spawn(async move {
        while rx.recv_async().await.is_ok() {
            while rx.try_recv().is_ok() {}
            let Some(id) = current_canvas_item() else {
                continue;
            };
            let view = {
                let board = Board::shared();
                let guard = lock(&board);
                prefs::DocView::from_viewport(guard.viewport())
            };
            if let Err(err) = prefs::save_view(id, view) {
                log::error!("save_view id={id:?}: {err}");
            }
        }
    });
}

/// Open the pen capture gate so contact samples reach the board.
fn enable_pen_capture() {
    pen_pump::set_capture_enabled(true);
}

/// Close the pen capture gate and drop any in-flight stroke. Called on
/// canvas unmount so a tap on Home that lands on a doc card never
/// delivers a pending Up back into the board after navigation.
fn disable_pen_capture() {
    pen_pump::set_capture_enabled(false);
    lock(&Board::shared()).cancel();
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
