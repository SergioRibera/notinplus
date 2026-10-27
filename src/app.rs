//! Freya app shell — brush palette overlay + fullscreen drawing surface.
//!
//! The same `root` component drives desktop (`launch`) and mobile
//! (`#[istmo::mobile_app]`) entry points; only the launcher wrapper
//! differs.
//!
//! Layout is Procreate-shaped: the canvas fills the entire window at
//! `(0, 0)` so pen samples (which arrive in decor-view / fullscreen
//! coordinates from `PenCaptureView`) line up 1:1 with what the freya
//! canvas paints. The palette floats on top as an absolutely-positioned
//! overlay, offset by the safe-area insets published by the platform.

use std::sync::{Arc, Mutex};

use freya::prelude::*;
use freya::router::Outlet;

use istmo::plugins::EdgeInsets;

use crate::route::Route;

use crate::canvas::{
    AnchorSourceRef, Board, InputMode, RedrawNotifier, drawing_surface, lock,
};
use crate::hooks::use_safe_area_insets;
use crate::library::{ItemId, ScratchId, TabTarget, ViewSlot};
use crate::prefs;
use crate::ui_mask::{MaskSlot, use_mask_slot};

/// Which flavour of mount the surrounding [`canvas_root`] call represents.
/// Drives per-mount side effects (doc load, viewport sidecar routing)
/// without forcing every caller to re-implement the hook plumbing.
///
/// Plugin-specific concerns (the PDF background, a reader) live with
/// each kind's face. The shell only knows about generic doc flavours.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CanvasMount {
    /// Persisted canvas item — doc + viewport sidecar load from
    /// [`crate::library`] by id; autosave worker spawned on mount.
    /// `slot` names the pane showing it so two panes of the same item
    /// keep their own viewport and board.
    Item { id: ItemId, slot: Option<ViewSlot> },
    /// Scratch surface. No library item, no autosave, no view sidecar.
    Ephemeral(ScratchId),
}

impl CanvasMount {
    /// Item id to drive autosave + viewport sidecar against, if any.
    pub(crate) const fn item(self) -> Option<ItemId> {
        match self {
            Self::Item { id, .. } => Some(id),
            Self::Ephemeral(_) => None,
        }
    }

    /// The pane showing this mount, if it sits in a mesa.
    pub(crate) const fn slot(self) -> Option<ViewSlot> {
        match self {
            Self::Item { slot, .. } => slot,
            Self::Ephemeral(_) => None,
        }
    }

    /// What a tab showing this mount would name.
    pub(crate) const fn target(self) -> TabTarget {
        match self {
            Self::Item { id, .. } => TabTarget::Item(id),
            Self::Ephemeral(id) => TabTarget::Scratch(id),
        }
    }
}

/// Freya context wrapper carrying the long-press anchor-ref closure
/// published by the enclosing linked-source plugin.
///
/// [`canvas_body`] consumes it with `try_consume_context`; when `None` (the
/// default for plain canvas routes) long-press pins land without a plugin
/// `SourceRef` attribution.
///
/// Clone / `PartialEq` are structural over the `Arc` pointer — two
/// contexts compare equal iff they wrap the same closure allocation.
#[derive(Clone)]
pub struct AnchorSourceCtx {
    pub source: AnchorSourceRef,
    /// What a long press on the page does before it drops a bookmark.
    pub text_press: Option<crate::canvas::TextPressRef>,
}

/// Provided by a face that paints its own pages under the ink (the PDF),
/// so the canvas it mounts does not lay the item's paper over them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FacePaintsBackground;

impl PartialEq for AnchorSourceCtx {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.source, &other.source)
    }
}

impl std::fmt::Debug for AnchorSourceCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnchorSourceCtx").finish_non_exhaustive()
    }
}

/// Debounced writer for global toolbar prefs. One worker per app process
/// — the sink installed in `root()` wakes it on every preset / size /
/// colour / input-mode mutation, and the drain-loop collapses bursts
/// into a single disk write.
fn spawn_prefs_worker(board: Arc<Mutex<Board>>, rx: flume::Receiver<()>) {
    spawn(async move {
        while rx.recv_async().await.is_ok() {
            while rx.try_recv().is_ok() {}
            let snapshot = {
                let g = lock(&board);
                prefs::Prefs {
                    current_preset: Some(*g.current_preset()),
                    current_color: Some(g.current_color()),
                    size_scales: g.size_scales_snapshot(),
                    input_mode_pan: matches!(g.input_mode(), InputMode::Pan),
                }
            };
            if let Err(err) = prefs::save_prefs(&snapshot) {
                log::error!("save_prefs: {err}");
            }
        }
    });
}

