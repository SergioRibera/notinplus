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

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use freya::prelude::*;
use freya::router::Routable;
use freya_canvas_bg::{
    CanvasBackground, DotGridBackground, GridBackground, LinedBackground, SolidColorBackground,
};
use freya_engine::prelude::Color as SkColor;

use crate::app::CanvasMount;
use crate::canvas::{Board, Viewport, lock};
use crate::doc::Doc;
use crate::library::{BackgroundStyle, ItemId, LibraryError, TabTarget, WorkspaceId};
use crate::prefs;
use crate::screens::home::Home;
use crate::screens::trash::Trash;

/// Off-white paper used when the caller does not override the surface.
pub const DEFAULT_PAPER: SkColor = SkColor::from_rgb(250, 250, 248);

/// App router. `Home` is the landing grid; `Space { id }` is the one
/// editing surface — a mesa holding every open document as a tab.
#[derive(Routable, Clone, Debug, PartialEq, Eq)]
#[rustfmt::skip]
pub enum Route {
    #[route("/")]
    Home,
    #[route("/trash")]
    Trash,
    #[route("/space/:id")]
    Space { id: WorkspaceId },
}

/// The mesa route: every open document of one workspace as tabs.
#[derive(Debug, PartialEq, Eq)]
pub struct Space {
    pub id: WorkspaceId,
}

impl Component for Space {
    fn render(&self) -> impl IntoElement {
        crate::screens::mesa::Mesa { id: self.id }
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
    // default off-white paper. A face that paints its own pages keeps
    // them: the paper would land on top once the library answered.
    let face_paints = try_consume_context::<crate::app::FacePaintsBackground>().is_some();
    if !had_pending && !face_paints {
        if let Some(id) = mount.item() {
            let board = Arc::clone(board);
            spawn(async move {
                let handle = match crate::library::shared().await {
                    Ok(h) => h,
                    Err(err) => {
                        log::warn!("init_canvas_mount: open_library: {err}");
                        return;
                    }
                };
                let found = handle
                    .lock()
                    .await
                    .index()
                    .item(id)
                    .map(|item| (item.color, item.background));
                let Some((color, bg)) = found else {
                    return;
                };
                let surface: SkColor = color.map_or(DEFAULT_PAPER, |c| {
                    SkColor::from_argb(c[3], c[0], c[1], c[2])
                });
                apply_canvas_background(&board, bg, surface);
            });
        }
    }
    load_doc_into_board(mount, Arc::clone(board));
}

/// Queue a background for the next mount of `target`.
///
/// Callers invoke this immediately before opening the tab — the mount hook of
/// that target drains the slot and installs the matching backend; other panes
/// mounting at the same time leave it alone.
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
            Some(id) => match crate::library::shared().await {
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
            if let Some(id) = id {
                let (tx, rx) = flume::unbounded::<()>();
                let (view_tx, view_rx) = flume::unbounded::<()>();
                guard.set_commit_sink(tx);
                guard.set_view_sink(view_tx);
                guard.set_live_sink(move |message| crate::sync::live(id, &message));
                guard.set_read_only(!crate::sync::can_edit(id));
                spawn_autosave_worker(id, &board, rx);
                spawn_policy_worker(id, Arc::downgrade(&board));
                spawn_view_worker(
                    prefs::ViewKey { item: id, slot },
                    Arc::clone(&board),
                    view_rx,
                );
            } else {
                // Ephemeral: no sinks installed; previous mount's
                // sinks already dropped with its board.
                guard.clear_commit_sink();
                guard.clear_view_sink();
                guard.clear_live_sink();
            }
        }
    });
}

/// Quiet time after the last stroke before the document is saved.
const SAVE_AFTER: Duration = Duration::from_millis(700);
/// Longest a document goes unsaved while strokes keep coming.
const SAVE_AT_MOST: Duration = Duration::from_secs(3);

