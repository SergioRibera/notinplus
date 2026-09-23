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
use istmo::plugins::{EdgeInsets, SafeArea, SafeAreaInsets};

use crate::brush::BrushPreset;
use crate::canvas::{Board, LayerSnapshot, RedrawNotifier, drawing_surface, lock};

const PALETTE: &[fn() -> BrushPreset] = &[
    BrushPreset::pencil,
    BrushPreset::marker,
    BrushPreset::pen,
    BrushPreset::highlighter,
    BrushPreset::eraser,
];

/// Launch the app in a desktop window.
pub fn run_desktop() {
    launch(LaunchConfig::new().with_window(WindowConfig::new(root).with_title("notinplus")));
}

/// Launch the app inside the mobile shell (Android `GameActivity` /
/// iOS `SwiftUI` container). Same `root` component as desktop.
pub fn run_mobile() {
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
                .with_window(WindowConfig::new(root).with_title("notinplus")),
        );
    }

    #[cfg(not(target_os = "android"))]
    launch(LaunchConfig::new().with_window(WindowConfig::new(root).with_title("notinplus")));
}

fn root() -> impl IntoElement {
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

    let mut insets = use_state(EdgeInsets::default);
    use_hook(move || {
        // Bridge the SafeArea early-event stream (which lives on a
        // helper OS thread) into the freya reactive world through a
        // flume channel. `State` is `!Send`, so we can't touch it from
        // the recv thread; the async drain runs inside freya's
        // executor where `insets.set(..)` is safe.
        let sa = match SafeArea::acquire() {
            Ok(sa) => sa,
            Err(err) => {
                log::debug!("safe_area not ready: {err:?}");
                return;
            }
        };
        let initial = fold_insets(sa.current_or_zero());
        insets.set(initial);
        let (tx, rx) = flume::unbounded::<EdgeInsets>();
        let stream = sa.stream();
        std::thread::spawn(move || {
            while let Ok(next) = stream.recv() {
                if tx.send(fold_insets(next)).is_err() {
                    break;
                }
            }
        });
        spawn(async move {
            while let Ok(next) = rx.recv_async().await {
                insets.set(next);
            }
        });
    });

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

    rect()
        .width(Size::fill())
        .height(Size::fill())
        .child(drawing_surface(&board))
        .child(palette_overlay(&board, selected, scale, pad))
        .child(layers_panel(&board, layers_ver, pad))
        .child(zoom_overlay(&board, zoom, pad))
}

fn zoom_overlay(
    board: &Arc<Mutex<Board>>,
    zoom: State<f32>,
    pad: EdgeInsets,
) -> impl IntoElement {
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
        .spacing(6.0)
        .padding((6.0, 8.0))
        .background(Color::from_argb(220, 30, 30, 34))
        .with_corner_radius(8.0)
        .child(label().color(Color::WHITE).font_size(12.0).text(text))
        .child(reset)
}

// Fold platform-published `SafeAreaInsets` (system bars + display
// cutout + IME) into a single set of edge padding the app applies. IME
// only pushes bottom padding — top/left/right ignore it so the palette
// doesn't jump when the keyboard opens.
const fn fold_insets(insets: SafeAreaInsets) -> EdgeInsets {
    let base = insets.system_bars.max(insets.display_cutout);
    EdgeInsets {
        top: base.top,
        right: base.right,
        bottom: base.bottom.max(insets.ime.bottom),
        left: base.left,
    }
}

fn palette_overlay(
    board: &Arc<Mutex<Board>>,
    selected: State<usize>,
    scale: State<f32>,
    pad: EdgeInsets,
) -> impl IntoElement {
    // Global-positioned so the palette floats above the fullscreen
    // canvas at `(pad.left, pad.top)` — no room reserved by the parent's
    // flow layout, which is what lets the canvas sit under the status
    // bar / notch on Android and iOS.
    let mut row = rect()
        .position(
            Position::new_global()
                .top(pad.top + 8.0)
                .left(pad.left + 8.0),
        )
        .horizontal()
        .spacing(8.0)
        .padding(10.0)
        .background(Color::from_argb(220, 30, 30, 34))
        .with_corner_radius(10.0);

    for (idx, make) in PALETTE.iter().enumerate() {
        row = row.child(palette_button(idx, make(), board, selected, scale));
    }
    row = row
        .child(size_control(board, scale))
        .child(undo_button(board));
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
) -> impl IntoElement {
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
        .on_press(move |_| {
            // Apply the preset now so `Board::set_current_preset`
            // folds in this tool's remembered `size_scale`; propagate
            // the resulting size into the shared `scale` state so the
            // label refreshes immediately instead of drifting until
            // the next explicit resize.
            let new_size = {
                let mut g = lock(&press_board);
                if g.current_kind() != preset.kind {
                    g.set_current_preset(preset);
                }
                g.current_size()
            };
            *scale.write() = new_size;
            *selected.write() = idx;
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
    if guard.current_kind() != preset.kind {
        guard.set_current_preset(preset);
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
        .background(Color::from_argb(220, 30, 30, 34))
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
