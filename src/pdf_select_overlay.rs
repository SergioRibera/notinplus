//! Text-selection overlay for PDF-face mounts.
//!
//! Rendered on top of the stroke canvas by
//! [`crate::builtin::pdf_linked::PdfFace`] whenever the active
//! [`crate::canvas::InputMode`] is
//! [`InputMode::Select`](crate::canvas::InputMode::Select). Captures
//! Freya pointer events + drives the PDF plugin's `pointer_*` API, and
//! paints the current selection as translucent rects over the pages.
//!
//! Pen strokes are untouched because the pump short-circuits on
//! `InputMode::Select` (see [`crate::pen_pump`]); mouse / touch events,
//! which flow through Freya, land here first because this element sits
//! above the canvas.

use std::time::{Duration, Instant};

use freya::prelude::*;
use freya_engine::prelude::{Color as SkColor, Paint, PaintStyle, Rect as SkRect};

use crate::active_boards;
use crate::active_pdf_sources;
use crate::app::CanvasMount;
use crate::canvas::{InputMode, lock};
use crate::library::ItemId;

use notein_plugin::{SelectMode, Source as _};

/// `true` when the shared board for `item` is currently in Select
/// mode. The overlay is always mounted (so pointer handlers can wake
/// Freya re-renders on mode change) but each handler short-circuits
/// here when the user is in Draw / Pan — strokes stay owned by the
/// pen pump.
fn in_select_mode(item: ItemId) -> bool {
    active_boards::get(CanvasMount::Item(item))
        .map(|b| lock(&b).input_mode())
        .is_some_and(|m| matches!(m, InputMode::Select))
}

/// Dwell window within which a second pointer_down counts as a
/// double-click and promotes the selection to word granularity.
const DBL_CLICK_WINDOW: Duration = Duration::from_millis(350);

/// Translucent highlight colour painted over selected rects.
const HIGHLIGHT_RGBA: (u8, u8, u8, u8) = (90, 155, 255, 90);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PdfSelectOverlay {
    pub item: ItemId,
}

impl Component for PdfSelectOverlay {
    fn render(&self) -> impl IntoElement {
        let item = self.item;
        let mut dragging = use_state(|| false);
        let mut last_down_at = use_state(|| Option::<Instant>::None);
        // Bumping this signal forces the paint child to re-render.
        let mut paint_tick = use_state(|| 0u64);

        use_hook(move || {
            let _ = active_pdf_sources::ensure_registered(item);
        });

        let on_pointer_down = move |evt: Event<PointerEventData>| {
            if !in_select_mode(item) {
                return;
            }
            let Some(handle) = active_pdf_sources::ensure_registered(item) else {
                return;
            };
            let Some(board) = active_boards::get(CanvasMount::Item(item)) else {
                return;
            };
            let el = evt.element_location();
            let (world_x, world_y) = lock(&board).screen_to_world(el.x as f32, el.y as f32);

            let now = Instant::now();
            let mode = match *last_down_at.read() {
                Some(prev) if now.duration_since(prev) < DBL_CLICK_WINDOW => SelectMode::Word,
                _ => SelectMode::Point,
            };
            last_down_at.set(Some(now));

            if handle
                .plugin
                .pointer_begin_select(handle.instance, world_x, world_y, mode)
            {
                dragging.set(true);
                let cur = *paint_tick.read();
                paint_tick.set(cur.wrapping_add(1));
                evt.stop_propagation();
            }
        };

        let on_pointer_move = move |evt: Event<PointerEventData>| {
            if !*dragging.read() {
                return;
            }
            let Some(handle) = active_pdf_sources::get(item) else {
                return;
            };
            let Some(board) = active_boards::get(CanvasMount::Item(item)) else {
                return;
            };
            let el = evt.element_location();
            let (wx, wy) = lock(&board).screen_to_world(el.x as f32, el.y as f32);
            handle.plugin.pointer_extend_select(handle.instance, wx, wy);
            let cur = *paint_tick.read();
                paint_tick.set(cur.wrapping_add(1));
        };

        // `global_pointer_up` so a drag that releases outside the overlay
        // bounds still finalises the selection cleanly.
        let on_pointer_up = move |_evt: Event<PointerEventData>| {
            if !*dragging.read() {
                return;
            }
            if let Some(handle) = active_pdf_sources::get(item) {
                handle.plugin.pointer_end_select(handle.instance);
            }
            dragging.set(false);
            let cur = *paint_tick.read();
                paint_tick.set(cur.wrapping_add(1));
        };

        let interactive = in_select_mode(item);
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .layer(Layer::Overlay)
            .position(Position::new_absolute().top(0.0).left(0.0))
            .interactive(interactive)
            .on_pointer_down(on_pointer_down)
            .on_pointer_move(on_pointer_move)
            .on_global_pointer_up(on_pointer_up)
            .child(selection_paint(item, paint_tick))
    }
}

/// Freya canvas whose paint callback reads the plugin's selection
/// world-space rects + maps them through the current viewport to
/// paint translucent highlights.
fn selection_paint(item: ItemId, tick: State<u64>) -> impl IntoElement {
    // Re-read triggers a Freya re-run whenever paint_tick bumps.
    let _ = *tick.read();
    canvas(RenderCallback::new(move |ctx| {
        let Some(handle) = active_pdf_sources::get(item) else {
            return;
        };
        let Some(board) = active_boards::get(CanvasMount::Item(item)) else {
            return;
        };
        let rects = handle.plugin.selection_world_rects(handle.instance);
        if rects.is_empty() {
            return;
        }
        let mut paint = Paint::default();
        paint.set_color(SkColor::from_argb(
            HIGHLIGHT_RGBA.3,
            HIGHLIGHT_RGBA.0,
            HIGHLIGHT_RGBA.1,
            HIGHLIGHT_RGBA.2,
        ));
        paint.set_style(PaintStyle::Fill);
        paint.set_anti_alias(true);
        let guard = lock(&board);
        for r in rects {
            let (sx0, sy0) = guard.world_to_screen(r.min_x, r.min_y);
            let (sx1, sy1) = guard.world_to_screen(r.max_x, r.max_y);
            let rect = SkRect::new(sx0, sy0, sx1, sy1);
            ctx.canvas.draw_rect(rect, &paint);
        }
    }))
    .width(Size::fill())
    .height(Size::fill())
}
