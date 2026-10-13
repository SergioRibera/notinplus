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
use std::time::Duration;

use freya::animation::{AnimNum, Ease, Function, use_animation_transition};
use freya::icons::lucide::{
    arrow_left, hand, layers, panel_left, panel_left_close, pencil, redo, sliders_horizontal, undo,
};
use freya::prelude::*;
use freya::router::*;
use istmo::plugins::EdgeInsets;

use crate::components::SidebarModal;
use crate::components::theme::{
    BALL_PEN, BALL_PEN_FILL, BRUSH_PEN, BRUSH_PEN_FILL, ERASER, FOUNTAIN_PEN, FOUNTAIN_PEN_FILL,
    HIGHLIGHTER, HIGHLIGHTER_FILL, PENCIL, PENCIL_FILL,
};
use crate::route::Route;

use crate::bookmark::SourceRef;
use crate::brush::{BrushKind, BrushPreset, ShapeMode};
use crate::canvas::{
    AnchorSourceRef, Board, InputMode, LayerSnapshot, RedrawNotifier, drawing_surface, lock,
};
use crate::hooks::use_safe_area_insets;
use crate::library::ItemId;
use crate::palette_popup::{HOVER_DELAY, brush_popup};
use crate::pdf_viewer::{PDF_PLUGIN_ID, PdfAnchor, encode_pdf_anchor};
use crate::pen_pump;
use crate::plugins;
use crate::prefs;
use crate::ui_mask::{self, UiRegion};

/// Which flavour of mount the surrounding [`canvas_root`] call represents.
/// Drives per-mount side effects (doc load, PDF background) without
/// forcing every caller to re-implement the hook plumbing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CanvasMount {
    /// Persisted canvas item — doc + viewport sidecar load from
    /// [`crate::library`] by id; autosave worker spawned on mount.
    Item(ItemId),
    /// Scratch surface. No library item, no autosave, no view sidecar.
    Ephemeral,
    /// Canvas overlaid on a PDF. Strokes persist against `id`; the PDF
    /// blob is loaded off the UI thread and installed as background.
    PdfBacked(ItemId),
}

/// Build the anchor-ref closure drawing_surface uses on long-press.
/// Only [`CanvasMount::PdfBacked`] produces a non-None closure — PDF
/// pins carry a [`SourceRef::Plugin`] back to the source item + page
/// so chip strips and cross-pane navigation can resolve the target.
fn pdf_anchor_source_for(mount: CanvasMount) -> Option<AnchorSourceRef> {
    let CanvasMount::PdfBacked(item) = mount else {
        return None;
    };
    Some(std::sync::Arc::new(move |att| {
        let anchor = encode_pdf_anchor(PdfAnchor {
            source: item,
            page: att.page.0 as u32,
        });
        SourceRef::Plugin {
            plugin_id: PDF_PLUGIN_ID.to_owned(),
            item: item.0,
            anchor,
            selection: None,
        }
    }))
}

impl CanvasMount {
    /// Item id to drive autosave + viewport sidecar against, if any.
    pub(crate) const fn item(self) -> Option<ItemId> {
        match self {
            Self::Item(id) | Self::PdfBacked(id) => Some(id),
            Self::Ephemeral => None,
        }
    }
}

/// Translate a Freya `on_sized` event area into the surface-pixel
/// tuple [`crate::ui_mask`] expects. Skips writing when the resolved
/// area collapses to zero — a still-laying-out element publishing an
/// empty rect would leave a phantom hole in the mask.
fn publish_mask(region: UiRegion, area: Area) {
    let w = area.max_x() - area.min_x();
    let h = area.max_y() - area.min_y();
    if w <= 0.0 || h <= 0.0 {
        ui_mask::set(region, None);
        return;
    }
    ui_mask::set(
        region,
        Some((area.min_x(), area.min_y(), area.max_x(), area.max_y())),
    );
}

/// Bundle of reactive signals every palette button shares to
/// coordinate the popup overlay. Grouped so [`palette_button`] does
/// not spread a wide parameter list across every caller.
#[derive(Clone, Copy)]
pub(crate) struct PopupCoords {
    open_idx: State<Option<usize>>,
    hovered_idx: State<Option<usize>>,
    button_areas: State<Vec<Option<Area>>>,
    /// Latest pen-hover surface position from
    /// [`crate::pen_pump::hover_receiver`], or `None` when the tool is
    /// out of proximity. `palette_overlay` translates this into a
    /// synthetic hover target so the popup opens under a stylus that
    /// never touches the screen.
    pen_hover: State<Option<pen_pump::HoverPoint>>,
    /// Palette index the pen last hovered over. Used to detect
    /// transitions so timers / leave events fire exactly once per
    /// boundary crossing.
    pen_last_target: State<Option<usize>>,
    /// Last colour the user picked from the brush popup's advanced
    /// picker. Lives up here (vs. inside the popup) because
    /// `brush_popup` is only mounted while the popup is open — hooks
    /// declared there would thrash on open/close cycles.
    color_special: State<Option<Color>>,
    /// Whether the advanced picker dropdown under the special swatch
    /// is currently expanded. Same lifting rationale as
    /// `color_special`.
    color_picker_open: State<bool>,
    /// Set while the pointer sits inside the opened `brush_popup` body.
    /// Lets the close-timer kicked off by `on_pointer_leave` on a
    /// palette button distinguish "pointer left the palette entirely"
    /// (close) from "pointer crossed the gap into the popup" (keep
    /// open).
    popup_hovered: State<bool>,
    /// Whether the palette row is expanded. `false` renders only the
    /// currently-selected brush as a single compact button; clicking
    /// it flips to `true` and reveals every other brush side-by-side.
    /// A click on the already-selected brush in the expanded state
    /// collapses back to compact. The same close-timer that dismisses
    /// the hover popup also drives idle-collapse so the palette falls
    /// back to compact once the pointer leaves the row + popup for
    /// `POPUP_CLOSE_DELAY`.
    expanded: State<bool>,
}

