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

use freya::icons::lucide::{arrow_left, hand, pencil};
use freya::prelude::*;
use freya::router::*;
use istmo::plugins::EdgeInsets;

use crate::route::Route;

use crate::brush::{BrushKind, BrushPreset, ShapeMode};
use crate::canvas::{Board, InputMode, LayerSnapshot, RedrawNotifier, drawing_surface, lock};
use crate::hooks::use_safe_area_insets;
use crate::palette_popup::{HOVER_DELAY, brush_popup};
use crate::pen_pump;
use crate::ui_mask::{self, UiRegion};

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
struct PopupCoords {
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
}

const PALETTE: &[fn() -> BrushPreset] = &[
    BrushPreset::pencil,
    BrushPreset::marker,
    BrushPreset::pen,
    BrushPreset::highlighter,
    BrushPreset::eraser,
    default_shape,
];

const fn default_shape() -> BrushPreset {
    BrushPreset::shape(ShapeMode::Line)
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
    launch(LaunchConfig::new().with_window(
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
    ));
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

pub(crate) fn root() -> impl IntoElement {
    let mut zoom = use_state(|| 1.0_f32);
    let board = use_hook(|| {
        let board = Board::shared();
        let platform = Platform::get();
        let (notifier, rx) = RedrawNotifier::new();
        lock(&board).set_notifier(notifier);
        let zoom_board = Arc::clone(&board);
        spawn(async move {
            while rx.recv_async().await.is_ok() {
                platform.send(UserEvent::RequestRedraw);
                // Piggyback the redraw wakeup: sync the reactive zoom
                // signal off the freshest viewport so the HUD label
                // stays in step with pinch / wheel gestures without
                // needing its own notifier plumbing.
                let s = lock(&zoom_board).viewport().scale;
                // 0.1% threshold — the HUD label rounds to whole
                // percent, so a stricter epsilon would push zoom.set
                // (and the reactive root re-render it triggers) far
                // more often than the label actually changes.
                if (s - *zoom.read()).abs() > 0.001 {
                    zoom.set(s);
                }
            }
        });
        board
    });

    let insets = use_safe_area_insets();

    let selected = use_state(|| 0usize);
    // Ticks once per layer mutation. Every layer-panel `on_press`
    // bumps it after mutating the board, which is what wakes freya's
    // reactive re-run for the panel (the canvas has its own
    // `RedrawNotifier` path).
    let layers_ver = use_state(|| 0u32);
    let scale = {
        let board = Arc::clone(&board);
        use_state(move || {
            // Align the board with the initial palette entry so the
            // scale label reads the tool the user actually sees
            // highlighted, not whatever `Board::default` picked.
            let mut g = lock(&board);
            let initial = PALETTE[0]();
            if g.current_kind() != initial.kind {
                g.set_current_preset(initial);
            }
            g.current_size()
        })
    };
    let pad = *insets.read();

    // Popup coordination shared across every palette button. `open_idx`
    // drives which popup is visible; `hovered_idx` is the current
    // hover target (used by the delay timer to detect cancellation);
    // `button_areas` receives every button's `on_sized` update so the
    // popup can anchor beneath the correct button on desktop layouts
    // whose widths vary with label length.
    let coords = PopupCoords {
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
    };

    let mode = {
        let board = Arc::clone(&board);
        use_state(move || lock(&board).input_mode())
    };

    rect()
        .width(Size::fill())
        .height(Size::fill())
        .child(drawing_surface(&board))
        .child(back_overlay(&board, pad))
        .child(palette_overlay(&board, selected, scale, pad, coords))
        .child(layers_panel(&board, layers_ver, pad))
        .child(mode_overlay(&board, mode, pad))
        .child(zoom_overlay(&board, zoom, pad))
}

/// Top-left "back to home" pill. Snapshots the current doc, spawns a
/// save task, then routes to `Route::Home`. The navigation kicks off
/// before the save resolves so the UI feels snappy — the async writer
/// finishes in the background and reports errors via `log`.
fn back_overlay(board: &Arc<Mutex<Board>>, pad: EdgeInsets) -> impl IntoElement {
    let board = Arc::clone(board);
    rect()
        .position(
            Position::new_global()
                .top(pad.top + 8.0)
                .left(pad.left + 8.0),
        )
        .padding((6.0, 10.0))
        .layer(Layer::Overlay)
        .background(Color::from_rgb(30, 30, 34))
        .with_corner_radius(8.0)
        .on_sized(move |e: Event<SizedEventData>| publish_mask(UiRegion::Back, e.area))
        .on_press(move |_| {
            let doc_snapshot = {
                let guard = lock(&board);
                guard.doc().clone()
            };
            let item_id = crate::route::current_canvas_item();
            spawn(async move {
                if let Some(id) = item_id {
                    match crate::home::open_library().await {
                        Ok(handle) => {
                            let mut lib = handle.lock().await;
                            if let Err(err) = lib.save_doc(id, &doc_snapshot).await {
                                log::error!("save_doc: {err}");
                            }
                        }
                        Err(err) => log::error!("open library on back: {err}"),
                    }
                }
                crate::route::set_current_canvas_item(None);
                let _ = RouterContext::get().push(crate::route::Route::Home);
            });
        })
        .child(
            SvgViewer::new(arrow_left())
                .fill(Color::WHITE)
                .width(Size::px(13.))
                .height(Size::px(13.)),
        )
}

/// Toggle between [`InputMode::Draw`] and [`InputMode::Pan`]. Single
/// icon-only button that mirrors the current mode so the glyph itself
/// communicates the *active* interpretation (pencil = drawing, hand =
/// panning).
fn mode_overlay(
    board: &Arc<Mutex<Board>>,
    mut mode: State<InputMode>,
    pad: EdgeInsets,
) -> impl IntoElement {
    let current = *mode.read();
    let press_board = Arc::clone(board);
    let icon = match current {
        InputMode::Draw => pencil(),
        InputMode::Pan => hand(),
    };
    rect()
        .position(
            Position::new_global()
                .bottom(pad.bottom + 8.0)
                .left(pad.left + 8.0),
        )
        .padding((6.0, 10.0))
        .layer(Layer::Overlay)
        .background(Color::from_rgb(30, 30, 34))
        .with_corner_radius(8.0)
        .on_sized(move |e: Event<SizedEventData>| publish_mask(UiRegion::Mode, e.area))
        .on_press(move |_| {
            let next = match current {
                InputMode::Draw => InputMode::Pan,
                InputMode::Pan => InputMode::Draw,
            };
            lock(&press_board).set_input_mode(next);
            mode.set(next);
        })
        .child(
            SvgViewer::new(icon)
                .fill(Color::WHITE)
                .width(Size::px(16.))
                .height(Size::px(16.)),
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
            Position::new_global()
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
    pad: EdgeInsets,
    coords: PopupCoords,
) -> impl IntoElement {
    dispatch_pen_hover(coords);
    // Global-positioned so the palette floats above the fullscreen
    // canvas at `(pad.left, pad.top)` — no room reserved by the parent's
    // flow layout, which is what lets the canvas sit under the status
    // bar / notch on Android and iOS.
    let mut row = rect()
        .position(
            Position::new_global()
                .top(pad.top + 8.0)
                .left(pad.left + 48.0),
        )
        .horizontal()
        .spacing(8.0)
        .padding(10.0)
        .layer(Layer::Overlay)
        .background(Color::from_rgb(30, 30, 34))
        .with_corner_radius(10.0)
        .on_sized(move |e: Event<SizedEventData>| publish_mask(UiRegion::Palette, e.area));

    for (idx, make) in PALETTE.iter().enumerate() {
        row = row.child(palette_button(idx, make(), board, selected, scale, coords));
    }
    row = row
        .child(size_control(board, scale))
        .child(undo_button(board));

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

fn undo_button(board: &Arc<Mutex<Board>>) -> impl IntoElement {
    let handle = Arc::clone(board);
    rect()
        .padding((6.0, 10.0))
        .background(Color::from_rgb(80, 60, 60))
        .with_corner_radius(6.0)
        .on_press(move |_| {
            lock(&handle).undo();
        })
        .child(label().color(Color::WHITE).font_size(14.0).text("Undo"))
}

fn palette_button(
    idx: usize,
    preset: BrushPreset,
    board: &Arc<Mutex<Board>>,
    mut selected: State<usize>,
    mut scale: State<f32>,
    coords: PopupCoords,
) -> impl IntoElement {
    let mut open_idx = coords.open_idx;
    let mut hovered_idx = coords.hovered_idx;
    let mut button_areas = coords.button_areas;
    let is_active = *selected.read() == idx;
    let swatch_color = preset
        .palette_color()
        .map_or(Color::from_rgb(220, 220, 220), |[r, g, b, _]| {
            Color::from_rgb(r, g, b)
        });

    // Commit the currently-selected preset to the board every render —
    // cheap, and keeps the board's active brush in lock-step with the
    // palette without needing a dedicated effect.
    if is_active {
        commit_preset(board, preset);
    }

    let (bg, fg) = if is_active {
        (Color::from_rgb(70, 70, 78), Color::WHITE)
    } else {
        (Color::from_rgb(50, 50, 55), Color::from_rgb(210, 210, 214))
    };

    let press_board = Arc::clone(board);
    rect()
        .horizontal()
        .spacing(6.0)
        .padding((8.0, 10.0))
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
            // Fire-and-forget hover timer. If the pointer leaves
            // before `HOVER_DELAY` elapses, `hovered_idx` no longer
            // points at `idx` and the open never fires.
            spawn(async move {
                async_io::Timer::after(HOVER_DELAY).await;
                if *hovered_idx.peek() == Some(idx) {
                    open_idx.set(Some(idx));
                }
            });
        })
        .on_pointer_leave(move |_| {
            if *hovered_idx.peek() == Some(idx) {
                hovered_idx.set(None);
            }
        })
        .on_press(move |e: Event<PressEventData>| {
            let is_touch = matches!(*e.data(), PressEventData::Touch(_));
            let was_active = *selected.peek() == idx;
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
            if is_touch && was_active {
                // Second tap on the already-selected brush toggles
                // the popup — the touch equivalent of hover-delay.
                let next = if *open_idx.peek() == Some(idx) {
                    None
                } else {
                    Some(idx)
                };
                open_idx.set(next);
            } else if !is_touch && *open_idx.peek() == Some(idx) {
                // Pointer / keyboard select on the currently-open
                // popup dismisses it — user is committing the choice.
                open_idx.set(None);
            } else if is_touch {
                // First tap on a different brush — always close any
                // previously-open popup so the two selections stay
                // in sync.
                open_idx.set(None);
            }
        })
        .child(
            rect()
                .width(Size::px(14.0))
                .height(Size::px(14.0))
                .background(swatch_color)
                .with_corner_radius(3.0),
        )
        .child(label().color(fg).font_size(14.0).text(preset.label()))
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

/// React to pen-hover updates: find which palette button the tool is
/// over, and drive `hovered_idx` / the open-popup timer the same way
/// [`palette_button`]'s Freya `on_pointer_enter` handler does for
/// mouse input. No-op when the pen has not moved between palette
/// buttons since the last call — a `pen_last_target` state guards
/// against re-firing the same timer every render.
fn dispatch_pen_hover(coords: PopupCoords) {
    let mut hovered_idx = coords.hovered_idx;
    let mut open_idx = coords.open_idx;
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
            spawn(async move {
                async_io::Timer::after(HOVER_DELAY).await;
                if *hovered_idx.peek() == Some(idx) {
                    open_idx.set(Some(idx));
                }
            });
        }
        None => {
            if hovered_idx.peek().is_some() {
                hovered_idx.set(None);
            }
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
fn layers_panel(
    board: &Arc<Mutex<Board>>,
    layers_ver: State<u32>,
    pad: EdgeInsets,
) -> impl IntoElement {
    // Subscribe to `layers_ver` so mutations elsewhere reactively
    // re-run this component.
    let _tick = *layers_ver.read();

    let (layers, active_id) = {
        let g = lock(board);
        (g.layers_snapshot(), g.active_layer_id())
    };

    let mut col = rect()
        .position(
            Position::new_global()
                .top(pad.top + 8.0)
                .right(pad.right + 8.0),
        )
        .vertical()
        .spacing(6.0)
        .padding(10.0)
        .layer(Layer::Overlay)
        .background(Color::from_rgb(30, 30, 34))
        .with_corner_radius(10.0)
        .on_sized(move |e: Event<SizedEventData>| publish_mask(UiRegion::Layers, e.area));

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
