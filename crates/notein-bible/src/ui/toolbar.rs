//! Top toolbar — search / more / translation chip.
//!
//! Mirrors the right-aligned stack seen in common Bible-reading apps:
//! two square icon buttons on the left and a pill-shaped translation
//! picker on the right. Callbacks are supplied by the host so the
//! actual behaviour (opening search sheet, popping the translation
//! menu, etc.) stays outside this crate.

use freya::prelude::*;

use super::theme;

const ICON_SIZE: f32 = 20.0;
const BUTTON_SIZE: f32 = 36.0;

/// Boxed back handler. Boxed (instead of a generic parameter) so
/// callers can leave it `None` without needing a turbofish on the
/// generic bounds.
pub type BackHandler = Box<dyn FnMut() + 'static>;

/// `(home, back, forward, search, more, translation)` click handlers.
/// All three nav buttons are optional — pass `None` when the stack is
/// empty so the toolbar collapses them.
#[allow(missing_debug_implementations)]
pub struct ToolbarCallbacks<FS, FM, FT>
where
    FS: FnMut() + 'static,
    FM: FnMut() + 'static,
    FT: FnMut() + 'static,
{
    pub on_home: Option<BackHandler>,
    pub on_back: Option<BackHandler>,
    pub on_forward: Option<BackHandler>,
    pub on_search: FS,
    pub on_more: FM,
    pub on_translation: FT,
}

pub fn top_toolbar<FS, FM, FT>(
    translation_abbrev: &str,
    callbacks: ToolbarCallbacks<FS, FM, FT>,
) -> impl IntoElement
where
    FS: FnMut() + 'static,
    FM: FnMut() + 'static,
    FT: FnMut() + 'static,
{
    let ToolbarCallbacks {
        on_home,
        on_back,
        on_forward,
        on_search,
        on_more: _on_more,
        on_translation,
    } = callbacks;

    let row = rect()
        .width(Size::fill())
        .height(Size::px(56.0))
        .background(parse_rgb(theme::BG))
        .padding((0., 16.))
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .main_align(Alignment::SpaceBetween)
        .spacing(4.0);

    let mut left = rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .spacing(2.0);
    if let Some(mut cb) = on_home {
        left = left.child(icon_button(freya::icons::lucide::house(), move || cb()));
    }
    // Back / forward buttons always render — when the stack is empty we
    // keep the icon in the grid with a muted tint so the toolbar
    // doesn't reflow between chapters.
    left = left.child(nav_icon_button(freya::icons::lucide::arrow_left(), on_back));
    left = left.child(nav_icon_button(freya::icons::lucide::arrow_right(), on_forward));

    let right = rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .spacing(4.0)
        .child(icon_button(freya::icons::lucide::search(), on_search))
        .child(translation_chip(translation_abbrev, on_translation));

    row.child(left).child(right)
}

fn icon_button<F>(icon: Bytes, mut on_press: F) -> impl IntoElement
where
    F: FnMut() + 'static,
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

/// Icon that renders enabled when a handler is supplied, muted
/// otherwise. Preserves slot width so the toolbar doesn't jitter
/// as the nav stacks fill / drain.
fn nav_icon_button(icon: Bytes, handler: Option<BackHandler>) -> impl IntoElement {
    match handler {
        Some(mut cb) => rect()
            .width(Size::px(BUTTON_SIZE))
            .height(Size::px(BUTTON_SIZE))
            .center()
            .with_corner_radius(BUTTON_SIZE / 2.0)
            .on_press(move |_: Event<PressEventData>| cb())
            .child(
                SvgViewer::new(icon)
                    .color(parse_rgb(theme::TEXT_PRIMARY))
                    .width(Size::px(ICON_SIZE))
                    .height(Size::px(ICON_SIZE)),
            ),
        None => rect()
            .width(Size::px(BUTTON_SIZE))
            .height(Size::px(BUTTON_SIZE))
            .center()
            .with_corner_radius(BUTTON_SIZE / 2.0)
            .child(
                SvgViewer::new(icon)
                    .color(Color::from_rgb(80, 80, 90))
                    .width(Size::px(ICON_SIZE))
                    .height(Size::px(ICON_SIZE)),
            ),
    }
}

fn translation_chip<F>(abbrev: &str, mut on_press: F) -> impl IntoElement
where
    F: FnMut() + 'static,
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
