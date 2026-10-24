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
use crate::bookmark::{SourceRef, WorldPoint};
use crate::bookmark_ui::now_ms;
use crate::canvas::{InputMode, lock};
use crate::library::ItemId;

use notein_plugin::{SelectMode, Source as _};

/// `true` when the shared board for `item` is currently in Select
/// mode. The overlay is always mounted (so pointer handlers can wake
/// Freya re-renders on mode change) but each handler short-circuits
/// here when the user is in Draw / Pan — strokes stay owned by the
/// pen pump.
fn in_select_mode(mount: CanvasMount) -> bool {
    active_boards::get(mount)
        .map(|b| lock(&b).input_mode())
        .is_some_and(|m| matches!(m, InputMode::Select))
}

/// Dwell window within which a second pointer_down counts as a
/// double-click and promotes the selection to word granularity.
const DBL_CLICK_WINDOW: Duration = Duration::from_millis(350);

/// Translucent highlight colour painted over selected rects.
const HIGHLIGHT_RGBA: (u8, u8, u8, u8) = (90, 155, 255, 90);
/// Pulse highlight colour painted on top of the selection colour
/// while a chip-tap navigation is still within the pulse window.
/// Warmer tone so the user visually distinguishes "a chip just landed
/// me here" from "I'm actively selecting".
const PULSE_RGBA: (u8, u8, u8, u8) = (255, 193, 86, 150);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PdfSelectOverlay {
    pub item: ItemId,
    /// Board this overlay drives (the pane it is mounted in).
    pub(crate) mount: CanvasMount,
}