pub fn run() {
    #[cfg(target_os = "android")]
    {
        use freya::prelude::NativeEvent;
        use winit::event_loop::EventLoop;
        use winit::platform::android::EventLoopBuilderExtAndroid;

        // Freya-winit's default launch path builds `EventLoop::builder().build()`
        // without threading in the `AndroidApp`, so it panics on
        // Android. Pre-build the event loop ourselves, hand off the app
        // handle that `#[istmo::mobile_app]` stashed, and pass the loop
        // in via `with_event_loop`.
        let Some(app) = istmo::android::android_app() else {
            log::error!("no AndroidApp: #[istmo::mobile_app] must run before app::run");
            return;
        };
        let event_loop = match EventLoop::<NativeEvent>::with_user_event()
            .with_android_app(app)
            .build()
        {
            Ok(event_loop) => event_loop,
            Err(err) => {
                log::error!("android event loop: {err}");
                return;
            }
        };
        install_global_router();
        launch(
            launch_config()
                .with_event_loop(event_loop)
                .with_window(WindowConfig::new(router_root).with_title("notinplus")),
        );
    }

    #[cfg(not(target_os = "android"))]
    {
        install_global_router();
        let cfg = launch_config().with_window(
            WindowConfig::new(router_root)
                .with_title("notinplus")
                .with_window_handle(|window| {
                    // Freya hands us the live winit `Window` exactly once at
                    // creation. Upgrade the symbolic pen registration to a
                    // real handle attach so the Linux Wayland / XInput2
                    // backends can bind to the compositor; macOS / Windows
                    // find their native view through the same call.
                    if let Some(publisher) = crate::desktop::PEN_PUBLISHER.get() {
                        if let Err(err) = publisher.register_window(crate::WINDOW_ID, &*window) {
                            log::warn!("pen register_window failed: {err}");
                        }
                    }
                    // The share sheet anchors on the window on macOS / Windows.
                    if let Err(err) = istmo_window::WindowRegistry::global()
                        .register(istmo_window::WindowId(crate::WINDOW_ID), &*window)
                    {
                        log::warn!("share window registration failed: {err}");
                    }
                }),
        );
        launch(cfg);
        crate::deferred::flush();
        crate::library::index::flush();
        // The window is closed: let the other devices know right away.
        crate::sync::stop();
    }
}

/// The bundled fonts: the canvas hand, and those of every plugin.
fn launch_config() -> LaunchConfig {
    let fonts = crate::plugins::catalog().fonts();
    fonts.into_iter().fold(
        LaunchConfig::new().with_font(
            crate::canvas_text::FONT_FAMILY,
            crate::canvas_text::FONT_BYTES,
        ),
        |config, font| config.with_font(font.family, font.bytes),
    )
}

fn router_root() -> impl IntoElement {
    AppShell
}

// `RouterContext` is `!Send + !Sync` (it holds an `Rc`-backed
// subscriber set + a `RefCell` history), so a cross-thread
// `OnceLock<RouterContext>` is impossible. The runtime creates
// exactly one on the UI thread and every consumer runs on the same
// thread (Freya event handlers + the shared task executor), so a
// thread-local cell is both correct and sufficient.
thread_local! {
    static GLOBAL_ROUTER: std::cell::OnceCell<freya::router::RouterContext> =
        const { std::cell::OnceCell::new() };
}

fn new_router() -> freya::router::RouterContext {
    freya::router::RouterContext::create_global::<crate::route::Route>(
        freya::router::RouterConfig::default().with_initial_path(crate::route::Route::Home),
    )
}

/// Create the router before launch, so every scope shares it.
fn install_global_router() {
    GLOBAL_ROUTER.with(|cell| {
        cell.get_or_init(new_router);
    });
}