const PALETTE: &[fn() -> BrushPreset] = &[
    BrushPreset::pencil,
    BrushPreset::marker,
    BrushPreset::pen,
    BrushPreset::highlighter,
    BrushPreset::eraser,
    default_shape,
];
const PALETTE_ICONS: [(Bytes, Bytes); 6] = [
    (PENCIL, PENCIL_FILL),
    (BALL_PEN, BALL_PEN_FILL),
    (BRUSH_PEN, BRUSH_PEN_FILL),
    (HIGHLIGHTER, HIGHLIGHTER_FILL),
    (ERASER, ERASER),
    (FOUNTAIN_PEN, FOUNTAIN_PEN_FILL),
];

const fn default_shape() -> BrushPreset {
    BrushPreset::shape(ShapeMode::Line)
}

/// Which palette slot (if any) matches `kind`. `BrushKind::Shape(_)`
/// collapses onto the single shape slot regardless of mode so the UI
/// still highlights the shape tool when the user left the session on
/// e.g. an arrow.
fn palette_index_for(kind: BrushKind) -> Option<usize> {
    PALETTE.iter().position(|make| match (make().kind, kind) {
        (BrushKind::Shape(_), BrushKind::Shape(_)) => true,
        (a, b) => a == b,
    })
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
                let preset = *g.current_preset();
                let color = g.current_color();
                let mode = g.input_mode();
                let scales: Vec<(BrushKind, f32)> = g.size_scales_snapshot();
                prefs::Prefs {
                    current_preset: Some(preset),
                    current_color: Some(color),
                    size_scales: scales,
                    input_mode_pan: matches!(mode, InputMode::Pan),
                }
            };
            if let Err(err) = prefs::save_prefs(&snapshot) {
                log::error!("save_prefs: {err}");
            }
        }
    });
}

pub fn run() {
    plugins::hydrate_from_disk();

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
        let app = istmo::android::android_app()
            .expect("AndroidApp not set; #[istmo::mobile_app] must run before app::run_mobile");
        let event_loop = EventLoop::<NativeEvent>::with_user_event()
            .with_android_app(app)
            .build()
            .expect("failed to build Android event loop");
        launch(
            LaunchConfig::new()
                .with_event_loop(event_loop)
                .with_window(WindowConfig::new(router).with_title("notinplus")),
        );
    }

    #[cfg(not(target_os = "android"))]
    launch(
        LaunchConfig::new().with_window(
            WindowConfig::new(router)
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
                }),
        ),
    );
}

fn router() -> impl IntoElement {
    AppShell
}

/// Root layout that measures its own area and hosts the shared
/// [`ModalPortal`] as the last child of every route. Every dialog /
/// sheet / popover the app opens lands here on top of the router.
#[derive(Debug, PartialEq)]
struct AppShell;

