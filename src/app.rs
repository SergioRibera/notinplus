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

use crate::brush::{Brush, EraserStyle, HighlighterStyle, MarkerStyle, PenStyle, PencilStyle};
use crate::canvas::{Board, RedrawNotifier, drawing_surface, lock};

const PALETTE: &[fn() -> Brush] = &[
    || Brush::Pencil(PencilStyle::default()),
    || Brush::Marker(MarkerStyle::default()),
    || Brush::Pen(PenStyle::default()),
    || Brush::Highlighter(HighlighterStyle::default()),
    || Brush::Eraser(EraserStyle::default()),
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
    let board = use_hook(|| {
        let board = Board::shared();
        let platform = Platform::get();
        let (notifier, rx) = RedrawNotifier::new();
        lock(&board).set_notifier(notifier);
        spawn(async move {
            while rx.recv_async().await.is_ok() {
                platform.send(UserEvent::RequestRedraw);
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
    let pad = *insets.read();

    rect()
        .width(Size::fill())
        .height(Size::fill())
        .child(drawing_surface(&board))
        .child(palette_overlay(&board, selected, pad))
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
    pad: EdgeInsets,
) -> impl IntoElement {
    // Global-positioned so the palette floats above the fullscreen
    // canvas at `(pad.left, pad.top)` — no room reserved by the parent's
    // flow layout, which is what lets the canvas sit under the status
    // bar / notch on Android and iOS.
    let mut row = rect()
        .position(Position::new_global().top(pad.top + 8.0).left(pad.left + 8.0))
        .horizontal()
        .spacing(8.0)
        .padding(10.0)
        .background(Color::from_argb(220, 30, 30, 34))
        .with_corner_radius(10.0);

    for (idx, make) in PALETTE.iter().enumerate() {
        row = row.child(palette_button(idx, make(), board, selected));
    }
    row
}

fn palette_button(
    idx: usize,
    brush: Brush,
    board: &Arc<Mutex<Board>>,
    mut selected: State<usize>,
) -> impl IntoElement {
    let is_active = *selected.read() == idx;
    let swatch_color = brush
        .color()
        .unwrap_or_else(|| Color::from_rgb(220, 220, 220));

    // Commit the currently-selected brush to the board every render —
    // cheap, and keeps the board's active brush in lock-step with the
    // palette without needing a dedicated effect.
    if is_active {
        commit_brush(board, brush);
    }

    let (bg, fg) = if is_active {
        (Color::from_rgb(70, 70, 78), Color::WHITE)
    } else {
        (Color::from_rgb(50, 50, 55), Color::from_rgb(210, 210, 214))
    };

    rect()
        .horizontal()
        .spacing(6.0)
        .padding((8.0, 10.0))
        .background(bg)
        .with_corner_radius(6.0)
        .on_press(move |_| {
            *selected.write() = idx;
        })
        .child(
            rect()
                .width(Size::px(14.0))
                .height(Size::px(14.0))
                .background(swatch_color)
                .with_corner_radius(3.0),
        )
        .child(label().color(fg).font_size(14.0).text(brush.label()))
}

fn commit_brush(board: &Arc<Mutex<Board>>, brush: Brush) {
    let mut guard = match board.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.brush() != brush {
        guard.set_brush(brush);
    }
}