/// The app's router.
#[must_use]
pub fn global_router() -> freya::router::RouterContext {
    GLOBAL_ROUTER.with(|cell| *cell.get_or_init(new_router))
}

/// Root layout that measures its own area and hosts the shared
/// [`ModalPortal`] as the last child of every route. Every dialog /
/// sheet / popover the app opens lands here on top of the router.
#[derive(Debug, PartialEq)]
struct AppShell;

impl Component for AppShell {
    fn render(&self) -> impl IntoElement {
        #[cfg(target_os = "android")]
        use_hook(crate::android_clipboard::install);
        let area = use_state(Area::default);
        let settings = crate::settings::use_settings_root();
        crate::state::use_library_root();
        crate::screens::home::use_browse_root();
        crate::plugins::host::use_host_pump();
        use_hook(move || {
            crate::plugins::content::index_library();
            let retention = settings.read().library.trash_retention.millis();
            crate::state::edit("vaciar la papelera", async move |lib| {
                lib.purge_expired(retention).await.map(drop)
            });
        });
        let insets = *crate::hooks::provide_safe_area_root().read();
        // Share the measured window area with any descendant that
        // wants to clamp its own popup/overlay inside the viewport —
        // see `notein_ui_kit::ViewportArea` + `clamp_popup_top`.
        provide_context(notein_ui_kit::ViewportArea(area));
        // Share the global router context at AppShell scope so
        // sibling subtrees (`ModalPortal`, `HostOverlay`) see it via
        // `RouterContext::get()` / `::try_get()`. The actual Router
        // component is replaced by `Outlet` below — it would create
        // its own scope-local context and leave siblings blind.
        use_provide_context(global_router);
        // Drain the font-registration buffer that plugins fill via
        // `Host::load_font` during pre-launch `install_host`. This
        // must run inside the freya scheduler context — `use_hook`
        // runs once per `AppShell` mount, exactly when `Platform::get`
        // becomes safe to call.
        use_hook(crate::sync::start);
        crate::sync::use_presence();
        use_hook(crate::deep_link::listen);
        use_hook(crate::session_end::listen);
        rect()
            .expanded()
            .background(notein_ui_kit::palette().bg)
            .on_sized({
                let mut area = area;
                move |e: Event<SizedEventData>| area.set(e.area)
            })
            // Router context is installed at the window root via
            // `with_root_context`, so every window-scoped descendant
            // (including sibling `HostOverlay`) sees it. We mount the
            // outlet directly instead of wrapping another `Router`
            // component — otherwise the inner `Router` scope creates
            // its own context and siblings outside its subtree would
            // again lose access.
            .child(Outlet::<Route>::new())
            .child({
                // Install the host-side ModalChrome context before the
                // portal materialises its children, so every modal
                // body inherits the shell's safe-area insets + pen-mask
                // hooks. Plugin-opened modals that bypass the shell
                // portal still work with the default (no inset, no
                // mask) chrome.
                crate::components::modal::provide_host_modal_chrome();
                crate::components::ModalPortal::new(area)
            })
            .child(notein_ui_kit::ToastHost::new(insets.bottom))
    }
}

/// Reactive signals the drawing chrome (toolbar, overlays, sidebar)
/// binds to. Mirrors a Board's live state so the chrome can live above
/// a split layout and still drive whichever pane is currently active.
///
/// All fields are freya [`State`]s — cheap to copy into closures. The
/// active board slot is populated by [`canvas_body`] on mount and
/// cleared on unmount; chrome rendering is a no-op while it is `None`
/// so a transient state (route transition, focus switch) does not
/// crash the overlays.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct ChromeState {
    pub(crate) active_board: State<Option<Arc<Mutex<Board>>>>,
    /// Item id backing the active board, if any. Mirrors
    /// `active_board` so overlays that need to address the mount by
    /// library id (e.g. "open the edit sheet for this doc") can
    /// resolve without going through the router — the router points
    /// at a `WorkspaceView` when panes are nested inside one, so its
    /// `canvas_item()` would always be `None` for those mounts.
    pub(crate) active_item: State<Option<ItemId>>,
    pub(crate) selected_bookmark: State<Option<crate::ids::BookmarkId>>,
    pub(crate) bookmark_body: State<String>,
    /// Editor of the canvas text box under edit, shared by the boxes and
    /// the formatting bar the chrome docks.
    pub(crate) text_editor: crate::canvas_text::TextEditorSlot,
    /// Last window area of the canvas text toolbar. Bottom overlays add its
    /// height to their offset while a box is edited so they sit above it.
    pub(crate) text_toolbar_area: State<Area>,
    /// Pen mask entries of this pane's overlays.
    pub(crate) masks: ChromeMasks,
    /// What the ink chrome shows, synced by the redraw pump.
    pub(crate) ink: crate::ink::InkState,
}