impl Component for AppShell {
    fn render(&self) -> impl IntoElement {
        let area = use_state(Area::default);
        rect()
            .expanded()
            .on_sized({
                let mut area = area;
                move |e: Event<SizedEventData>| area.set(e.area)
            })
            .child(Router::<Route>::new(|| {
                RouterConfig::default().with_initial_path(Route::Home)
            }))
            .child(crate::components::ModalPortal::new(area))
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
    pub(crate) zoom: State<f32>,
    /// Mirrors `Board::undo_depth() > 0`. Resynced from the redraw
    /// pump (every doc-mutating commit pings it) so the toolbar's
    /// Undo chip disables when the stack empties.
    pub(crate) can_undo: State<bool>,
    /// Mirrors `Board::redo_depth() > 0`. See `can_undo`.
    pub(crate) can_redo: State<bool>,
    pub(crate) selected: State<usize>,
    pub(crate) scale: State<f32>,
    pub(crate) layers_ver: State<u32>,
    /// Whether the floating layers panel is currently rendered. The
    /// top-right toolbar flips this and the overlay collapses when
    /// `false`; nothing else is a mounted hook so toggling is cheap.
    pub(crate) layers_visible: State<bool>,
    pub(crate) mode: State<InputMode>,
    pub(crate) selected_bookmark: State<Option<crate::ids::BookmarkId>>,
    pub(crate) bookmark_body: State<String>,
    pub(crate) sidebar_open: State<bool>,
    pub(crate) sidebar_query: State<String>,
    pub(crate) sidebar_sort: State<crate::bookmark_ui::BookmarkSort>,
}

/// Allocate the chrome signals. Call once per shell that hosts a chrome
/// instance — a route root for single-pane views, a workspace root
/// above multiple panes.
pub(crate) fn use_chrome_state() -> ChromeState {
    ChromeState {
        active_board: use_state(|| Option::<Arc<Mutex<Board>>>::None),
        active_item: use_state(|| Option::<ItemId>::None),
        zoom: use_state(|| 1.0_f32),
        can_undo: use_state(|| false),
        can_redo: use_state(|| false),
        selected: use_state(|| 0_usize),
        scale: use_state(|| 1.0_f32),
        layers_ver: use_state(|| 0_u32),
        layers_visible: use_state(|| false),
        mode: use_state(|| InputMode::Draw),
        selected_bookmark: use_state(|| Option::<crate::ids::BookmarkId>::None),
        bookmark_body: use_state(String::new),
        sidebar_open: use_state(|| false),
        sidebar_query: use_state(String::new),
        sidebar_sort: use_state(|| crate::bookmark_ui::BookmarkSort::Recent),
    }
}

/// Component wrapper combining [`canvas_body`] and [`canvas_chrome`]
/// into a single pane-local unit. Each instance owns its own
/// [`ChromeState`] + [`PopupCoords`], so the chrome (toolbar, overlays,
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
        let coords = use_chrome_coords();
        let insets = use_safe_area_insets();
        let pad = *insets.read();
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .content(Content::Flex)
            .child(canvas_body(self.mount, state))
            .child(canvas_chrome(state, coords, pad))
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
pub(crate) fn canvas_body(mount: CanvasMount, state: ChromeState) -> impl IntoElement {
    let mut zoom = state.zoom;
    let mut can_undo = state.can_undo;
    let mut can_redo = state.can_redo;
    let mut selected_setter = state.selected;
    let mut scale_setter = state.scale;
    let mut mode_setter = state.mode;
    let mut active_setter = state.active_board;
    let mut active_item_setter = state.active_item;
    let board = use_hook(move || {
        let board = Arc::new(Mutex::new(Board::default()));
        let platform = Platform::get();
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
        }
        // Seed chrome signals from the restored board state. Done
        // after the prefs apply so overlays open on the correct
        // preset / size / mode on first paint.
        {
            let g = lock(&board);
            selected_setter.set(palette_index_for(g.current_kind()).unwrap_or(0));
            scale_setter.set(g.current_size());
            mode_setter.set(g.input_mode());
            can_undo.set(g.undo_depth() > 0);
            can_redo.set(g.redo_depth() > 0);
        }
        spawn_prefs_worker(Arc::clone(&board), prefs_rx);
        let zoom_board = Arc::clone(&board);
        spawn(async move {
            while rx.recv_async().await.is_ok() {
                platform.send(UserEvent::RequestRedraw);
                // Piggyback the redraw wakeup: sync the reactive zoom
                // signal off the freshest viewport so the HUD label
                // stays in step with pinch / wheel gestures without
                // needing its own notifier plumbing.
                let (s, u, r) = {
                    let g = lock(&zoom_board);
                    (g.viewport().scale, g.undo_depth() > 0, g.redo_depth() > 0)
                };
                // 0.1% threshold — the HUD label rounds to whole
                // percent, so a stricter epsilon would push zoom.set
                // (and the reactive root re-render it triggers) far
                // more often than the label actually changes.
                if (s - *zoom.read()).abs() > 0.001 {
                    zoom.set(s);
                }
                if u != *can_undo.read() {
                    can_undo.set(u);
                }
                if r != *can_redo.read() {
                    can_redo.set(r);
                }
            }
        });
        // Provide to descendants (settings modal, etc.) so they can
        // consume via `try_consume_context::<Arc<Mutex<Board>>>()` and
        // operate on this mount's board — not a sibling pane's.
        provide_context(Arc::clone(&board));
        // Register with the pen pump so stylus samples route to this
        // board. If a sibling pane later gets focus, its own
        // `canvas_body` call overwrites `ACTIVE_BOARD` and this one
        // falls silent until it regains focus.
        pen_pump::set_active_board(&board);
        if let Some(id) = mount.item() {
            crate::active_boards::set(id, Arc::clone(&board));
        }
        crate::route::init_canvas_mount(mount, &board);
        // Publish for the sibling chrome. Any previously-published
        // board (a prior mount of the same shell) is overwritten —
        // last-mount-wins matches the pen pump semantics.
        active_setter.set(Some(Arc::clone(&board)));
        active_item_setter.set(mount.item());
        board
    });
    use_drop({
        let board = Arc::clone(&board);
        let mut active_setter = state.active_board;
        let mut active_item_setter = state.active_item;
        move || {
            pen_pump::clear_active_board_if(&board);
            if let Some(id) = mount.item() {
                crate::active_boards::clear_if(id, &board);
            }
            // Drop any in-flight stroke so a tap mid-navigation does
            // not deliver a trailing Up into the mounted board after
            // its view has already unmounted.
            lock(&board).cancel();
            // Pointer-equality guard: a newer mount may already have
            // claimed the slot during the route transition.
            let same = active_setter
                .peek()
                .as_ref()
                .map_or(false, |cur| Arc::ptr_eq(cur, &board));
            if same {
                active_setter.set(None);
                if *active_item_setter.peek() == mount.item() {
                    active_item_setter.set(None);
                }
            }
        }
    });
    let anchor_source = pdf_anchor_source_for(mount);
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .child(drawing_surface(
            &board,
            state.selected_bookmark,
            state.bookmark_body,
            anchor_source,
        ))
}

/// Toolbar + overlays stacked above the active pane. Bound to the
/// board currently sitting in `state.active_board`; renders an empty
/// pass-through until a canvas publishes one.
/// Allocate the palette popup signals. Must be called once per render
/// at the top of the owning component — the chrome's rendering body
/// takes the result in as a parameter so [`canvas_chrome`] stays
/// hook-free and safe to call conditionally.
pub(crate) fn use_chrome_coords() -> PopupCoords {
    PopupCoords {
        open_idx: use_state(|| Option::<usize>::None),
        hovered_idx: use_state(|| Option::<usize>::None),
        button_areas: use_state(|| vec![None::<Area>; PALETTE.len()]),
        pen_hover: {
            let mut cell = use_state(|| Option::<pen_pump::HoverPoint>::None);
            use_hook(move || {
                let rx = pen_pump::hover_receiver();
                spawn(async move {
                    while let Ok(sample) = rx.recv_async().await {
                        cell.set(sample);
                    }
                });
            });
            cell
        },
        pen_last_target: use_state(|| Option::<usize>::None),
        color_special: use_state(|| Option::<Color>::None),
        color_picker_open: use_state(|| false),
        popup_hovered: use_state(|| false),
        expanded: use_state(|| false),
    }
}