impl Component for PdfSelectOverlay {
    fn render(&self) -> impl IntoElement {
        let item = self.item;
        let mount = self.mount;
        let mut dragging = use_state(|| false);
        let mut last_down_at = use_state(|| Option::<Instant>::None);
        // Bumping this signal forces the paint child to re-render.
        let mut paint_tick = use_state(|| 0u64);

        use_hook(move || {
            let _ = active_pdf_sources::ensure_registered(item);
        });

        let on_pointer_down = move |evt: Event<PointerEventData>| {
            if !in_select_mode(mount) {
                return;
            }
            let Some(handle) = active_pdf_sources::ensure_registered(item) else {
                return;
            };
            let Some(board) = active_boards::get(mount) else {
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
            let Some(board) = active_boards::get(mount) else {
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
        let on_pointer_up = move |evt: Event<PointerEventData>| {
            if !*dragging.read() {
                return;
            }
            let has_selection = if let Some(handle) = active_pdf_sources::get(item) {
                handle.plugin.pointer_end_select(handle.instance);
                handle
                    .plugin
                    .selection(handle.instance)
                    .is_some_and(|s| !s.is_empty())
            } else {
                false
            };
            dragging.set(false);
            if has_selection {
                let at = evt.global_location();
                #[allow(clippy::cast_possible_truncation)] // window coordinates
                show_bar(item, mount, (at.x as f32, at.y as f32));
            }
            let cur = *paint_tick.read();
            paint_tick.set(cur.wrapping_add(1));
        };

        let interactive = in_select_mode(mount);
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .layer(Layer::Overlay)
            .position(Position::new_absolute().top(0.0).left(0.0))
            .interactive(interactive)
            .on_pointer_down(on_pointer_down)
            .on_pointer_move(on_pointer_move)
            .on_global_pointer_up(on_pointer_up)
            .child(selection_paint(item, mount, paint_tick))
    }
}

/// The selection bar for what is selected in the PDF of `item`: the
/// five highlighters, a note, copy as reference, to the notebook and a
/// plain bookmark.
pub(crate) fn show_bar(item: ItemId, mount: CanvasMount, at: (f32, f32)) {
    use notein_plugin::payload::Payload;
    let Some(handle) = active_pdf_sources::get(item) else {
        return;
    };
    let Some(sel) = handle.plugin.selection(handle.instance).map(|s| s.normalized()) else {
        return;
    };
    let snippet = handle
        .plugin
        .current_selection(handle.instance)
        .map(|s| s.snippet)
        .unwrap_or_default();
    let world = handle
        .plugin
        .selection_world_rects(handle.instance)
        .first()
        .map_or(WorldPoint::new(0.0, 0.0), |r| {
            WorldPoint::new((r.min_x + r.max_x) / 2.0, r.min_y)
        });
    let anchor = crate::plugins::builtin::pdf::PdfAnchor {
        page: sel.start_page,
        text: (!snippet.is_empty()).then(|| snippet.clone()),
        range: (sel.start_page == sel.end_page).then_some((sel.start_char, sel.end_char)),
    };
    let reference = SourceRef::Plugin {
        plugin_id: crate::plugins::builtin::pdf::KIND.to_owned(),
        item: item.0,
        anchor: anchor.to_bytes(),
        selection: (!snippet.is_empty()).then(|| snippet.clone()),
    };
    let label = crate::selection::label_of(&reference);
    let mark = {
        let reference = reference.clone();
        std::rc::Rc::new(move |color: Option<[u8; 3]>, body: String| {
            let Some(board) = active_boards::get(mount) else {
                return;
            };
            let now = now_ms();
            let mut guard = lock(&board);
            let id = guard.add_bookmark_full(world, 0.0, now, body.clone(), vec![reference.clone()]);
            if color.is_some() {
                guard.update_bookmark(id, body, vec![reference.clone()], color.map(|c| [c[0], c[1], c[2], 0xFF]), now);
            }
            drop(guard);
            clear(item);
        })
    };
    let (color_mark, note_mark, plain_mark) = (mark.clone(), mark.clone(), mark);
    let quote = snippet.clone();
    crate::selection::SelectionMenu {
        at,
        active: None,
        on_color: std::rc::Rc::new(move |rgb| color_mark(rgb, String::new())),
        on_note: std::rc::Rc::new(move || {
            let (tx, rx) = flume::bounded::<notein_plugin::services::AnnotationOutcome>(1);
            let note_mark = note_mark.clone();
            spawn_forever(async move {
                match rx.recv_async().await {
                    Ok(notein_plugin::services::AnnotationOutcome::Saved { color, body }) => note_mark(color, body),
                    Ok(_) | Err(_) => clear(item),
                }
            });
            crate::annotation::AnnotationPopover::open(
                notein_plugin::services::AnnotationRequest {
                    doc: crate::plugins::content::doc_id(item),
                    kind: crate::plugins::builtin::pdf::KIND.to_owned(),
                    title: format!("Página {}", sel.start_page + 1),
                    quote: (!quote.is_empty()).then(|| quote.clone()),
                    color: None,
                    body: String::new(),
                    target: Some((at.0, at.1, 1.0, 1.0)),
                    existing: false,
                },
                None,
                std::sync::Arc::new(move |outcome| {
                    let _ = tx.send(outcome);
                }),
            );
        }),
        reference: Some((reference, label)),
        document: Some(item),
        extra: vec![(
            "Marcador",
            freya::icons::lucide::bookmark(),
            std::rc::Rc::new(move || plain_mark(None, String::new())),
        )],
        on_dismiss: std::rc::Rc::new(move || clear(item)),
    }
    .show();
}

fn clear(item: ItemId) {
    if let Some(handle) = active_pdf_sources::get(item) {
        handle.plugin.clear_selection(handle.instance);
    }
    if let Some(board) = active_boards::get_item(item) {
        lock(&board).request_repaint();
    }
}

/// Long-press on a page: select the word under the finger and offer
/// the selection bar. `false` when the press was not on text.
pub(crate) fn select_word_at(item: ItemId, mount: CanvasMount, world: (f32, f32), window: (f32, f32)) -> bool {
    let Some(handle) = active_pdf_sources::ensure_registered(item) else {
        return false;
    };
    if !handle.plugin.pointer_begin_select(handle.instance, world.0, world.1, SelectMode::Word) {
        return false;
    }
    handle.plugin.pointer_end_select(handle.instance);
    let selected = handle
        .plugin
        .selection(handle.instance)
        .is_some_and(|s| !s.is_empty());
    if selected {
        show_bar(item, mount, window);
    }
    selected
}

/// Freya canvas whose paint callback reads the plugin's selection
/// world-space rects + maps them through the current viewport to
/// paint translucent highlights.
fn selection_paint(item: ItemId, mount: CanvasMount, tick: State<u64>) -> impl IntoElement {
    // Re-read triggers a Freya re-run whenever paint_tick bumps.
    let _ = *tick.read();
    canvas(RenderCallback::new(move |ctx| {
        let Some(handle) = active_pdf_sources::get(item) else {
            return;
        };
        let Some(board) = active_boards::get(mount) else {
            return;
        };
        let guard = lock(&board);

        // 1. Selection colour (user dragged highlight).
        let sel_rects = handle.plugin.selection_world_rects(handle.instance);
        if !sel_rects.is_empty() {
            let mut paint = Paint::default();
            paint.set_color(SkColor::from_argb(
                HIGHLIGHT_RGBA.3,
                HIGHLIGHT_RGBA.0,
                HIGHLIGHT_RGBA.1,
                HIGHLIGHT_RGBA.2,
            ));
            paint.set_style(PaintStyle::Fill);
            paint.set_anti_alias(true);
            for r in sel_rects {
                let (sx0, sy0) = guard.world_to_screen(r.min_x, r.min_y);
                let (sx1, sy1) = guard.world_to_screen(r.max_x, r.max_y);
                ctx.canvas.draw_rect(SkRect::new(sx0, sy0, sx1, sy1), &paint);
            }
        }

        // 2. Pulse highlight (chip just navigated here). Rendered flat
        // (no fade tween — we don't own a continuous redraw loop); the
        // paint disappears on the next natural redraw once
        // `active_pulse` reaps past `PULSE_DURATION`.
        if let Some(pulse) = active_pdf_sources::active_pulse(item) {
            let rects = handle
                .plugin
                .anchor_world_rects(handle.instance, &pulse.anchor);
            if !rects.is_empty() {
                let mut paint = Paint::default();
                paint.set_color(SkColor::from_argb(
                    PULSE_RGBA.3,
                    PULSE_RGBA.0,
                    PULSE_RGBA.1,
                    PULSE_RGBA.2,
                ));
                paint.set_style(PaintStyle::Fill);
                paint.set_anti_alias(true);
                for r in rects {
                    let (sx0, sy0) = guard.world_to_screen(r.min_x, r.min_y);
                    let (sx1, sy1) = guard.world_to_screen(r.max_x, r.max_y);
                    ctx.canvas.draw_rect(SkRect::new(sx0, sy0, sx1, sy1), &paint);
                }
            }
        }
    }))
    .width(Size::fill())
    .height(Size::fill())
}