/// Pen mask entries owned by one pane's chrome, so split panes do not
/// overwrite each other's (see [`crate::ui_mask`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct ChromeMasks {
    pub(crate) left: MaskSlot,
    pub(crate) palette: MaskSlot,
    pub(crate) zoom: MaskSlot,
    pub(crate) right: MaskSlot,
    pub(crate) bookmark_card: MaskSlot,
}

impl ChromeMasks {
    fn use_masks() -> Self {
        Self {
            left: use_mask_slot(),
            palette: use_mask_slot(),
            zoom: use_mask_slot(),
            right: use_mask_slot(),
            bookmark_card: use_mask_slot(),
        }
    }
}

/// Allocate the chrome signals. Call once per shell that hosts a chrome
/// instance — a route root for single-pane views, a workspace root
/// above multiple panes.
pub(crate) fn use_chrome_state() -> ChromeState {
    ChromeState {
        active_board: use_state(|| Option::<Arc<Mutex<Board>>>::None),
        active_item: use_state(|| Option::<ItemId>::None),
        selected_bookmark: use_state(|| Option::<crate::ids::BookmarkId>::None),
        bookmark_body: use_state(String::new),
        text_editor: crate::canvas_text::TextEditorSlot::use_slot(),
        text_toolbar_area: use_state(Area::default),
        masks: ChromeMasks::use_masks(),
        ink: crate::ink::InkState::use_state(),
    }
}

/// Component wrapper combining [`canvas_body`] and [`canvas_chrome`]
/// into a single pane-local unit. Each instance owns its own
/// [`ChromeState`], so the chrome (toolbar, overlays,
/// bookmark sidebar) is scoped to the pane's body area — not the
/// workspace shell, which would otherwise paint over sibling tab bars.
///
/// Required when the body is mounted inside a dynamic container
/// (workspace pane tree, tabbed shells): the Freya hook runtime demands
/// stable per-component hook order, which only holds when each dynamic
/// child owns its own component instance.
#[derive(Clone, PartialEq)]
pub(crate) struct CanvasPane {
    pub mount: CanvasMount,
}

impl Component for CanvasPane {
    fn render(&self) -> impl IntoElement {
        let state = use_chrome_state();
        use_provide_context(move || state.text_editor.popup_reporter());
        let insets = use_safe_area_insets();
        let pad = *insets.read();
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .content(Content::Flex)
            .child(canvas_body(self.mount, &state))
            .child(canvas_chrome(&state, pad, self.mount.slot()))
    }
}