pub(crate) fn canvas_chrome(
    state: ChromeState,
    coords: PopupCoords,
    pad: EdgeInsets,
) -> impl IntoElement {
    // Hook runs unconditionally so stable hook order holds across the
    // early-return branch below. Drives the compact↔expanded animation
    // of the palette row — non-selected brush buttons read the
    // progress and modulate their width + opacity accordingly.
    let palette_anim = use_animation_transition(coords.expanded, |from, to| {
        let f = if from { 1.0_f32 } else { 0.0_f32 };
        let t = if to { 1.0_f32 } else { 0.0_f32 };
        AnimNum::new(f, t)
            .time(220)
            .ease(Ease::Out)
            .function(Function::Cubic)
    });
    let palette_progress = palette_anim.get().value();
    let Some(board) = state.active_board.read().clone() else {
        // No pane mounted yet — zero-sized placeholder so parent
        // layout slot does not jump when the pane arrives.
        return rect();
    };
    // Full-viewport float: `Position::new_absolute` with top/left/
    // right/bottom 0 anchors a container that covers the pane without
    // participating in flex layout. Children carry their own absolute
    // offsets referenced to this container, so overlay corners
    // (top-left back pill, bottom-right zoom HUD, etc.) land on the
    // real viewport corners — not on a collapsed flex slot. No
    // pointer handlers attached so events fall through to the canvas
    // body behind.
    rect()
        .width(Size::percent(100.))
        .height(Size::percent(100.))
        .layer(Layer::Overlay)
        .position(
            Position::new_absolute()
                .top(0.0)
                .left(0.0)
                .right(0.0)
                .bottom(0.0),
        )
        .child(top_right_stack(&board, &state, pad))
        .child(left_top(&board, &state, coords, pad, palette_progress))
        .child(zoom_overlay(&board, state.zoom, pad))
        .child(crate::bookmark_ui::bookmark_card_overlay(
            &board,
            state.selected_bookmark,
            state.bookmark_body,
        ))
}

pub(crate) fn canvas_root(mount: CanvasMount) -> impl IntoElement {
    CanvasPane { mount }
}

fn left_top(
    board: &Arc<Mutex<Board>>,
    state: &ChromeState,
    coords: PopupCoords,
    pad: EdgeInsets,
    palette_progress: f32,
) -> impl IntoElement {
    let mut current_input_mode = state.mode;
    let input_mode_icon = match *current_input_mode.read() {
        InputMode::Draw => pencil(),
        InputMode::Pan => hand(),
    };

    rect()
        .position(
            Position::new_absolute()
                .top(pad.top + 8.0)
                .left(pad.left + 8.0),
        )
        .vertical()
        .layer(Layer::Overlay)
        .spacing(6.)
        .on_sized(move |e: Event<SizedEventData>| publish_mask(UiRegion::Back, e.area))
        .child(
            rect()
                .spacing(6.)
                .horizontal()
                .child(icon_button(arrow_left(), {
                    let board = Arc::clone(board);
                    move || {
                        let mut doc_snapshot = {
                            let guard = lock(&board);
                            guard.doc().clone()
                        };
                        let item_id = RouterContext::get()
                            .current::<crate::route::Route>()
                            .canvas_item();
                        spawn(async move {
                            if let Some(id) = item_id {
                                match crate::home::open_library().await {
                                    Ok(handle) => {
                                        let mut lib = handle.lock().await;
                                        if let Err(err) = lib.save_doc(id, &mut doc_snapshot).await
                                        {
                                            log::error!("save_doc: {err}");
                                        }
                                    }
                                    Err(err) => log::error!("open library on back: {err}"),
                                }
                            }
                            let _ = RouterContext::get().push(crate::route::Route::Home);
                        });
                    }
                }))
                .child(icon_button(panel_left(), {
                    let board = Arc::clone(board);
                    let state = state.clone();
                    move || {
                        SidebarModal::open(pad, state, board.clone());
                    }
                }))
                .child(icon_button(input_mode_icon, {
                    let press_board = Arc::clone(board);
                    move || {
                        let next = match *current_input_mode.read() {
                            InputMode::Draw => InputMode::Pan,
                            InputMode::Pan => InputMode::Draw,
                        };
                        lock(&press_board).set_input_mode(next);
                        current_input_mode.set(next);
                    }
                }))
                .child(icon_button_gated(undo(), *state.can_undo.read(), {
                    let board = Arc::clone(board);
                    move || {
                        lock(&board).undo();
                    }
                }))
                .child(icon_button_gated(redo(), *state.can_redo.read(), {
                    let board = Arc::clone(board);
                    move || {
                        lock(&board).redo();
                    }
                })),
        )
        .child(palette_overlay(
            board,
            state.selected,
            state.scale,
            coords,
            palette_progress,
        ))
}

fn icon_button(icon: Bytes, cb: impl Into<NoArgCallback<()>>) -> impl IntoElement {
    icon_button_gated(icon, true, cb)
}

/// Like `icon_button` but with an `enabled` gate. Dim glyph + press is
/// a no-op when `enabled == false`; the chip still participates in the
/// UI mask so taps don't bleed through to the canvas underneath.
fn icon_button_gated(
    icon: Bytes,
    enabled: bool,
    cb: impl Into<NoArgCallback<()>>,
) -> impl IntoElement {
    let cb = cb.into();
    let (bg, glyph) = if enabled {
        (Color::from_rgb(30, 30, 34), Color::WHITE)
    } else {
        (Color::from_rgb(18, 18, 20), Color::from_rgb(70, 70, 76))
    };
    rect()
        .center()
        .width(Size::px(30.))
        .height(Size::px(30.))
        .padding((6.0, 10.0))
        .layer(Layer::Overlay)
        .background(bg)
        .with_corner_radius(8.0)
        .on_sized(move |e: Event<SizedEventData>| publish_mask(UiRegion::Back, e.area))
        .on_press(move |_| {
            if enabled {
                cb.call();
            }
        })
        .child(
            SvgViewer::new(icon)
                .color(glyph)
                .width(Size::px(13.))
                .height(Size::px(13.)),
        )
}

