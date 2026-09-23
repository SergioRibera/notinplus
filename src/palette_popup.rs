//! Hover / long-press popup for the brush palette.
//!
//! Trigger rules:
//!
//! - Mouse / pen: hovering over a button for [`HOVER_DELAY`] opens its
//!   popup. Leaving the button before the timer fires cancels — a
//!   generation counter on `hovered` invalidates stale timers so a
//!   quick re-enter on a different button does not open the previous.
//! - Touch: second tap on the currently-selected button opens the
//!   popup. First tap selects the brush; second tap flips the popup.
//!
//! Content is per-[`BrushKind`]. Every editable knob writes through
//! [`Board::set_brush_config`] so the popup owns no state beyond the
//! transient `open_idx` shared with `crate::app::root`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use freya::prelude::*;

use crate::brush::{BrushConfig, BrushKind, BrushPreset, EraserMode, PressureCurve};
use crate::canvas::{Board, lock};

/// How long the pointer must sit inside a palette button before the
/// popup opens. Matches Procreate's tool inspector delay closely
/// enough to feel deliberate without being sluggish.
pub const HOVER_DELAY: Duration = Duration::from_millis(500);

const SWATCHES: &[[u8; 4]] = &[
    [20, 20, 20, 255],
    [230, 230, 230, 255],
    [200, 60, 60, 255],
    [230, 130, 40, 255],
    [230, 205, 60, 255],
    [80, 170, 80, 255],
    [60, 130, 220, 255],
    [140, 90, 200, 255],
];

/// Render the popup for the palette entry at `idx`.
///
/// Positioned directly below `anchor`. Returns an element the caller
/// stacks into the palette overlay unconditionally — hides itself as
/// a zero-size stub when `open_idx` is not `Some(idx)` or `anchor` is
/// unknown.
pub fn brush_popup(
    board: &Arc<Mutex<Board>>,
    open_idx: State<Option<usize>>,
    anchor: Option<Area>,
    idx: usize,
    preset: BrushPreset,
) -> impl IntoElement {
    let show = *open_idx.read() == Some(idx);
    let (top, left) = anchor.map_or((0.0, 0.0), |a| (a.max_y() + 6.0, a.min_x()));

    let mut body = rect()
        .position(Position::new_global().top(top).left(left))
        .vertical()
        .spacing(8.0)
        .padding(10.0)
        .background(Color::from_argb(240, 40, 40, 46))
        .with_corner_radius(10.0);

    if !show || anchor.is_none() {
        // Zero-size hidden stub keeps both branches of `impl
        // IntoElement` in the same concrete type without a Left/Right
        // wrapper.
        return body.width(Size::px(0.0)).height(Size::px(0.0));
    }

    if preset.kind != BrushKind::Eraser {
        body = body.child(color_row(board));
    }
    body = body.child(size_row(board));
    body = match preset.kind {
        BrushKind::Pen => body.child(pen_curve_row(board)),
        BrushKind::Eraser => body.child(eraser_mode_row(board)),
        BrushKind::Pencil | BrushKind::Marker | BrushKind::Highlighter | BrushKind::Custom(_) => {
            body
        }
    };
    body
}

fn color_row(board: &Arc<Mutex<Board>>) -> impl IntoElement {
    let current = lock(board).current_color();
    let mut row = rect().horizontal().spacing(6.0);
    for swatch in SWATCHES {
        let is_active = *swatch == current;
        let press_board = Arc::clone(board);
        let colour = *swatch;
        let outline: Option<Border> = if is_active {
            Some(
                Border::new()
                    .fill(Color::WHITE)
                    .width(2.0)
                    .alignment(BorderAlignment::Inner),
            )
        } else {
            None
        };
        row = row.child(
            rect()
                .width(Size::px(20.0))
                .height(Size::px(20.0))
                .background(Color::from_rgb(colour[0], colour[1], colour[2]))
                .with_corner_radius(4.0)
                .border(outline)
                .on_press(move |_| {
                    lock(&press_board).set_current_color(colour);
                }),
        );
    }
    row
}

fn size_row(board: &Arc<Mutex<Board>>) -> impl IntoElement {
    let cur = lock(board).current_size();
    let label_text = format!("Size {cur:.2}\u{00d7}");
    let dec_board = Arc::clone(board);
    let inc_board = Arc::clone(board);

    let minus = pill_button("\u{2212}", move |_| {
        let mut g = lock(&dec_board);
        let next = (g.current_size() - 0.25).max(0.25);
        g.set_current_size(next);
    });
    let plus = pill_button("+", move |_| {
        let mut g = lock(&inc_board);
        let next = (g.current_size() + 0.25).min(4.0);
        g.set_current_size(next);
    });
    rect()
        .horizontal()
        .spacing(6.0)
        .child(label().color(Color::WHITE).font_size(13.0).text(label_text))
        .child(minus)
        .child(plus)
}

fn pen_curve_row(board: &Arc<Mutex<Board>>) -> impl IntoElement {
    let cfg = lock(board).brush_config(BrushKind::Pen);
    let current = match cfg {
        BrushConfig::Pen { curve } => curve,
        _ => PressureCurve::Linear,
    };
    let mut row = rect()
        .horizontal()
        .spacing(6.0)
        .child(label().color(Color::WHITE).font_size(13.0).text("Curve"));
    for (curve, name) in [
        (PressureCurve::Linear, "Linear"),
        (PressureCurve::Soft, "Soft"),
        (PressureCurve::Hard, "Hard"),
    ] {
        let selected = curve == current;
        let press_board = Arc::clone(board);
        row = row.child(mode_button(name, selected, move |_| {
            lock(&press_board).set_brush_config(BrushKind::Pen, BrushConfig::Pen { curve });
        }));
    }
    row
}

fn eraser_mode_row(board: &Arc<Mutex<Board>>) -> impl IntoElement {
    let cfg = lock(board).brush_config(BrushKind::Eraser);
    let current = match cfg {
        BrushConfig::Eraser { mode } => mode,
        _ => EraserMode::Point,
    };
    let mut row = rect()
        .horizontal()
        .spacing(6.0)
        .child(label().color(Color::WHITE).font_size(13.0).text("Mode"));
    for (mode, name) in [
        (EraserMode::Point, "Point"),
        (EraserMode::Stroke, "Stroke"),
        (EraserMode::SelectionRect, "Rect"),
    ] {
        let selected = mode == current;
        let press_board = Arc::clone(board);
        row = row.child(mode_button(name, selected, move |_| {
            lock(&press_board).set_brush_config(BrushKind::Eraser, BrushConfig::Eraser { mode });
        }));
    }
    row
}

fn pill_button(
    text: &str,
    handler: impl FnMut(Event<PressEventData>) + 'static,
) -> impl IntoElement {
    rect()
        .padding((4.0, 8.0))
        .background(Color::from_rgb(70, 70, 78))
        .with_corner_radius(4.0)
        .on_press(handler)
        .child(
            label()
                .color(Color::WHITE)
                .font_size(13.0)
                .text(text.to_string()),
        )
}

fn mode_button(
    text: &str,
    selected: bool,
    handler: impl FnMut(Event<PressEventData>) + 'static,
) -> impl IntoElement {
    let (bg, fg) = if selected {
        (Color::from_rgb(90, 130, 220), Color::WHITE)
    } else {
        (Color::from_rgb(60, 60, 66), Color::from_rgb(210, 210, 214))
    };
    rect()
        .padding((4.0, 8.0))
        .background(bg)
        .with_corner_radius(4.0)
        .on_press(handler)
        .child(label().color(fg).font_size(13.0).text(text.to_string()))
}