/// Save the document a while after each burst of commits, on a thread
/// of its own so neither the pen nor the UI waits on the export or the
/// disk. While a save is due the worker holds the board, so closing it
/// right after a stroke still saves that stroke; otherwise it only
/// watches, and exits once the board and its commit sink are gone.
fn spawn_autosave_worker(id: ItemId, board: &Arc<Mutex<Board>>, rx: flume::Receiver<()>) {
    let watched = Arc::downgrade(board);
    let spawned = std::thread::Builder::new()
        .name("notein-autosave".to_owned())
        .spawn(move || {
            while rx.recv().is_ok() {
                let Some(board) = watched.upgrade() else {
                    return;
                };
                SAVING.fetch_add(1, Ordering::AcqRel);
                settle(&rx);
                save(id, &board);
                SAVING.fetch_sub(1, Ordering::AcqRel);
            }
        });
    if let Err(err) = spawned {
        log::error!("autosave thread id={id:?}: {err}");
    }
}

/// Wait until commits stop for [`SAVE_AFTER`], [`SAVE_AT_MOST`] went by
/// since the first one, or [`save_open_documents`] asked.
fn settle(rx: &flume::Receiver<()>) {
    /// How often a waiting save looks whether it was asked to hurry.
    const GLANCE: Duration = Duration::from_millis(100);
    let asked = SAVE_NOW.load(Ordering::Acquire);
    let first = Instant::now();
    let mut last = first;
    while SAVE_NOW.load(Ordering::Acquire) == asked && first.elapsed() < SAVE_AT_MOST {
        match rx.recv_timeout(GLANCE) {
            Ok(()) => last = Instant::now(),
            Err(flume::RecvTimeoutError::Timeout) if last.elapsed() < SAVE_AFTER => {}
            Err(_) => return,
        }
    }
}

/// Bumped to make every waiting canvas save run at once.
static SAVE_NOW: AtomicU64 = AtomicU64::new(0);

/// Canvas saves waiting or running.
static SAVING: AtomicUsize = AtomicUsize::new(0);

/// Save every open canvas with unsaved strokes now, for when the app
/// may go away.
pub fn save_open_documents() {
    SAVE_NOW.fetch_add(1, Ordering::AcqRel);
}

/// [`save_open_documents`], then wait a moment for the saves to land.
pub fn save_open_documents_and_wait() {
    save_open_documents();
    let asked = Instant::now();
    while SAVING.load(Ordering::Acquire) > 0 && asked.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn save(id: ItemId, board: &Arc<Mutex<Board>>) {
    let (snapshot, content) = {
        let guard = lock(board);
        let doc = guard.doc();
        let taken = (
            doc.saved_state(),
            crate::plugins::builtin::canvas::CanvasContent::of(doc),
        );
        drop(guard);
        taken
    };
    let (bytes, captured) = match snapshot {
        Ok(snapshot) => snapshot,
        Err(err) => {
            log::error!("autosave snapshot id={id:?}: {err}");
            return;
        }
    };
    match crate::library::shared_blocking() {
        Ok(handle) => {
            let mut lib = pollster::block_on(handle.lock());
            if let Err(err) = pollster::block_on(lib.save_doc_snapshot(id, &bytes, captured)) {
                log::error!("autosave save_doc id={id:?}: {err}");
            }
        }
        Err(err) => log::error!("autosave open library: {err}"),
    }
    crate::plugins::builtin::canvas::after_save(id, &content, &bytes);
    sync_siblings(id, board);
    crate::sync::doc_changed(id);
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
    let (style, surface) = queued.map_or_else(
        || (BackgroundStyle::default(), DEFAULT_PAPER),
        |p| (p.style, p.surface),
    );
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
        let exported = lock(board).doc().updates_since(&version);
        let updates = match exported {
            Ok(updates) => updates,
            Err(err) => {
                log::error!("sibling sync export id={item:?}: {err}");
                continue;
            }
        };
        let merged = lock(&sibling).merge_updates(&updates);
        if let Err(err) = merged {
            log::error!("sibling sync import id={item:?}: {err}");
        }
    }
}