fn zoom_overlay(board: &Arc<Mutex<Board>>, zoom: State<f32>, pad: EdgeInsets) -> impl IntoElement {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let pct = (*zoom.read() * 100.0).round() as i32;
    let text = format!("{pct}%");

    let reset_board = Arc::clone(board);
    let reset = rect()
        .padding((4.0, 8.0))
        .background(Color::from_rgb(70, 70, 78))
        .with_corner_radius(4.0)
        .on_press(move |_| {
            lock(&reset_board).reset_viewport();
        })
        .child(label().color(Color::WHITE).font_size(12.0).text("Reset"));

    rect()
        .position(
            Position::new_absolute()
                .bottom(pad.bottom + 8.0)
                .right(pad.right + 8.0),
        )
        .horizontal()
        .center()
        .spacing(6.0)
        .padding((6.0, 8.0))
        .layer(Layer::Overlay)
        .background(Color::from_rgb(30, 30, 34))
        .with_corner_radius(8.0)
        .on_sized(move |e: Event<SizedEventData>| publish_mask(UiRegion::Zoom, e.area))
        .child(label().color(Color::WHITE).font_size(12.0).text(text))
        .child(reset)
}

fn palette_overlay(
    board: &Arc<Mutex<Board>>,
    selected: State<usize>,
    scale: State<f32>,
    coords: PopupCoords,
    progress: f32,
) -> impl IntoElement {
    dispatch_pen_hover(board, selected, scale, coords);
    let selected_idx = (*selected.read()).min(PALETTE.len() - 1);
    // Global-positioned so the palette floats above the fullscreen
    // canvas at `(pad.left, pad.top)` — no room reserved by the parent's
    // flow layout, which is what lets the canvas sit under the status
    // bar / notch on Android and iOS.
    let mut row = rect()
        .horizontal()
        .spacing((8.0 * progress).max(0.0))
        .padding(10.0)
        .background(Color::from_rgb(30, 30, 34))
        .with_corner_radius(10.0)
        .on_sized(move |e: Event<SizedEventData>| publish_mask(UiRegion::Palette, e.area));

    // Always render every brush. The selected one stays at full size;
    // the rest scale their width + opacity by `progress` so the row
    // animates smoothly between compact (progress = 0, only selected
    // visible) and expanded (progress = 1, all brushes side-by-side).
    // Pointer handlers stay installed on every button regardless of
    // progress — a zero-width button has no hit area so stray hovers
    // during the fade cannot swap selection.
    for (idx, make) in PALETTE.iter().enumerate() {
        let is_selected = idx == selected_idx;
        row = row.child(palette_button(
            idx,
            make(),
            board,
            selected,
            scale,
            coords,
            if is_selected { 1.0 } else { progress },
        ));
    }

    // Anchor popup at the currently-open button. Rendered as a
    // sibling so the popup escapes the row's horizontal layout and
    // sits beneath the correct button in global coords.
    let popup_idx = *coords.open_idx.read();
    if let Some(idx) = popup_idx
        && let Some(area) = coords.button_areas.read().get(idx).copied().flatten()
    {
        row = row.child(brush_popup(
            board,
            coords.open_idx,
            Some(area),
            idx,
            PALETTE[idx](),
            coords.color_special,
            coords.color_picker_open,
            coords.popup_hovered,
            coords.hovered_idx,
            coords.expanded,
        ));
    } else {
        // Popup hidden — clear the mask slot so a stale rect from the
        // previous popup does not keep swallowing pen input.
        ui_mask::set(UiRegion::Popup, None);
    }
    row
}

fn size_control(board: &Arc<Mutex<Board>>, mut scale: State<f32>) -> impl IntoElement {
    let cur = *scale.read();
    let label_text = format!("{cur:.2}\u{00d7}");

    let dec_board = Arc::clone(board);
    let inc_board = Arc::clone(board);

    let minus = rect()
        .padding((4.0, 8.0))
        .background(Color::from_rgb(70, 70, 78))
        .with_corner_radius(4.0)
        .on_press(move |_| {
            let next = (*scale.read() - 0.25).max(0.25);
            *scale.write() = next;
            lock(&dec_board).set_current_size(next);
        })
        .child(label().color(Color::WHITE).font_size(14.0).text("\u{2212}"));

    let plus = rect()
        .padding((4.0, 8.0))
        .background(Color::from_rgb(70, 70, 78))
        .with_corner_radius(4.0)
        .on_press(move |_| {
            let next = (*scale.read() + 0.25).min(4.0);
            *scale.write() = next;
            lock(&inc_board).set_current_size(next);
        })
        .child(label().color(Color::WHITE).font_size(14.0).text("+"));

    rect()
        .horizontal()
        .spacing(4.0)
        .padding((8.0, 6.0))
        .background(Color::from_rgb(50, 50, 55))
        .with_corner_radius(6.0)
        .child(minus)
        .child(label().color(Color::WHITE).font_size(13.0).text(label_text))
        .child(plus)
}