/// Mount a canvas pane: creates the per-mount [`Board`], wires input,
/// publishes the handle to `state.active_board` so sibling chrome can
/// drive it, and returns the Freya drawing surface. Chrome lives at a
/// parent / workspace level — this fn contributes nothing visible
/// beyond the strokes + background it paints.
///
/// Prefer wrapping call sites inside [`CanvasPane`] when mounting from
/// a dynamic container (pane tree, tab bar); the free fn is kept for
/// single-pane routes that call it exactly once per render.
pub(crate) fn canvas_body(mount: CanvasMount, state: &ChromeState) -> impl IntoElement {
    // When an ancestor hoisted the mount — e.g.
    // `DocLinkedView::render` creates the board at the shell level so
    // both halves of a split-view share it — pick up the provided
    // `Board` from context instead of standing up a fresh one. The
    // reuse path skips every init side-effect (prefs workers,
    // redraw-sync, active_boards registration, `init_canvas_mount`)
    // because the hoisting parent already ran them; we still seed the
    // local `ChromeState` signals so the per-side chrome reflects the
    // shared board's current state.
    let shared_board = freya::prelude::try_consume_context::<Arc<Mutex<Board>>>();
    let is_shared = shared_board.is_some();
    let board = use_hook({
        let shared_board = shared_board;
        let mut state_copy = *state;
        move || {
            if let Some(b) = shared_board {
                seed_chrome_from_board(&b, &mut state_copy, mount);
                return b;
            }
            mount_board(mount, &state_copy)
        }
    });
    use_drop({
        let board = Arc::clone(&board);
        let mut active_setter = state.active_board;
        let mut active_item_setter = state.active_item;
        move || {
            // Per-surface pen-pump deregistration lives inside
            // `drawing_surface`; it owns the surface id and runs its
            // own `use_drop`.
            //
            // Shared mount: lifecycle (active_boards + cancel + the
            // chrome signals) belongs to the hoisting ancestor (see
            // [`crate::doc_linked::DocLinkedView`]'s `use_drop`).
            // Doing it here would clear state the ancestor just
            // re-seeded on a split-toggle remount.
            if is_shared {
                return;
            }
            crate::active_boards::clear_if(mount, &board);
            // Drop any in-flight stroke so a tap mid-navigation does
            // not deliver a trailing Up into the mounted board after
            // its view has already unmounted. An open text edit is kept.
            let mut guard = lock(&board);
            guard.end_text_session();
            guard.cancel();
            drop(guard);
            // Pointer-equality guard: a newer mount may already have
            // claimed the slot during the route transition.
            let same = active_setter
                .peek()
                .as_ref()
                .is_some_and(|cur| Arc::ptr_eq(cur, &board));
            if same {
                active_setter.set(None);
                if *active_item_setter.peek() == mount.item() {
                    active_item_setter.set(None);
                }
            }
        }
    });
    let anchor_ctx = freya::prelude::try_consume_context::<AnchorSourceCtx>();
    let anchor_source = anchor_ctx.as_ref().map(|ctx| Arc::clone(&ctx.source));
    let text_press = anchor_ctx.and_then(|ctx| ctx.text_press);
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .content(Content::Flex)
        .child(drawing_surface(
            &board,
            state.selected_bookmark,
            state.bookmark_body,
            state.text_editor,
            anchor_source,
            text_press,
        ))
}

/// Create a fresh `Board` for `mount`, restore global toolbar prefs,
/// spawn the autosave + prefs + redraw-sync workers, publish the
/// board to `active_boards` + the chrome's `active_board` signal, and
/// install the `Arc<Mutex<Board>>` as a context value so descendants
/// can look it up. Returns the `Arc` for the caller to hold.
///
/// Separated from [`canvas_body`] so shells that need to hoist the
/// mount above a split-view layout (see
/// [`crate::doc_linked::DocLinkedView`]) can run the init at a
/// container scope and have inner canvas calls reuse the published
/// board via `try_consume_context`.
///
/// Must be called inside a component render — spawn + `provide_context`
/// resolve against the current scope.
pub(crate) fn mount_board(mount: CanvasMount, state: &ChromeState) -> Arc<Mutex<Board>> {
    let mut active_setter = state.active_board;
    let mut active_item_setter = state.active_item;
    let ink = state.ink;

    let board = Arc::new(Mutex::new(Board::default()));
    let window = Platform::get().current_window();
    let (notifier, rx) = RedrawNotifier::new();
    let (prefs_tx, prefs_rx) = flume::unbounded::<()>();
    {
        let mut g = lock(&board);
        g.set_notifier(notifier);
        // Restore global toolbar prefs before any UI reads
        // `current_preset`. Sink is installed after the apply so the
        // restore itself does not re-ping and churn disk at launch.
        let saved = prefs::load_prefs();
        for (kind, scale) in &saved.size_scales {
            g.set_size_scale(*kind, *scale);
        }
        if let Some(preset) = saved.current_preset {
            g.set_current_preset(preset);
        }
        if let Some(color) = saved.current_color {
            g.set_current_color(color);
        }
        g.set_input_mode(saved.input_mode());
        g.set_prefs_sink(prefs_tx);
        let handwriting = prefs::load_handwriting_prefs();
        handwriting.font.select();
        g.set_handwriting(handwriting.enabled);
    }
    spawn_handwriting_timer(&board);
    // Seed chrome signals from the restored board state. Done after
    // the prefs apply so overlays open on the correct preset / size
    // / mode on first paint.
    {
        ink.sync(&lock(&board));
    }
    spawn_prefs_worker(Arc::clone(&board), prefs_rx);
    let sync_board = Arc::clone(&board);
    spawn(async move {
        while rx.recv_async().await.is_ok() {
            window.request_redraw();
            ink.sync(&lock(&sync_board));
        }
    });
    // Provide to descendants (settings modal, split-view sibling
    // panes, …) so they can consume via `try_consume_context::
    // <Arc<Mutex<Board>>>()` and operate on this mount's board.
    provide_context(Arc::clone(&board));
    crate::active_boards::set(mount, Arc::clone(&board));
    crate::route::init_canvas_mount(mount, &board);
    // Publish for the sibling chrome. Any previously-published board
    // (a prior mount of the same shell) is overwritten — last-mount
    // -wins matches the pen pump semantics.
    active_setter.set(Some(Arc::clone(&board)));
    active_item_setter.set(mount.item());
    board
}

