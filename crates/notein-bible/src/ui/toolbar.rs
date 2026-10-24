//! Top toolbar — search / more / translation chip.
//!
//! Mirrors the right-aligned stack seen in common Bible-reading apps:
//! two square icon buttons on the left and a pill-shaped translation
//! picker on the right. Callbacks are supplied by the host so the
//! actual behaviour (opening search sheet, popping the translation
//! menu, etc.) stays outside this crate.

use std::borrow::Cow;

use freya::prelude::*;

use notein_ui_kit::theme::palette;

const ICON_SIZE: f32 = 20.0;
const BUTTON_SIZE: f32 = 36.0;

/// Boxed back handler. Boxed (instead of a generic parameter) so
/// callers can leave it `None` without needing a turbofish on the
/// generic bounds.
pub type BackHandler = Box<dyn FnMut() + 'static>;

/// `(home, back, forward, split_h, split_v, close_split, search,
/// translation)` click handlers. Nav + split buttons are optional —
/// pass `None` to render them disabled (or absent for `close_split`).
#[allow(missing_debug_implementations)]
pub struct ToolbarCallbacks<FT>
where
    FT: FnMut() + 'static,
{
    pub on_home: Option<BackHandler>,
    /// Opens the sidebar (documents, contents, bookmarks).
    pub on_sidebar: Option<BackHandler>,
    pub on_back: Option<BackHandler>,
    pub on_forward: Option<BackHandler>,
    /// Opens (or creates) the notebook beside the reader.
    pub on_notebook: Option<BackHandler>,
    /// Opens the document actions (open beside, link with, links).
    pub on_links: Option<BackHandler>,
    /// Reactive buffer bound to the inline search input. The caller
    /// owns the signal + reacts to its value in a `use_side_effect`
    /// (so the toolbar doesn't need an explicit submit callback).
    pub search_query: Writable<String>,
    /// Fired when the user types OR picks a suggestion; the caller
    /// decides whether the string is a new filter query or a hit label
    /// to navigate to.
    pub on_search: EventHandler<String>,
    /// Current list of hit labels rendered as suggestions in the
    /// inline `FormInput` dropdown. The caller refreshes this each time
    /// `on_search` runs the FTS query.
    pub search_suggestions: Vec<Cow<'static, str>>,
    pub on_translation: FT,
}

pub fn top_toolbar<FT>(
    translation_abbrev: &str,
    callbacks: ToolbarCallbacks<FT>,
) -> impl IntoElement
where
    FT: FnMut() + 'static,
{
    let ToolbarCallbacks {
        on_home,
        on_sidebar,
        on_back,
        on_forward,
        on_notebook,
        on_links,
        search_query,
        on_search,
        search_suggestions,
        on_translation,
    } = callbacks;

    let row = rect()
        .width(Size::fill())
        .background(palette().bg)
        .padding(16.)
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .main_align(Alignment::SpaceBetween)
        .spacing(4.0);

    let mut left = rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .spacing(2.0);
    if let Some(cb) = on_home {
        left = left.child(icon_button(freya::icons::lucide::house(), cb));
    }
    if let Some(cb) = on_sidebar {
        left = left.child(icon_button(freya::icons::lucide::panel_left(), cb));
    }
    // Back / forward buttons always render — when the stack is empty we
    // keep the icon in the grid with a muted tint so the toolbar
    // doesn't reflow between chapters.
    left = left.child(nav_icon_button(freya::icons::lucide::arrow_left(), on_back));
    left = left.child(nav_icon_button(
        freya::icons::lucide::arrow_right(),
        on_forward,
    ));

    let mut right = rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .spacing(4.0)
        .child(
            notein_ui_kit::FormInput::new(Writable::<String>::from(search_query))
                .flat()
                .collapsible()
                .left_icon(freya::icons::lucide::search())
                .placeholder("Buscar en la Biblia…")
                .background(Color::TRANSPARENT)
                .width(Size::px(360.))
                .min_width(Size::px(40.))
                .suggestions(search_suggestions)
                .suggestions_unfiltered()
                .on_change(on_search),
        );
    if let Some(cb) = on_notebook {
        right = right.child(icon_button(
            freya::icons::lucide::notebook_text(),
            cb,
        ));
    }
    if let Some(cb) = on_links {
        right = right.child(icon_button(freya::icons::lucide::link(), cb));
    }
    right = right.child(translation_chip(translation_abbrev, on_translation));

    row.child(left).child(right)
}

/// Variant of [`top_toolbar`] rendered when no translations are
/// installed. Keeps the full chrome visible (so the user can still
/// navigate home) but every control except the home icon is muted +
/// inert — the search box, translation chip and nav icons all
/// render as decorations.
pub fn empty_toolbar(on_home: impl FnMut() + 'static) -> impl IntoElement {
    let on_home = on_home;
    rect()
        .width(Size::fill())
        .height(Size::px(56.0))
        .background(palette().bg)
        .padding((0., 16.))
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .main_align(Alignment::SpaceBetween)
        .spacing(4.0)
        .child(
            rect()
                .direction(Direction::Horizontal)
                .cross_align(Alignment::Center)
                .spacing(2.0)
                .child(icon_button(freya::icons::lucide::house(), on_home))
                .child(nav_icon_button(freya::icons::lucide::arrow_left(), None))
                .child(nav_icon_button(freya::icons::lucide::arrow_right(), None)),
        )
        .child(
            rect()
                .direction(Direction::Horizontal)
                .cross_align(Alignment::Center)
                .spacing(4.0)
                .child(muted_search_placeholder())
                .child(muted_translation_chip()),
        )
}

fn muted_search_placeholder() -> impl IntoElement {
    rect()
        .height(Size::px(34.0))
        .width(Size::px(40.0))
        .center()
        .with_corner_radius(17.0)
        .child(
            SvgViewer::new(freya::icons::lucide::search())
                .color(palette().t3)
                .width(Size::px(ICON_SIZE))
                .height(Size::px(ICON_SIZE)),
        )
}

fn muted_translation_chip() -> impl IntoElement {
    rect()
        .height(Size::px(34.0))
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .padding((0., 12.))
        .spacing(6.0)
        .margin((0., 0., 0., 6.))
        .with_corner_radius(17.0)
        .background(palette().s2)
        .child(
            SvgViewer::new(freya::icons::lucide::globe())
                .color(palette().t3)
                .width(Size::px(14.0))
                .height(Size::px(14.0)),
        )
        .child(
            label()
                .font_size(13.0)
                .color(palette().t3)
                .text("—"),
        )
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
                .color(palette().t1)
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
                    .color(palette().t1)
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
                    .color(palette().t3)
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
        .background(palette().s2)
        .on_press(move |_: Event<PressEventData>| on_press())
        .child(
            SvgViewer::new(freya::icons::lucide::globe())
                .color(palette().t1)
                .width(Size::px(14.0))
                .height(Size::px(14.0)),
        )
        .child(
            label()
                .font_size(13.0)
                .color(palette().t1)
                .text(abbrev.to_string()),
        )
}