fn palette_button(
    idx: usize,
    preset: BrushPreset,
    board: &Arc<Mutex<Board>>,
    mut selected: State<usize>,
    mut scale: State<f32>,
    coords: PopupCoords,
    reveal: f32,
) -> impl IntoElement {
    let mut open_idx = coords.open_idx;
    let mut hovered_idx = coords.hovered_idx;
    let mut button_areas = coords.button_areas;
    let is_active = *selected.read() == idx;
    let _subscribe_color = *coords.color_special.read();
    let swatch_rgba = if is_active && preset.palette_color().is_some() {
        Some(lock(board).current_color())
    } else {
        preset.palette_color()
    };
    let swatch_color = swatch_rgba.map_or(Color::from_rgb(220, 220, 220), |[r, g, b, _]| {
        Color::from_rgb(r, g, b)
    });

    // Commit the currently-selected preset to the board every render —
    // cheap, and keeps the board's active brush in lock-step with the
    // palette without needing a dedicated effect.
    if is_active {
        commit_preset(board, preset);
    }

    let bg = if is_active {
        Color::from_rgb(70, 70, 78)
    } else {
        Color::from_rgb(50, 50, 55)
    };

    let press_board = Arc::clone(board);
    let enter_board = Arc::clone(board);
    let mut press_expanded = coords.expanded;
    let mut press_open_idx = open_idx;
    let mut leave_hovered_idx = hovered_idx;
    let leave_open_idx = open_idx;
    let leave_popup_hovered = coords.popup_hovered;
    let leave_expanded = coords.expanded;
    let (icon, fill_icon) = PALETTE_ICONS[idx].clone();

    // `reveal` goes from 0 (hidden sibling in compact mode) to 1
    // (fully visible). The selected button is always rendered with
    // reveal = 1 regardless of mode, so the row collapses to a single
    // brush while the others fade their width + opacity on and off.
    let clamp = reveal.clamp(0.0, 1.0);
    let btn_width = (45.0_f32 * clamp).max(0.0);
    let opacity = clamp;
    rect()
        .center()
        .width(Size::px(btn_width))
        .height(Size::px(45.))
        .padding(10.)
        .opacity(opacity)
        .background(bg)
        .with_corner_radius(6.0)
        .on_sized(move |e: Event<SizedEventData>| {
            // Publish the layout-resolved rect so `palette_overlay`
            // can anchor the popup beneath this specific button. The
            // event fires every layout pass — cheap to write through
            // an equality guard on the slot below.
            let area = e.area;
            let mut slots = button_areas.write();
            if slots.get(idx).copied().flatten() != Some(area) {
                slots[idx] = Some(area);
            }
        })
        .on_pointer_enter(move |_| {
            hovered_idx.set(Some(idx));
            // Popup already visible on another button: schedule a
            // short settle delay before swapping so a pointer that
            // merely passes over this brush on the way elsewhere
            // does not promote it. The timer no-ops if `hovered_idx`
            // has moved on by the time it fires.
            let (delay, board_timer) = if open_idx.peek().is_some()
                && *open_idx.peek() != Some(idx)
            {
                (HOVER_SWAP_DELAY, Arc::clone(&enter_board))
            } else {
                (HOVER_DELAY, Arc::clone(&enter_board))
            };
            spawn(async move {
                async_io::Timer::after(delay).await;
                if *hovered_idx.peek() == Some(idx) {
                    open_idx.set(Some(idx));
                    focus_palette_entry(&board_timer, preset, idx, selected, scale);
                }
            });
        })
        .on_pointer_leave(move |_| {
            if *leave_hovered_idx.peek() == Some(idx) {
                leave_hovered_idx.set(None);
            }
            schedule_popup_close(
                leave_open_idx,
                leave_hovered_idx,
                leave_popup_hovered,
                leave_expanded,
            );
        })
        .on_press(move |_: Event<PressEventData>| {
            let was_active = *selected.peek() == idx;
            let is_expanded = *press_expanded.peek();
            // Apply the preset now so `Board::set_current_preset`
            // folds in this tool's remembered `size_scale`; propagate
            // the resulting size into the shared `scale` state so the
            // label refreshes immediately instead of drifting until
            // the next explicit resize.
            let new_size = {
                let mut g = lock(&press_board);
                if !same_brush_family(g.current_kind(), preset.kind) {
                    g.set_current_preset(preset);
                }
                g.current_size()
            };
            *scale.write() = new_size;
            *selected.write() = idx;
            // Expand / collapse semantics:
            //   - compact → click selected brush expands the row.
            //   - expanded + click same brush → collapse back to compact.
            //   - expanded + click different brush → just select, stays expanded.
            // Popup is dismissed on every collapse to keep the two
            // visual affordances in step.
            if !is_expanded {
                press_expanded.set(true);
                press_open_idx.set(None);
            } else if was_active {
                press_expanded.set(false);
                press_open_idx.set(None);
            }
        })
        .child(
            rect()
                .position(Position::new_absolute().left(-2.).top(-2.))
                .child(
                    SvgViewer::new(fill_icon)
                        .width(Size::px(30.))
                        .height(Size::px(30.))
                        .color(swatch_color),
                ),
        )
        .child(
            rect().layer(Layer::Relative(2)).child(
                SvgViewer::new(icon)
                    .width(Size::px(30.))
                    .height(Size::px(30.)),
            ),
        )
}

fn commit_preset(board: &Arc<Mutex<Board>>, preset: BrushPreset) {
    let mut guard = lock(board);
    // Compare on kind alone. `set_current_preset` folds in the
    // remembered size_scale for that kind, so a full-preset compare
    // would spuriously re-fire whenever the user tunes the slider.
    if !same_brush_family(guard.current_kind(), preset.kind) {
        guard.set_current_preset(preset);
    }
}

/// Promote the palette entry at `idx` to the active selection: commit
/// its preset onto the board and refresh the reactive `selected` /
/// `scale` signals so every popup row (color, size, …) reads /
/// writes against this brush instead of whichever was active before.
///
/// Shared by hover-opens, hover-swaps and the stylus-driven pen hover
/// path so each code path promotes the preset identically.
fn focus_palette_entry(
    board: &Arc<Mutex<Board>>,
    preset: BrushPreset,
    idx: usize,
    mut selected: State<usize>,
    mut scale: State<f32>,
) {
    let new_size = {
        let mut g = lock(board);
        if !same_brush_family(g.current_kind(), preset.kind) {
            g.set_current_preset(preset);
        }
        g.current_size()
    };
    *scale.write() = new_size;
    if *selected.peek() != idx {
        *selected.write() = idx;
    }
}

/// How long the close-timer waits after the pointer leaves a palette
/// button / the popup body before dismissing the popup. Long enough
/// for the pointer to cross the ~6 px gap between palette row and
/// popup, short enough that an outright exit feels responsive.
pub(crate) const POPUP_CLOSE_DELAY: Duration = Duration::from_millis(380);

