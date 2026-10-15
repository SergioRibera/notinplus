//! Top toolbar — audio / search / more / translation chip.
//!
//! Mirrors the right-aligned stack seen in common Bible-reading apps:
//! three square icon buttons on the left and a pill-shaped translation
//! picker on the right. Callbacks are supplied by the host so the
//! actual behaviour (opening search sheet, popping the translation
//! menu, etc.) stays outside this crate.

use freya::prelude::*;

use super::theme;

const ICON_SIZE: f32 = 20.0;
const BUTTON_SIZE: f32 = 36.0;

/// `(audio, search, more, translation)` click handlers.
#[allow(missing_debug_implementations)]
pub struct ToolbarCallbacks<FA, FS, FM, FT>
where
    FA: FnMut() + Send + 'static,
    FS: FnMut() + Send + 'static,
    FM: FnMut() + Send + 'static,
    FT: FnMut() + Send + 'static,
{
    pub on_audio: FA,
    pub on_search: FS,
    pub on_more: FM,
    pub on_translation: FT,
}

pub fn top_toolbar<FA, FS, FM, FT>(
    translation_abbrev: &str,
    callbacks: ToolbarCallbacks<FA, FS, FM, FT>,
) -> impl IntoElement
where
    FA: FnMut() + Send + 'static,
    FS: FnMut() + Send + 'static,
    FM: FnMut() + Send + 'static,
    FT: FnMut() + Send + 'static,
{
    let ToolbarCallbacks {
        on_audio,
        on_search,
        on_more,
        on_translation,
    } = callbacks;

    rect()
        .width(Size::fill())
        .height(Size::px(56.0))
        .background(parse_rgb(theme::BG))
        .padding((0., 16.))
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .main_align(Alignment::End)
        .spacing(4.0)
        .child(icon_button(freya::icons::lucide::volume_2(), on_audio))
        .child(icon_button(freya::icons::lucide::search(), on_search))
        .child(icon_button(
            freya::icons::lucide::ellipsis(),
            on_more,
        ))
        .child(translation_chip(translation_abbrev, on_translation))
}

fn icon_button<F>(icon: Bytes, mut on_press: F) -> impl IntoElement
where
    F: FnMut() + Send + 'static,
{
    rect()
        .width(Size::px(BUTTON_SIZE))
        .height(Size::px(BUTTON_SIZE))
        .center()
        .with_corner_radius(BUTTON_SIZE / 2.0)
        .on_press(move |_: Event<PressEventData>| on_press())
        .child(
            SvgViewer::new(icon)
                .color(parse_rgb(theme::TEXT_PRIMARY))
                .width(Size::px(ICON_SIZE))
                .height(Size::px(ICON_SIZE)),
        )
}

fn translation_chip<F>(abbrev: &str, mut on_press: F) -> impl IntoElement
where
    F: FnMut() + Send + 'static,
{
    rect()
        .height(Size::px(34.0))
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .padding((0., 12.))
        .spacing(6.0)
        .margin((0., 0., 0., 6.))
        .with_corner_radius(17.0)
        .background(parse_rgb(theme::CHIP_BG))
        .on_press(move |_: Event<PressEventData>| on_press())
        .child(
            SvgViewer::new(freya::icons::lucide::globe())
                .color(parse_rgb(theme::TEXT_PRIMARY))
                .width(Size::px(14.0))
                .height(Size::px(14.0)),
        )
        .child(
            label()
                .font_size(13.0)
                .color(parse_rgb(theme::TEXT_PRIMARY))
                .text(abbrev.to_string()),
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
