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
use crate::active_pdf_sources::{self, PdfSourceHandle};
use crate::app::CanvasMount;
use crate::bookmark::{SourceRef, WorldPoint};
use crate::bookmark_ui::now_ms;
use crate::canvas::{InputMode, lock};
use crate::ids::BookmarkId;
use crate::library::ItemId;
use crate::pdf_viewer::PDF_PLUGIN_ID;

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
/// Pulse highlight colour painted on top of the selection colour
/// while a chip-tap navigation is still within the pulse window.
/// Warmer tone so the user visually distinguishes "a chip just landed
/// me here" from "I'm actively selecting".
const PULSE_RGBA: (u8, u8, u8, u8) = (255, 193, 86, 150);

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
        // Context menu toggles after a drag completes with a non-empty
        // selection. Clicking anywhere else — or starting a new drag —
        // hides it again.
        let mut menu_visible = use_state(|| false);

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
                menu_visible.set(false);
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
            menu_visible.set(has_selection);
            let cur = *paint_tick.read();
            paint_tick.set(cur.wrapping_add(1));
        };

        let interactive = in_select_mode(item);
        let show_menu = *menu_visible.read();
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
            .child(if show_menu {
                selection_menu(item, menu_visible).into_element()
            } else {
                rect().width(Size::px(0.0)).height(Size::px(0.0)).into_element()
            })
    }
}

/// Floating action card shown after the user finishes a selection.
/// Currently exposes a single "Crear bookmark" action; the clipboard
/// "Copiar como ref" flow lands with Phase 6 once the `[[…]]` body
/// chip format is settled.
fn selection_menu(item: ItemId, mut visible: State<bool>) -> impl IntoElement {
    let on_create = move |_: Event<PressEventData>| {
        let created = active_pdf_sources::get(item)
            .and_then(|h| create_bookmark_from_selection(item, &h));
        if created.is_some() {
            if let Some(handle) = active_pdf_sources::get(item) {
                handle.plugin.clear_selection(handle.instance);
            }
            visible.set(false);
        } else {
            log::warn!("create bookmark from selection failed for {item:?}");
        }
    };

    rect()
        .layer(Layer::Overlay)
        .position(
            Position::new_absolute()
                .bottom(24.0)
                .right(24.0),
        )
        .padding(Gaps::new(8.0, 12.0, 8.0, 12.0))
        .corner_radius(CornerRadius::new_all(8.0))
        .background(Color::from_argb(240, 32, 32, 36))
        .shadow(Shadow::from((
            0.0_f32,
            6.0_f32,
            18.0_f32,
            0.0_f32,
            Color::from_argb(120, 0, 0, 0),
        )))
        .child(
            rect()
                .horizontal()
                .spacing(8.0)
                .cross_align(Alignment::Center)
                .child(
                    rect()
                        .padding(Gaps::new(6.0, 10.0, 6.0, 10.0))
                        .corner_radius(CornerRadius::new_all(6.0))
                        .background(Color::from_argb(255, 90, 155, 255))
                        .on_press(on_create)
                        .child(
                            label()
                                .color(Color::from_rgb(255, 255, 255))
                                .font_size(13.0)
                                .text("Crear bookmark"),
                        ),
                ),
        )
}

/// Dispatch "create bookmark from current selection":
/// - Pulls `Selection { anchor, snippet }` from the plugin.
/// - Targets the board mounted for the same item.
/// - Emits a single [`crate::doc_op::DocOp::AddBookmark`] carrying the
///   snippet in the body and a `SourceRef::Plugin` pointing back at the
///   PDF anchor.
fn create_bookmark_from_selection(item: ItemId, handle: &PdfSourceHandle) -> Option<BookmarkId> {
    let selection = handle.plugin.current_selection(handle.instance)?;
    let target = active_boards::get(CanvasMount::Item(item))?;
    let now = now_ms();
    let source_ref = SourceRef::Plugin {
        plugin_id: PDF_PLUGIN_ID.to_owned(),
        item: item.0,
        anchor: selection.anchor.clone(),
        selection: Some(selection.snippet.clone()),
    };
    let mut guard = lock(&target);
    // Drop the bookmark at the target viewport's current centre so the
    // pin is visible after creation. `world_point_at_viewport_centre`
    // defaults to (0, 0) when the surface rect hasn't been measured
    // yet — still better than silently stashing offscreen.
    let world = WorldPoint::new(0.0, 0.0);
    Some(guard.add_bookmark_full(
        world,
        0.0,
        now,
        selection.snippet,
        vec![source_ref],
    ))
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