/// How long a pointer must settle on a palette button before promoting
/// it to the active brush while the popup is already open on another.
/// Prevents mid-traversal brushes from being selected when the user
/// swings the pointer across the row to reach a far one.
pub(crate) const HOVER_SWAP_DELAY: Duration = Duration::from_millis(180);

/// Spawn a one-shot close timer. Fires only when, at expiry, no
/// palette button is hovered AND the popup body itself is not under
/// the pointer. Scheduled from both `palette_button::on_pointer_leave`
/// and the popup body's own leave handler so either exit path drives
/// the popup closed. Also collapses the palette row back to compact
/// so a hover-driven open never leaves the full row stuck visible
/// after the pointer walks away.
pub(crate) fn schedule_popup_close(
    open_idx: State<Option<usize>>,
    hovered_idx: State<Option<usize>>,
    popup_hovered: State<bool>,
    expanded: State<bool>,
) {
    let mut open_idx = open_idx;
    let mut expanded = expanded;
    spawn(async move {
        async_io::Timer::after(POPUP_CLOSE_DELAY).await;
        if hovered_idx.peek().is_none() && !*popup_hovered.peek() {
            if open_idx.peek().is_some() {
                open_idx.set(None);
            }
            if *expanded.peek() {
                expanded.set(false);
            }
        }
    });
}

/// React to pen-hover updates: find which palette button the tool is
/// over, and drive `hovered_idx` / the open-popup timer the same way
/// [`palette_button`]'s Freya `on_pointer_enter` handler does for
/// mouse input. No-op when the pen has not moved between palette
/// buttons since the last call — a `pen_last_target` state guards
/// against re-firing the same timer every render.
fn dispatch_pen_hover(
    board: &Arc<Mutex<Board>>,
    selected: State<usize>,
    scale: State<f32>,
    coords: PopupCoords,
) {
    let mut hovered_idx = coords.hovered_idx;
    let mut open_idx = coords.open_idx;
    let popup_hovered = coords.popup_hovered;
    let mut pen_last_target = coords.pen_last_target;
    let pen = *coords.pen_hover.read();
    let areas = coords.button_areas.read();
    let cur = pen.and_then(|hp| find_button_under(&areas, hp.x, hp.y));
    if *pen_last_target.peek() == cur {
        return;
    }
    pen_last_target.set(cur);
    match cur {
        Some(idx) => {
            hovered_idx.set(Some(idx));
            // Mirror the mouse-side settle delay: when the popup is
            // already open on another button, debounce the swap so a
            // stylus crossing the row quickly does not promote every
            // brush it passes over.
            let delay = if open_idx.peek().is_some() && *open_idx.peek() != Some(idx) {
                HOVER_SWAP_DELAY
            } else {
                HOVER_DELAY
            };
            let board = Arc::clone(board);
            spawn(async move {
                async_io::Timer::after(delay).await;
                if *hovered_idx.peek() == Some(idx) {
                    open_idx.set(Some(idx));
                    let preset = PALETTE[idx]();
                    focus_palette_entry(&board, preset, idx, selected, scale);
                }
            });
        }
        None => {
            if hovered_idx.peek().is_some() {
                hovered_idx.set(None);
            }
            schedule_popup_close(open_idx, hovered_idx, popup_hovered, coords.expanded);
        }
    }
}

/// Linear scan across `areas` returning the first index whose stored
/// rect contains `(x, y)`. Cheap enough at PALETTE-length list sizes;
/// the alternative bucket / spatial cache is not worth the memory.
fn find_button_under(areas: &[Option<Area>], x: f32, y: f32) -> Option<usize> {
    areas.iter().enumerate().find_map(|(idx, slot)| {
        slot.and_then(|a| {
            (x >= a.min_x() && x <= a.max_x() && y >= a.min_y() && y <= a.max_y()).then_some(idx)
        })
    })
}

/// Are two [`BrushKind`]s the same "palette family"?
///
/// Every [`BrushKind::Shape`] mode collapses to the single shape
/// family so switching mode via the popup does not reset the current
/// preset back to the palette default. Non-Shape kinds compare equal
/// only by exact value.
const fn same_brush_family(a: BrushKind, b: BrushKind) -> bool {
    match (a, b) {
        (BrushKind::Shape(_), BrushKind::Shape(_))
        | (BrushKind::Pencil, BrushKind::Pencil)
        | (BrushKind::Marker, BrushKind::Marker)
        | (BrushKind::Eraser, BrushKind::Eraser)
        | (BrushKind::Pen, BrushKind::Pen)
        | (BrushKind::Highlighter, BrushKind::Highlighter) => true,
        (BrushKind::Custom(x), BrushKind::Custom(y)) => x == y,
        _ => false,
    }
}

// Right-side floating layer stack. Displays layers top-of-stack first
// so the visual matches Procreate / Photoshop conventions (topmost
// paint order = topmost row), with `+` add / `-` remove buttons
// beneath. Every mutation bumps `layers_ver` so freya re-runs this
// component and picks up the new snapshot.
/// Compose the top-right chrome: a horizontal icon toolbar always
/// present, and the layers panel body stacked beneath when
/// `state.layers_visible` is set. Both share one absolute anchor so
/// the toolbar's layout drives where the panel docks.
fn top_right_stack(
    board: &Arc<Mutex<Board>>,
    state: &ChromeState,
    pad: EdgeInsets,
) -> impl IntoElement {
    let layers_visible = *state.layers_visible.read();
    let mut stack = rect()
        .position(
            Position::new_absolute()
                .top(pad.top + 8.0)
                .right(pad.right + 8.0),
        )
        .vertical()
        .cross_align(Alignment::End)
        .spacing(8.0)
        .layer(Layer::Overlay)
        .on_sized(move |e: Event<SizedEventData>| publish_mask(UiRegion::Layers, e.area))
        .child(top_right_toolbar(state));
    if layers_visible {
        stack = stack.child(layers_panel_body(board, state.layers_ver));
    }
    stack
}