/// Close a handwritten letter once the pen rests. The board sends the
/// pending letter's generation after every stroke; a newer one arriving
/// during the wait restarts it. Exits when the board (and its sender)
/// drops.
fn spawn_handwriting_timer(board: &Arc<Mutex<Board>>) {
    let (tx, rx) = flume::unbounded::<u64>();
    lock(board).set_handwriting_waker(tx);
    let board = Arc::downgrade(board);
    spawn(async move {
        while let Ok(mut generation) = rx.recv_async().await {
            loop {
                async_io::Timer::after(crate::canvas::LETTER_IDLE).await;
                match rx.try_recv() {
                    Ok(newer) => generation = newer,
                    Err(_) => break,
                }
            }
            let Some(board) = board.upgrade() else {
                return;
            };
            lock(&board).flush_handwriting(generation);
        }
    });
}

/// Reuse-path helper: seed a fresh `ChromeState`'s signals from an
/// existing `Board` published by [`mount_board`] at an ancestor
/// scope. Mirror of the signal-seeding block inside `mount_board` —
/// kept separate so the fresh init path never pays the extra
/// zoom-signal write when the chrome state is one it already seeded.
fn seed_chrome_from_board(board: &Arc<Mutex<Board>>, state: &mut ChromeState, mount: CanvasMount) {
    state.ink.sync(&lock(board));
    state.active_board.set(Some(Arc::clone(board)));
    state.active_item.set(mount.item());
}

pub(crate) fn canvas_chrome(
    state: &ChromeState,
    pad: EdgeInsets,
    slot: Option<ViewSlot>,
) -> impl IntoElement {
    // Per-pane area probe. Descendants (bookmark card overlay, popups)
    // consume `PaneArea` to clamp against the pane's own bounds instead
    // of the full window.
    let pane_area_signal = use_state(Area::default);
    provide_context(notein_ui_kit::PaneArea(pane_area_signal));
    let Some(board) = state.active_board.read().clone() else {
        return rect();
    };
    let lift = pad.bottom
        + if state.text_editor.editing.read().is_some() {
            state.text_toolbar_area.read().height()
        } else {
            0.0
        };
    rect()
        .width(Size::percent(100.))
        .height(Size::percent(100.))
        .layer(Layer::Overlay)
        .position(Position::new_absolute().top(0.0).left(0.0))
        .on_sized({
            let mut setter = pane_area_signal;
            move |e: Event<SizedEventData>| {
                if *setter.peek() != e.area {
                    setter.set(e.area);
                }
            }
        })
        .child(crate::ink::Chrome {
            board: Arc::clone(&board),
            state: state.ink,
            dock_mask: state.masks.palette,
            zoom_mask: state.masks.zoom,
            bar_mask: state.masks.left,
            lift,
            slot,
        })
        .child(crate::ink::PinNotes {
            board: Arc::clone(&board),
            selected: state.selected_bookmark,
            origin: {
                let area = *pane_area_signal.read();
                (area.min_x(), area.min_y())
            },
            item: *state.active_item.read(),
        })
        .child(crate::canvas_text::text_toolbar(
            &board,
            state.text_editor,
            state.text_toolbar_area,
            pad,
        ))
}

