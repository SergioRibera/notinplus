//! Bottom chapter nav — pill with `<` + `BOOK N` + `>` + a floating
//! play circle to its left.
//!
//! The host passes current book name + chapter + three callbacks. All
//! state (which book, which chapter, whether audio is playing) lives
//! on the viewer state, not here.

use freya::prelude::*;

use super::theme;

const BAR_HEIGHT: f32 = 72.0;
const PILL_HEIGHT: f32 = 48.0;
const PLAY_SIZE: f32 = 52.0;

#[allow(missing_debug_implementations)]
pub struct BottomNavCallbacks<FP, FN, FA>
where
    FP: FnMut() + 'static,
    FN: FnMut() + 'static,
    FA: FnMut() + 'static,
{
    pub on_prev: FP,
    pub on_next: FN,
    pub on_play: FA,
}

pub fn bottom_nav<FP, FN, FA>(
    book_name: &str,
    chapter: u16,
    callbacks: BottomNavCallbacks<FP, FN, FA>,
) -> impl IntoElement
where
    FP: FnMut() + 'static,
    FN: FnMut() + 'static,
    FA: FnMut() + 'static,
{
    let BottomNavCallbacks { on_prev, on_next, on_play } = callbacks;
    let caption = format!("{} {}", book_name.to_uppercase(), chapter);

    rect()
        .width(Size::fill())
        .height(Size::px(BAR_HEIGHT))
        .background(parse_rgb(theme::BG))
        .padding((0., 16.))
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .main_align(Alignment::Center)
        .spacing(12.0)
        .child(play_button(on_play))
        .child(chapter_pill(caption, on_prev, on_next))
}

fn play_button<F>(mut on_press: F) -> impl IntoElement
where
    F: FnMut() + 'static,
{
    rect()
        .width(Size::px(PLAY_SIZE))
        .height(Size::px(PLAY_SIZE))
        .center()
        .with_corner_radius(PLAY_SIZE / 2.0)
        .background(parse_rgb(theme::CHIP_BG))
        .on_press(move |_: Event<PressEventData>| on_press())
        .child(
            SvgViewer::new(freya::icons::lucide::play())
                .color(parse_rgb(theme::TEXT_PRIMARY))
                .width(Size::px(20.0))
                .height(Size::px(20.0)),
        )
}

fn chapter_pill<FP, FN>(
    caption: String,
    mut on_prev: FP,
    mut on_next: FN,
) -> impl IntoElement
where
    FP: FnMut() + 'static,
    FN: FnMut() + 'static,
{
    rect()
        .height(Size::px(PILL_HEIGHT))
        .min_width(Size::px(220.0))
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .main_align(Alignment::SpaceBetween)
        .padding((0., 10.))
        .with_corner_radius(PILL_HEIGHT / 2.0)
        .background(parse_rgb(theme::CHIP_BG))
        .child(
            rect()
                .width(Size::px(36.0))
                .height(Size::px(36.0))
                .center()
                .on_press(move |_: Event<PressEventData>| on_prev())
                .child(
                    SvgViewer::new(freya::icons::lucide::chevron_left())
                        .color(parse_rgb(theme::TEXT_PRIMARY))
                        .width(Size::px(18.0))
                        .height(Size::px(18.0)),
                ),
        )
        .child(
            label()
                .font_size(14.0)
                .color(parse_rgb(theme::TEXT_PRIMARY))
                .text(caption),
        )
        .child(
            rect()
                .width(Size::px(36.0))
                .height(Size::px(36.0))
                .center()
                .on_press(move |_: Event<PressEventData>| on_next())
                .child(
                    SvgViewer::new(freya::icons::lucide::chevron_right())
                        .color(parse_rgb(theme::TEXT_PRIMARY))
                        .width(Size::px(18.0))
                        .height(Size::px(18.0)),
                ),
        )
}

fn parse_rgb(s: &str) -> Color {
    let trimmed = s.trim_start_matches("rgb(").trim_end_matches(')');
    let parts: Vec<u8> = trimmed
        .split(',')
        .filter_map(|t| t.trim().parse::<u8>().ok())
        .collect();
    if parts.len() == 3 {
        Color::from_rgb(parts[0], parts[1], parts[2])
    } else {
        Color::WHITE
    }
}
