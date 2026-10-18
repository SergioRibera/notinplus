//! Page chrome, styled like the Bible reader's top bar.

use freya::icons::lucide;
use freya::prelude::*;

pub const BACKGROUND: Color = Color::from_rgb(10, 10, 10);
pub const TEXT: Color = Color::from_rgb(235, 235, 235);
pub const MUTED: Color = Color::from_rgb(150, 150, 160);
pub const CHIP: Color = Color::from_rgb(32, 32, 32);

const ICON_SIZE: f32 = 20.0;
const BUTTON_SIZE: f32 = 36.0;

/// Boxed press handler.
pub type Action = Box<dyn FnMut() + 'static>;

/// Top bar handlers. Missing ones hide their button.
#[allow(missing_debug_implementations)]
#[derive(Default)]
pub struct TopbarActions {
    pub on_home: Option<Action>,
    pub on_sidebar: Option<Action>,
    pub on_toggle_mode: Option<Action>,
    pub on_split_horizontal: Option<Action>,
    pub on_split_vertical: Option<Action>,
    pub on_close_split: Option<Action>,
}

/// `[home][sidebar] title ............ [split][mode]`
#[must_use]
pub fn topbar(title: &str, read_only: bool, actions: TopbarActions) -> impl IntoElement {
    let TopbarActions {
        on_home,
        on_sidebar,
        on_toggle_mode,
        on_split_horizontal,
        on_split_vertical,
        on_close_split,
    } = actions;

    let title = if title.trim().is_empty() {
        "(sin título)".to_owned()
    } else {
        title.to_owned()
    };

    rect()
        .width(Size::fill())
        .background(BACKGROUND)
        .padding(16.)
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::Flex)
        .spacing(4.0)
        .maybe_child(on_home.map(|on_press| icon_button(lucide::house(), on_press)))
        .maybe_child(on_sidebar.map(|on_press| icon_button(lucide::panel_left(), on_press)))
        .child(
            label()
                .width(Size::flex(1.))
                .margin((0., 8.))
                .font_size(16.)
                .font_weight(FontWeight::SEMI_BOLD)
                .color(TEXT)
                .max_lines(1)
                .text_overflow(TextOverflow::Ellipsis)
                .text(title),
        )
        .maybe_child(
            on_split_horizontal
                .map(|on_press| icon_button(lucide::square_split_horizontal(), on_press)),
        )
        .maybe_child(
            on_split_vertical
                .map(|on_press| icon_button(lucide::square_split_vertical(), on_press)),
        )
        .maybe_child(on_close_split.map(|on_press| icon_button(lucide::x(), on_press)))
        .maybe_child(on_toggle_mode.map(|on_press| mode_chip(read_only, on_press)))
}

fn icon_button(icon: Bytes, mut on_press: Action) -> Element {
    rect()
        .width(Size::px(BUTTON_SIZE))
        .height(Size::px(BUTTON_SIZE))
        .center()
        .corner_radius(BUTTON_SIZE / 2.0)
        .on_press(move |_| on_press())
        .child(
            SvgViewer::new(icon)
                .color(TEXT)
                .width(Size::px(ICON_SIZE))
                .height(Size::px(ICON_SIZE)),
        )
        .into_element()
}

/// Pill showing the current mode; pressing switches it.
fn mode_chip(read_only: bool, mut on_press: Action) -> Element {
    let (icon, text) = if read_only {
        (lucide::book_open(), "Lectura")
    } else {
        (lucide::pencil(), "Edición")
    };
    rect()
        .height(Size::px(34.0))
        .horizontal()
        .cross_align(Alignment::Center)
        .padding((0., 12.))
        .spacing(6.0)
        .margin((0., 0., 0., 6.))
        .corner_radius(17.0)
        .background(CHIP)
        .on_press(move |_| on_press())
        .child(
            SvgViewer::new(icon)
                .color(TEXT)
                .width(Size::px(14.0))
                .height(Size::px(14.0)),
        )
        .child(label().font_size(13.0).color(TEXT).text(text))
        .into_element()
}
