//! Pin UI — long-press to create, floating card to view / edit.
//!
//! Phase 1.5 of `SOURCES_PLAN`. Scope limited to body-only editing;
//! rich `[[…]]` refs chip parsing + source anchors land in Phase 1.6+.
//!
//! The gesture layer that produces long-press events lives inside
//! [`crate::canvas::drawing_surface`] because it must sit on the same
//! pointer stack the viewport gestures already own — layering an
//! overlay rect for gestures would eat drawing input. The card
//! overlay here is a sibling of the drawing surface, rendered only
//! when a bookmark is selected.

use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use freya::prelude::*;

use crate::bookmark::TimestampMs;
use crate::canvas::{Board, lock};
use crate::components::FormInput;
use crate::ids::BookmarkId;

/// Card width in logical pixels. Narrow enough to not swamp a mobile
/// landscape canvas; wide enough for a two-line body preview.
const CARD_WIDTH: f32 = 240.0;
/// Vertical gap from the pin to the card so the pin stays visible.
const CARD_GAP: f32 = 20.0;

/// Floating card overlay anchored to the selected bookmark. Reads +
/// writes bookmark body through the shared [`Board`]; closing clears
/// `selected`.
#[must_use]
pub fn bookmark_card_overlay(
    board: &Arc<Mutex<Board>>,
    mut selected: State<Option<BookmarkId>>,
    body_buffer: State<String>,
) -> impl IntoElement {
    let Some(selected_id) = *selected.read() else {
        // Nothing selected — render a zero-area placeholder so the
        // overlay slot in `app::root` always has a child regardless
        // of state.
        return rect().width(Size::px(0.0)).height(Size::px(0.0));
    };

    let (card_x, card_y, present) = {
        let guard = lock(board);
        match guard.doc().bookmark(selected_id) {
            Some(bm) => {
                let world = guard.bookmark_world_position(bm);
                let (sx, sy) = guard.viewport().world_to_screen(world.x, world.y);
                (sx, sy, true)
            }
            None => (0.0_f32, 0.0_f32, false),
        }
    };
    if !present {
        // Selected bookmark vanished (deleted elsewhere, remote
        // tombstone, etc.) — close the card next tick.
        selected.set(None);
        return rect().width(Size::px(0.0)).height(Size::px(0.0));
    }

    let save_board = Arc::clone(board);
    let save_body = body_buffer;
    let mut save_selected = selected;
    let on_save = move |_: Event<PressEventData>| {
        let Some(id) = *save_selected.peek() else {
            return;
        };
        let body = save_body.peek().clone();
        let now = now_ms();
        lock(&save_board).update_bookmark(id, body, Vec::new(), None, now);
        save_selected.set(None);
    };

    let delete_board = Arc::clone(board);
    let mut delete_selected = selected;
    let on_delete = move |_: Event<PressEventData>| {
        let Some(id) = *delete_selected.peek() else {
            return;
        };
        lock(&delete_board).delete_bookmark(id);
        delete_selected.set(None);
    };

    let mut close_selected = selected;
    let on_close = move |_: Event<PressEventData>| {
        close_selected.set(None);
    };

    // Position the card below the pin, nudged right so the top-left
    // corner lands roughly on the pin baseline. Clamping to the
    // surface is left to Phase 1.6 — the current overlay stays
    // visible under any realistic zoom.
    let left = (card_x - CARD_WIDTH * 0.5).max(8.0);
    let top = card_y + CARD_GAP;

    rect()
        .position(Position::new_global().top(top).left(left))
        .layer(Layer::Overlay)
        .width(Size::px(CARD_WIDTH))
        .padding((12.0, 12.0))
        .spacing(10.0)
        .background(Color::from_rgb(28, 28, 32))
        .with_corner_radius(10.0)
        .child(FormInput::new(body_buffer).placeholder("Nota"))
        .child(
            rect()
                .direction(Direction::Horizontal)
                .main_align(Alignment::SpaceBetween)
                .cross_align(Alignment::Center)
                .width(Size::fill())
                .spacing(8.0)
                .child(
                    rect()
                        .padding((6.0, 10.0))
                        .background(Color::from_rgb(60, 60, 68))
                        .with_corner_radius(6.0)
                        .on_press(on_close)
                        .child("Cerrar"),
                )
                .child(
                    rect()
                        .padding((6.0, 10.0))
                        .background(Color::from_rgb(140, 32, 32))
                        .with_corner_radius(6.0)
                        .on_press(on_delete)
                        .child("Borrar"),
                )
                .child(
                    rect()
                        .padding((6.0, 10.0))
                        .background(Color::from_rgb(32, 120, 72))
                        .with_corner_radius(6.0)
                        .on_press(on_save)
                        .child("Guardar"),
                ),
        )
}

/// Wall-clock ms since Unix epoch. Display-only — merge order is
/// decided by the op's lamport stamp, not by this value.
pub fn now_ms() -> TimestampMs {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}