/// Horizontal row of icon buttons that lives above the layers panel.
/// Current slots: toggle layers panel, open the document-config edit
/// sheet, placeholder for a generic settings pane.
fn top_right_toolbar(state: &ChromeState) -> impl IntoElement {
    let mut layers_vis = state.layers_visible;
    let toggle_layers: NoArgCallback<()> = NoArgCallback::new(move || {
        let next = !*layers_vis.peek();
        layers_vis.set(next);
    });

    let active_item = state.active_item;
    let open_doc_config: NoArgCallback<()> = NoArgCallback::new(move || {
        // Route is `WorkspaceView` for every pane inside a workspace,
        // so `canvas_item()` on the route would always be `None` and
        // the sheet never opens. Read the per-pane `active_item`
        // signal that `canvas_body` publishes on mount instead.
        let Some(id) = *active_item.peek() else {
            // Ephemeral mounts have no item to edit.
            return;
        };
        crate::home::open_canvas_edit_sheet(id);
    });

    rect()
        .horizontal()
        .spacing(6.0)
        .child(icon_button(layers(), toggle_layers))
        .child(icon_button(sliders_horizontal(), open_doc_config))
}

/// Floating layer-stack body — just the card. Positioning now lives in
/// [`top_right_stack`] so this fn can be composed under any anchor the
/// shell wants (currently the top-right toolbar's vertical column).
fn layers_panel_body(board: &Arc<Mutex<Board>>, layers_ver: State<u32>) -> impl IntoElement {
    // Subscribe to `layers_ver` so mutations elsewhere reactively
    // re-run this component.
    let _tick = *layers_ver.read();

    let (layers, active_id) = {
        let g = lock(board);
        (g.layers_snapshot(), g.active_layer_id())
    };

    let mut col = rect()
        .vertical()
        .spacing(6.0)
        .padding(10.0)
        .background(Color::from_rgb(30, 30, 34))
        .with_corner_radius(10.0);

    // Header — plain label, no interaction.
    col = col.child(
        label()
            .color(Color::from_rgb(210, 210, 214))
            .font_size(12.0)
            .text("Layers"),
    );

    // Doc stores layers bottom-to-top; UI shows top-to-bottom.
    for snap in layers.iter().rev() {
        let is_active = snap.id == active_id;
        col = col.child(layer_row(snap, is_active, board, layers_ver));
    }

    col = col.child(layer_action_row(board, layers_ver));
    col
}

fn layer_row(
    snap: &LayerSnapshot,
    is_active: bool,
    board: &Arc<Mutex<Board>>,
    layers_ver: State<u32>,
) -> impl IntoElement {
    let (bg, fg) = if is_active {
        (Color::from_rgb(70, 70, 78), Color::WHITE)
    } else {
        (Color::from_rgb(50, 50, 55), Color::from_rgb(210, 210, 214))
    };

    // Filled disc = visible, hollow disc = hidden. Tap toggles.
    let eye_symbol = if snap.visible { "\u{25c9}" } else { "\u{25cb}" };
    let vis_board = Arc::clone(board);
    let vis_id = snap.id;
    let vis_now = snap.visible;
    let mut ver_vis = layers_ver;
    let eye = rect()
        .padding((4.0, 6.0))
        .background(Color::from_rgb(60, 60, 66))
        .with_corner_radius(4.0)
        .on_press(move |_| {
            lock(&vis_board).set_layer_visible(vis_id, !vis_now);
            *ver_vis.write() += 1;
        })
        .child(label().color(fg).font_size(13.0).text(eye_symbol));

    // Padlock symbol reflects state; tap toggles.
    let lock_symbol = if snap.locked {
        "\u{1f512}"
    } else {
        "\u{1f513}"
    };
    let lock_board = Arc::clone(board);
    let lock_id = snap.id;
    let lock_now = snap.locked;
    let mut ver_lock = layers_ver;
    let padlock = rect()
        .padding((4.0, 6.0))
        .background(Color::from_rgb(60, 60, 66))
        .with_corner_radius(4.0)
        .on_press(move |_| {
            lock(&lock_board).set_layer_locked(lock_id, !lock_now);
            *ver_lock.write() += 1;
        })
        .child(label().color(fg).font_size(12.0).text(lock_symbol));

    let sel_board = Arc::clone(board);
    let sel_id = snap.id;
    let mut ver_sel = layers_ver;
    let count_text = format!("{}  ({})", snap.name, snap.stroke_count);

    rect()
        .horizontal()
        .spacing(6.0)
        .padding((6.0, 8.0))
        .background(bg)
        .with_corner_radius(6.0)
        .on_press(move |_| {
            lock(&sel_board).set_active_layer(sel_id);
            *ver_sel.write() += 1;
        })
        .child(eye)
        .child(padlock)
        .child(label().color(fg).font_size(13.0).text(count_text))
}

fn layer_action_row(board: &Arc<Mutex<Board>>, layers_ver: State<u32>) -> impl IntoElement {
    let add_board = Arc::clone(board);
    let mut ver_add = layers_ver;
    let add = rect()
        .padding((4.0, 8.0))
        .background(Color::from_rgb(60, 90, 60))
        .with_corner_radius(4.0)
        .on_press(move |_| {
            lock(&add_board).add_layer();
            *ver_add.write() += 1;
        })
        .child(label().color(Color::WHITE).font_size(14.0).text("+"));

    let del_board = Arc::clone(board);
    let mut ver_del = layers_ver;
    let del = rect()
        .padding((4.0, 8.0))
        .background(Color::from_rgb(90, 60, 60))
        .with_corner_radius(4.0)
        .on_press(move |_| {
            lock(&del_board).remove_active_layer();
            *ver_del.write() += 1;
        })
        .child(label().color(Color::WHITE).font_size(14.0).text("\u{2212}"));

    rect().horizontal().spacing(4.0).child(add).child(del)
}
