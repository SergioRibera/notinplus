//! Navigation shell.
//!
//! Two routes: the Home grid and the mesa (`Space`) that shows every
//! open document as a tab. Opening anything goes through
//! [`crate::space::open`], which lands the tab and navigates here.
//!
//! Route ↔ component convention: `#[derive(Routable)]` materialises
//! each variant by constructing a struct of the same name — so
//! `Route::Home` builds `Home { }` and `Route::Space { id }` builds
//! `Space { id }`.
//!
//! Background handoff: per-item background selection travels through
//! [`queue_canvas_background`] as a one-shot hint — `home` sets it
//! immediately before opening a freshly created item and the mount
//! hook on the canvas pane drains the slot and installs the matching
//! background. Falls back to a blank off-white surface when nothing is
//! queued (fresh start / cold URL entry).

use std::sync::{Arc, Mutex, Weak};

use freya::prelude::*;
use freya::router::*;
use freya_canvas_bg::{
    CanvasBackground, DotGridBackground, GridBackground, LinedBackground, SolidColorBackground,
};
use freya_engine::prelude::Color as SkColor;

use crate::app::CanvasMount;
use crate::canvas::{Board, Viewport, lock};
use crate::doc::Doc;
use crate::home::Home;
use crate::library::{BackgroundStyle, ItemId, LibraryError, TabTarget, WorkspaceId};
use crate::prefs;
use crate::workspace_view::WorkspaceView;

/// Off-white paper used when the caller does not override the surface.
pub const DEFAULT_PAPER: SkColor = SkColor::from_rgb(250, 250, 248);

/// App router. `Home` is the landing grid; `Space { id }` is the one
/// editing surface — a mesa holding every open document as a tab.
#[derive(Routable, Clone, Debug, PartialEq)]
#[rustfmt::skip]
pub enum Route {
    #[route("/")]
    Home,
    #[route("/space/:id")]
    Space { id: WorkspaceId },
}

/// The mesa route: every open document of one workspace as tabs.
#[derive(Debug, PartialEq)]
pub struct Space {
    pub id: WorkspaceId,
}

impl Component for Space {
    fn render(&self) -> impl IntoElement {
        WorkspaceView { id: self.id }
    }
}

/// Entry point driven by [`crate::app::CanvasPane`] on mount. Runs the
/// per-board side effects: install queued background, hydrate the doc +
/// autosave sinks, and (for PDF-backed mounts) load the PDF into the
/// background slot off the UI thread.
pub(crate) fn init_canvas_mount(mount: CanvasMount, board: &Arc<Mutex<Board>>) {
    let had_pending = apply_pending_canvas_background(board, mount);
    // No pending slot — this mount did not come through the home open
    // path (e.g. re-opening the split in a DocLinked bundle). Pull the
    // item's persisted color + pattern from the library so the canvas
    // paints with the user's chosen backdrop instead of flashing the
    // default off-white paper.
    if !had_pending {
        if let Some(id) = mount.item() {
            let board = Arc::clone(board);
            spawn(async move {
                let handle = match crate::home::open_library().await {
                    Ok(h) => h,
                    Err(err) => {
                        log::warn!("init_canvas_mount: open_library: {err}");
                        return;
                    }
                };
                let (color, bg) = {
                    let lib = handle.lock().await;
                    let Some(item) = lib.index().item(id) else {
                        return;
                    };
                    (item.color, item.background)
                };
                let surface: SkColor = color
                    .map(|c| SkColor::from_argb(c[3], c[0], c[1], c[2]))
                    .unwrap_or(DEFAULT_PAPER);
                apply_canvas_background(&board, bg, surface);
            });
        }
    }
    load_doc_into_board(mount, Arc::clone(board));
}

/// Queue a background for the next mount of `target`. Callers invoke
/// this immediately before opening the tab — the mount hook of that
/// target drains the slot and installs the matching backend; other
/// panes mounting at the same time leave it alone.
pub fn queue_canvas_background(target: TabTarget, style: BackgroundStyle, surface: SkColor) {
    if let Ok(mut slot) = pending().lock() {
        *slot = Some(PendingBackground {
            target,
            style,
            surface,
        });
    }
}

#[derive(Clone, Copy)]
struct PendingBackground {
    target: TabTarget,
    style: BackgroundStyle,
    surface: SkColor,
}

fn pending() -> &'static Mutex<Option<PendingBackground>> {
    static SLOT: std::sync::OnceLock<Mutex<Option<PendingBackground>>> = std::sync::OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Hydrate the mounting canvas's doc from disk (or a blank
/// [`Doc::default`] for ephemeral / first-open flows), restore its
/// saved viewport, and wire the autosave + view workers to the per-mount
/// board. Each mount gets its own worker pair; dropping the previous
/// sender (via the next `set_commit_sink` / unmount) wakes the old
/// worker with `Disconnected` so it exits cleanly.
fn load_doc_into_board(mount: CanvasMount, board: Arc<Mutex<Board>>) {
    spawn(async move {
        let id = mount.item();
        let slot = match mount {
            CanvasMount::Item { slot, .. } => slot,
            CanvasMount::Ephemeral(_) => None,
        };
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
        let saved_view = id.and_then(|item| prefs::load_view(prefs::ViewKey { item, slot }));
        {
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
            match id {
                Some(id) => {
                    let (tx, rx) = flume::unbounded::<()>();
                    let (view_tx, view_rx) = flume::unbounded::<()>();
                    guard.set_commit_sink(tx);
                    guard.set_view_sink(view_tx);
                    guard.set_live_sink(move |message| crate::sync::live(id, &message));
                    guard.set_read_only(!crate::sync::can_edit(id));
                    spawn_autosave_worker(id, Arc::clone(&board), rx);
                    spawn_policy_worker(id, Arc::downgrade(&board));
                    spawn_view_worker(
                        prefs::ViewKey { item: id, slot },
                        Arc::clone(&board),
                        view_rx,
                    );
                }
                None => {
                    // Ephemeral: no sinks installed; previous mount's
                    // sinks already dropped with its board.
                    guard.clear_commit_sink();
                    guard.clear_view_sink();
                    guard.clear_live_sink();
                }
            }
        }
    });
}

