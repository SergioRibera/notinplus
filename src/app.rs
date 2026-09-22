//! Freya app shell — brush palette + drawing surface.
//!
//! The same `root` component drives desktop (`launch`) and mobile
//! (`#[istmo::mobile_app]`) entry points; only the launcher wrapper
//! differs.

use std::sync::{Arc, Mutex};

use freya::prelude::*;

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
    // Freya's canvas widget only repaints when the window is asked to
    // redraw — the `RenderCallback` closure has `eq == true`, so the
    // diff never marks it dirty on its own. We wire a bounded flume
    // channel: every `Board` mutation pings it (from both the pointer
    // handlers and the pen_pump thread), and this drain task translates
    // pings into `UserEvent::RequestRedraw`.
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
    let selected = use_state(|| 0usize);

    let mut palette = rect()
        .horizontal()
        .spacing(8.0)
        .padding(10.0)
        .background(Color::from_rgb(30, 30, 34));

    for (idx, make) in PALETTE.iter().enumerate() {
        palette = palette.child(palette_button(idx, make(), &board, selected));
    }

    rect()
        .width(Size::fill())
        .height(Size::fill())
        .vertical()
        .child(palette)
        .child(drawing_surface(&board))
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
