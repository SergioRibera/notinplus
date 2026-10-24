//! Bottom chapter bar: a pill with `‹ BOOK N ›` whose caption opens the
//! book picker, and back / forward through the passages visited at the
//! ends.

use freya::icons::lucide;
use freya::prelude::*;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{IconButton, tokens};

const BAR_HEIGHT: f32 = 72.0;
const PILL_HEIGHT: f32 = 48.0;

/// A press handler of the bar.
pub type NavHandler = Box<dyn FnMut() + 'static>;

#[allow(missing_debug_implementations)] // closures
pub struct BottomNavCallbacks<FP, FN, FC>
where
    FP: FnMut() + 'static,
    FN: FnMut() + 'static,
    FC: FnMut() + 'static,
{
    pub on_prev: FP,
    pub on_next: FN,
    /// The caption was pressed: open the book and chapter picker.
    pub on_open_picker: FC,
    /// Back to the passage before; `None` hides the button.
    pub on_back: Option<NavHandler>,
    /// Forward again; `None` hides the button.
    pub on_forward: Option<NavHandler>,
}

pub fn bottom_nav<FP, FN, FC>(
    book_name: &str,
    chapter: u16,
    callbacks: BottomNavCallbacks<FP, FN, FC>,
) -> impl IntoElement
where
    FP: FnMut() + 'static,
    FN: FnMut() + 'static,
    FC: FnMut() + 'static,
{
    let BottomNavCallbacks {
        mut on_prev,
        mut on_next,
        mut on_open_picker,
        on_back,
        on_forward,
    } = callbacks;
    let caption = format!("{} {}", book_name.to_uppercase(), chapter);
    let pill = rect()
        .height(Size::px(PILL_HEIGHT))
        .width(Size::percent(100.))
        .max_width(Size::px(240.0))
        .horizontal()
        .content(Content::Flex)
        .cross_align(Alignment::Center)
        .padding((0., 6.))
        .corner_radius(PILL_HEIGHT / 2.0)
        .background(palette().s2)
        .child(
            IconButton::new(lucide::chevron_left(), "Capítulo anterior")
                .on_press(move |_| on_prev()),
        )
        .child(
            rect()
                .width(Size::flex(1.))
                .padding((4., 6.))
                .on_press(move |_: Event<PressEventData>| on_open_picker())
                .child(
                    label()
                        .width(Size::fill())
                        .font_size(14.0)
                        .text_align(TextAlign::Center)
                        .text_overflow(TextOverflow::Ellipsis)
                        .max_lines(1)
                        .color(palette().t1)
                        .text(caption),
                ),
        )
        .child(
            IconButton::new(lucide::chevron_right(), "Capítulo siguiente")
                .on_press(move |_| on_next()),
        );
    rect()
        .width(Size::fill())
        .height(Size::px(BAR_HEIGHT))
        .background(palette().bg)
        .padding((0., 12.))
        .horizontal()
        .content(Content::Flex)
        .cross_align(Alignment::Center)
        .child(history_button(lucide::undo_2(), "Pasaje anterior", on_back))
        .child(
            rect()
                .width(Size::flex(1.))
                .main_align(Alignment::Center)
                .horizontal()
                .child(pill),
        )
        .child(history_button(lucide::redo_2(), "Pasaje siguiente", on_forward))
}

/// A history button, or an empty slot of its size so the pill stays
/// centred.
fn history_button(icon: Bytes, label: &'static str, handler: Option<NavHandler>) -> Element {
    handler.map_or_else(
        || {
            rect()
                .width(Size::px(tokens::size::BUTTON))
                .height(Size::px(tokens::size::BUTTON))
                .into_element()
        },
        |mut handler| {
            IconButton::new(icon, label)
                .on_press(move |_| handler())
                .into_element()
        },
    )
}