/// Drive one autosave iteration per ping. `recv_async` blocks until a
/// commit fires, then `try_recv` drains any additional pings that landed
/// while this worker was awaiting the library mutex — a tight pen-up /
/// pen-down burst collapses into one write instead of several. The
/// worker owns its item id + a strong ref to this mount's board; when
/// the mount unmounts and the matching sender is dropped, the receiver
/// sees `Disconnected` on the next recv and the worker exits.
fn spawn_autosave_worker(id: ItemId, board: Arc<Mutex<Board>>, rx: flume::Receiver<()>) {
    spawn(async move {
        while rx.recv_async().await.is_ok() {
            while rx.try_recv().is_ok() {}
            let (bytes, captured) = {
                let guard = lock(&board);
                let doc = guard.doc();
                match doc.snapshot_bytes() {
                    Ok(bytes) => (bytes, crate::library::refs::capture(doc)),
                    Err(err) => {
                        log::error!("autosave snapshot id={id:?}: {err}");
                        continue;
                    }
                }
            };
            match crate::home::open_library().await {
                Ok(handle) => {
                    let mut lib = handle.lock().await;
                    if let Err(err) = lib.save_doc_snapshot(id, &bytes, captured).await {
                        log::error!("autosave save_doc id={id:?}: {err}");
                    }
                }
                Err(err) => log::error!("autosave open library: {err}"),
            }
            sync_siblings(id, &board);
            crate::sync::doc_changed(id);
        }
    });
}

/// Drive one view-sidecar write per ping. Same drain pattern as the doc
/// autosave worker — a pan burst collapses into a single write.
/// Keep the board's read-only gate in step with the share policy for
/// as long as the board lives: the owner may lift or impose it while
/// the document is open here.
fn spawn_policy_worker(id: ItemId, board: Weak<Mutex<Board>>) {
    let changes = crate::sync::changes();
    spawn(async move {
        while changes.recv_async().await.is_ok() {
            while changes.try_recv().is_ok() {}
            let Some(board) = board.upgrade() else {
                return;
            };
            lock(&board).set_read_only(!crate::sync::can_edit(id));
        }
    });
}

fn spawn_view_worker(key: prefs::ViewKey, board: Arc<Mutex<Board>>, rx: flume::Receiver<()>) {
    spawn(async move {
        while rx.recv_async().await.is_ok() {
            while rx.try_recv().is_ok() {}
            let view = {
                let guard = lock(&board);
                prefs::DocView::from_viewport(guard.viewport())
            };
            if let Err(err) = prefs::save_view(key, view) {
                log::error!("save_view item={:?}: {err}", key.item);
            }
        }
    });
}

fn apply_pending_canvas_background(board: &Arc<Mutex<Board>>, mount: CanvasMount) -> bool {
    let wanted = mount.target();
    let queued = pending().lock().ok().and_then(|mut slot| {
        if slot.is_some_and(|p| p.target == wanted) {
            slot.take()
        } else {
            None
        }
    });
    let had_queued = queued.is_some();
    let (style, surface) = queued.map_or((BackgroundStyle::default(), DEFAULT_PAPER), |p| {
        (p.style, p.surface)
    });
    apply_canvas_background(board, style, surface);
    had_queued
}

/// Install the `(style, surface)` pair on an already-mounted board.
/// Used by the edit-sheet flow so a tint / pattern change paints into
/// the live canvas immediately instead of waiting for the next mount.
pub fn apply_canvas_background(
    board: &Arc<Mutex<Board>>,
    style: BackgroundStyle,
    surface: SkColor,
) {
    let bg: Arc<dyn CanvasBackground> = match style {
        BackgroundStyle::Blank => Arc::new(SolidColorBackground::new(surface)),
        BackgroundStyle::Line => Arc::new(LinedBackground::new(surface)),
        BackgroundStyle::Grid => Arc::new(GridBackground::new(surface)),
        BackgroundStyle::DotGrid => Arc::new(DotGridBackground::new(surface)),
    };
    lock(board).set_background(bg);
}

/// Push what `board` knows about `item` into every other board showing
/// the same item, so two panes of one document stay identical while
/// each keeps its own viewport and undo history.
fn sync_siblings(item: ItemId, board: &Arc<Mutex<Board>>) {
    for (_, sibling) in crate::active_boards::for_item(item) {
        if Arc::ptr_eq(&sibling, board) {
            continue;
        }
        let version = lock(&sibling).doc().version();
        let updates = match lock(board).doc().updates_since(&version) {
            Ok(updates) => updates,
            Err(err) => {
                log::error!("sibling sync export id={item:?}: {err}");
                continue;
            }
        };
        if let Err(err) = lock(&sibling).merge_updates(&updates) {
            log::error!("sibling sync import id={item:?}: {err}");
        }
    }
}
