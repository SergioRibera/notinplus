//! Text selection over a PDF face: drag with the pen or mouse in
//! "Seleccionar texto", or long-press a word, then act on it from the
//! selection bar.

use std::time::{Duration, Instant};

use freya::prelude::*;
use freya_engine::prelude::{Color as SkColor, Paint, PaintStyle, Rect as SkRect};

use crate::active_boards;
use crate::app::CanvasMount;
use crate::bookmark::{SourceRef, WorldPoint};
use crate::canvas::{InputMode, lock};
use crate::format::now as now_ms;
use crate::library::ItemId;
use crate::pdf_text::{PdfText, SelectMode};

fn in_select_mode(mount: CanvasMount) -> bool {
    active_boards::get(mount)
        .map(|b| lock(&b).input_mode())
        .is_some_and(|m| matches!(m, InputMode::Select))
}

/// A second press within this long selects whole words.
const DBL_CLICK_WINDOW: Duration = Duration::from_millis(350);

/// Selection wash over the text.
const HIGHLIGHT_RGBA: (u8, u8, u8, u8) = (90, 155, 255, 90);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
        let mut paint_tick = use_state(|| 0u64);
        use_drop(move || PdfText::get().forget(item));

        let world_of = move |evt: &Event<PointerEventData>| {
            let board = active_boards::get(mount)?;
            let el = evt.element_location();
            #[allow(clippy::cast_possible_truncation)] // surface pixels
            Some(lock(&board).screen_to_world(el.x as f32, el.y as f32))
        };
        let on_pointer_down = move |evt: Event<PointerEventData>| {
            if !in_select_mode(mount) {
                return;
            }
            let Some((wx, wy)) = world_of(&evt) else {
                return;
            };
            let now = Instant::now();
            let mode = match *last_down_at.read() {
                Some(prev) if now.duration_since(prev) < DBL_CLICK_WINDOW => SelectMode::Word,
                _ => SelectMode::Point,
            };
            last_down_at.set(Some(now));
            if PdfText::get().begin(item, wx, wy, mode) {
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
            let Some((wx, wy)) = world_of(&evt) else {
                return;
            };
            PdfText::get().extend(item, wx, wy);
            let cur = *paint_tick.read();
            paint_tick.set(cur.wrapping_add(1));
        };
        let on_pointer_up = move |evt: Event<PointerEventData>| {
            if !*dragging.read() {
                return;
            }
            PdfText::get().end(item);
            dragging.set(false);
            if PdfText::get().selection(item).is_some() {
                let at = evt.global_location();
                #[allow(clippy::cast_possible_truncation)] // window coordinates
                show_bar(item, mount, (at.x as f32, at.y as f32));
            }
            let cur = *paint_tick.read();
            paint_tick.set(cur.wrapping_add(1));
        };
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .layer(Layer::Overlay)
            .position(Position::new_absolute().top(0.0).left(0.0))
            .interactive(in_select_mode(mount))
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
    let engine = PdfText::get();
    let Some(sel) = engine.selection(item) else {
        return;
    };
    let snippet = engine.selected_text(item).unwrap_or_default();
    let world = engine
        .selection_rects(item)
        .first()
        .map_or(WorldPoint::new(0.0, 0.0), |r| {
            WorldPoint::new(f32::midpoint(r.min_x, r.max_x), r.min_y)
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
    let mark = {
        let reference = reference.clone();
        std::rc::Rc::new(move |color: Option<[u8; 3]>, body: String| {
            let Some(board) = active_boards::get(mount) else {
                return;
            };
            let now = now_ms();
            let mut guard = lock(&board);
            let id =
                guard.add_bookmark_full(world, 0.0, now, body.clone(), vec![reference.clone()]);
            if color.is_some() {
                guard.update_bookmark(
                    id,
                    body,
                    vec![reference.clone()],
                    color.map(|c| [c[0], c[1], c[2], 0xFF]),
                    now,
                );
            }
            drop(guard);
            clear(item);
        })
    };
    let (color_mark, note_mark, plain_mark) = (mark.clone(), mark.clone(), mark);
    let quote = snippet;
    crate::selection::show(notein_plugin::ui::SelectionMenu {
        at,
        active: None,
        on_color: std::rc::Rc::new(move |rgb| color_mark(rgb, String::new())),
        on_note: Some(std::rc::Rc::new(move || {
            let (tx, rx) = flume::bounded::<notein_plugin::services::AnnotationOutcome>(1);
            let note_mark = note_mark.clone();
            spawn_in_window(async move {
                match rx.recv_async().await {
                    Ok(notein_plugin::services::AnnotationOutcome::Saved { color, body }) => {
                        note_mark(color, body);
                    }
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
        })),
        reference: Some(reference),
        document: Some(crate::plugins::content::doc_id(item)),
        extra: vec![(
            "Marcador",
            freya::icons::lucide::bookmark(),
            std::rc::Rc::new(move || plain_mark(None, String::new())),
        )],
        caption: None,
        sticky: false,
        on_dismiss: std::rc::Rc::new(move || clear(item)),
    });
}

fn clear(item: ItemId) {
    PdfText::get().clear(item);
    if let Some(board) = active_boards::get_item(item) {
        lock(&board).request_repaint();
    }
}

/// Long-press on a page: select the word under the finger and offer
/// the selection bar. `false` when the press was not on text.
pub(crate) fn select_word_at(
    item: ItemId,
    mount: CanvasMount,
    world: (f32, f32),
    window: (f32, f32),
) -> bool {
    let engine = PdfText::get();
    if !engine.begin(item, world.0, world.1, SelectMode::Word) {
        return false;
    }
    engine.end(item);
    let selected = engine.selection(item).is_some();
    if selected {
        show_bar(item, mount, window);
    }
    selected
}

/// The selection wash, mapped through the board's viewport.
fn selection_paint(item: ItemId, mount: CanvasMount, tick: State<u64>) -> impl IntoElement {
    let _ = *tick.read();
    rect()
        .background(RenderCallback::new(move |ctx| {
            let RenderTarget::Background { canvas, .. } = &ctx.target else {
                return;
            };
            let rects = PdfText::get().selection_rects(item);
            if rects.is_empty() {
                return;
            }
            let Some(board) = active_boards::get(mount) else {
                return;
            };
            let screen: Vec<SkRect> = {
                let guard = lock(&board);
                rects
                    .iter()
                    .map(|r| {
                        let (sx0, sy0) = guard.world_to_screen(r.min_x, r.min_y);
                        let (sx1, sy1) = guard.world_to_screen(r.max_x, r.max_y);
                        SkRect::new(sx0, sy0, sx1, sy1)
                    })
                    .collect()
            };
            let mut paint = Paint::default();
            paint.set_color(SkColor::from_argb(
                HIGHLIGHT_RGBA.3,
                HIGHLIGHT_RGBA.0,
                HIGHLIGHT_RGBA.1,
                HIGHLIGHT_RGBA.2,
            ));
            paint.set_style(PaintStyle::Fill);
            paint.set_anti_alias(true);
            for r in &screen {
                canvas.draw_rect(r, &paint);
            }
        }))
        .width(Size::fill())
        .height(Size::fill())
}
