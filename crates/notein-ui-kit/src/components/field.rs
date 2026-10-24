//! Labelled text field and the search field built on it.

use std::borrow::Cow;

use freya::prelude::*;

use super::icon::Icon;
use crate::theme::palette;
use crate::tokens::{radius, text};

/// A single-line text input with label, help text and error state.
#[derive(Clone, PartialEq)]
pub struct TextField {
    value: Writable<String>,
    label: Option<Cow<'static, str>>,
    placeholder: Option<Cow<'static, str>>,
    help: Option<Cow<'static, str>>,
    error: Option<Cow<'static, str>>,
    icon: Option<Bytes>,
    suffix: Option<Cow<'static, str>>,
    auto_focus: bool,
    enabled: bool,
    max_len: Option<usize>,
    a11y_id: Option<AccessibilityId>,
    on_change: Option<EventHandler<String>>,
    on_submit: Option<EventHandler<String>>,
    on_blur: Option<EventHandler<()>>,
}

impl std::fmt::Debug for TextField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextField")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl TextField {
    #[must_use]
    pub fn new(value: impl Into<Writable<String>>) -> Self {
        Self {
            value: value.into(),
            label: None,
            placeholder: None,
            help: None,
            error: None,
            icon: None,
            suffix: None,
            auto_focus: false,
            enabled: true,
            max_len: None,
            a11y_id: None,
            on_change: None,
            on_submit: None,
            on_blur: None,
        }
    }

    #[must_use]
    pub fn label(mut self, label: impl Into<Cow<'static, str>>) -> Self {
        self.label = Some(label.into());
        self
    }

    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<Cow<'static, str>>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Hint under the field.
    #[must_use]
    pub fn help(mut self, help: impl Into<Cow<'static, str>>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Error under the field; turns the border red.
    #[must_use]
    pub fn error(mut self, error: Option<impl Into<Cow<'static, str>>>) -> Self {
        self.error = error.map(Into::into);
        self
    }

    #[must_use]
    pub fn icon(mut self, icon: Bytes) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Unit or hint drawn at the right edge inside the field.
    #[must_use]
    pub fn suffix(mut self, suffix: impl Into<Cow<'static, str>>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    #[must_use]
    pub const fn auto_focus(mut self, auto_focus: bool) -> Self {
        self.auto_focus = auto_focus;
        self
    }

    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Show an error once the text exceeds `max_len` characters.
    #[must_use]
    pub const fn max_len(mut self, max_len: usize) -> Self {
        self.max_len = Some(max_len);
        self
    }

    #[must_use]
    pub const fn a11y_id(mut self, id: AccessibilityId) -> Self {
        self.a11y_id = Some(id);
        self
    }

    /// Fires on every keystroke with the new text.
    #[must_use]
    pub fn on_change(mut self, handler: impl Into<EventHandler<String>>) -> Self {
        self.on_change = Some(handler.into());
        self
    }

    /// Fires on Enter.
    #[must_use]
    pub fn on_submit(mut self, handler: impl Into<EventHandler<String>>) -> Self {
        self.on_submit = Some(handler.into());
        self
    }

    /// Fires when the field loses focus after having it.
    #[must_use]
    pub fn on_blur(mut self, handler: impl Into<EventHandler<()>>) -> Self {
        self.on_blur = Some(handler.into());
        self
    }
}

impl TextField {
    /// The caller's error, or the length limit once it is passed.
    fn shown_error(&self) -> Option<Cow<'static, str>> {
        self.error.clone().or_else(|| {
            self.max_len
                .filter(|max| self.value.read().chars().count() > *max)
                .map(|max| Cow::Owned(format!("Máximo {max} caracteres")))
        })
    }
}

/// Call `gained` when the field takes focus and `lost` when it lets it
/// go.
fn use_focus_edges(
    focus: Memo<Focus>,
    gained: Option<EventHandler<()>>,
    lost: Option<EventHandler<()>>,
) {
    let mut was_focused = use_state(|| false);
    use_side_effect(move || {
        let now = focus.read().is_focused();
        let before = *was_focused.peek();
        if now == before {
            return;
        }
        was_focused.set(now);
        let edge = if now { &gained } else { &lost };
        if let Some(handler) = edge {
            handler.call(());
        }
    });
}

impl Component for TextField {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let own_id = use_a11y();
        let a11y_id = self.a11y_id.unwrap_or(own_id);
        let focus = use_focus(a11y_id);
        let focused = focus().is_focused();
        use_focus_edges(focus, None, self.on_blur.clone());
        let error = self.shown_error();
        let border = if error.is_some() {
            Some((p.danger, 2.0))
        } else if focused {
            Some((p.accent, 2.0))
        } else {
            None
        };

        let mut input = Input::new(self.value.clone())
            .padding(Gaps::new_all(0.0))
            .auto_focus(self.auto_focus)
            .a11y_id(a11y_id)
            .width(Size::flex(1.0))
            .flat()
            .theme_colors(InputColorsThemePartial {
                border_fill: Some(Color::TRANSPARENT.into()),
                focus_border_fill: Some(Color::TRANSPARENT.into()),
                background: Some(Color::TRANSPARENT.into()),
                focus_background: Some(Color::TRANSPARENT.into()),
                color: Some(p.t1.into()),
                placeholder_color: Some(p.t3.into()),
            });
        if let Some(placeholder) = &self.placeholder {
            input = input.placeholder(placeholder.clone());
        }
        if let Some(on_submit) = self.on_submit.clone() {
            input = input.on_submit(on_submit);
        }
        if let Some(on_change) = self.on_change.clone() {
            input = input.on_validate(move |v: InputValidator| on_change.call(v.text().clone()));
        }

        rect()
            .width(Size::fill())
            .spacing(6.0)
            .opacity(if self.enabled { 1.0 } else { 0.45 })
            .maybe_child(self.label.clone().map(|l| {
                label()
                    .text(l)
                    .font_size(text::LABEL)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(p.t2)
            }))
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(40.0))
                    .corner_radius(radius::MD)
                    .background(p.s3)
                    .map(border, |r, (color, width)| {
                        r.border(
                            Border::new()
                                .fill(color)
                                .width(width)
                                .alignment(BorderAlignment::Inner),
                        )
                    })
                    .padding((0.0, 12.0))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .content(Content::Flex)
                    .spacing(8.0)
                    .maybe_child(
                        self.icon
                            .clone()
                            .map(|icon| Icon::new(icon).size(16.0).color(p.t3)),
                    )
                    .child(input)
                    .maybe_child(
                        self.suffix
                            .clone()
                            .map(|s| label().text(s).font_size(text::LABEL).color(p.t3)),
                    ),
            )
            .maybe_child(error.map_or_else(
                || {
                    self.help.clone().map(|h| {
                        label()
                            .text(h)
                            .font_size(text::LABEL)
                            .color(p.t3)
                            .line_height(text::LINE_BODY)
                    })
                },
                |e| Some(label().text(e).font_size(text::LABEL).color(p.danger)),
            ))
    }
}

/// The search field: magnifier, placeholder, clear button and an
/// optional keyboard hint such as `Ctrl K`.
#[derive(Clone, PartialEq)]
pub struct SearchField {
    value: Writable<String>,
    placeholder: Cow<'static, str>,
    shortcut: Option<Cow<'static, str>>,
    auto_focus: bool,
    height: f32,
    background: Option<Color>,
    a11y_id: Option<AccessibilityId>,
    on_change: Option<EventHandler<String>>,
    on_submit: Option<EventHandler<String>>,
    on_focus: Option<EventHandler<()>>,
}

impl std::fmt::Debug for SearchField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchField")
            .field("placeholder", &self.placeholder)
            .finish_non_exhaustive()
    }
}

impl SearchField {
    #[must_use]
    pub fn new(
        value: impl Into<Writable<String>>,
        placeholder: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            value: value.into(),
            placeholder: placeholder.into(),
            shortcut: None,
            auto_focus: false,
            height: 40.0,
            background: None,
            a11y_id: None,
            on_change: None,
            on_submit: None,
            on_focus: None,
        }
    }

    /// Keyboard hint drawn at the right edge.
    #[must_use]
    pub fn shortcut(mut self, shortcut: impl Into<Cow<'static, str>>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    #[must_use]
    pub const fn auto_focus(mut self, auto_focus: bool) -> Self {
        self.auto_focus = auto_focus;
        self
    }

    #[must_use]
    pub const fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    #[must_use]
    pub const fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    #[must_use]
    pub const fn a11y_id(mut self, id: AccessibilityId) -> Self {
        self.a11y_id = Some(id);
        self
    }

    #[must_use]
    pub fn on_change(mut self, handler: impl Into<EventHandler<String>>) -> Self {
        self.on_change = Some(handler.into());
        self
    }

    #[must_use]
    pub fn on_submit(mut self, handler: impl Into<EventHandler<String>>) -> Self {
        self.on_submit = Some(handler.into());
        self
    }

    /// Fires when the field gains focus (Home opens the search
    /// overlay from here).
    #[must_use]
    pub fn on_focus(mut self, handler: impl Into<EventHandler<()>>) -> Self {
        self.on_focus = Some(handler.into());
        self
    }
}

impl Component for SearchField {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let own_id = use_a11y();
        let a11y_id = self.a11y_id.unwrap_or(own_id);
        let focus = use_focus(a11y_id);
        let focused = focus().is_focused();
        use_focus_edges(focus, self.on_focus.clone(), None);
        let has_text = !self.value.read().is_empty();
        let mut value = self.value.clone();
        let clear_change = self.on_change.clone();

        let mut input = Input::new(self.value.clone())
            .padding(Gaps::new_all(0.0))
            .auto_focus(self.auto_focus)
            .a11y_id(a11y_id)
            .width(Size::flex(1.0))
            .flat()
            .placeholder(self.placeholder.clone())
            .theme_colors(InputColorsThemePartial {
                border_fill: Some(Color::TRANSPARENT.into()),
                focus_border_fill: Some(Color::TRANSPARENT.into()),
                background: Some(Color::TRANSPARENT.into()),
                focus_background: Some(Color::TRANSPARENT.into()),
                color: Some(p.t1.into()),
                placeholder_color: Some(p.t3.into()),
            });
        if let Some(on_submit) = self.on_submit.clone() {
            input = input.on_submit(on_submit);
        }
        if let Some(on_change) = self.on_change.clone() {
            input = input.on_validate(move |v: InputValidator| on_change.call(v.text().clone()));
        }

        rect()
            .width(Size::fill())
            .height(Size::px(self.height))
            .corner_radius(radius::MD)
            .background(self.background.unwrap_or(p.s2))
            .maybe(focused, |r| {
                r.border(
                    Border::new()
                        .fill(p.accent)
                        .width(2.0)
                        .alignment(BorderAlignment::Inner),
                )
            })
            .padding((0.0, 12.0))
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(8.0)
            .child(
                Icon::new(freya::icons::lucide::search())
                    .size(18.0)
                    .color(p.t3),
            )
            .child(input)
            .maybe_child(has_text.then(|| {
                rect()
                    .a11y_role(AccessibilityRole::Button)
                    .a11y_alt("Borrar búsqueda")
                    .padding(4.0)
                    .on_press(move |_| {
                        value.set(String::new());
                        if let Some(handler) = &clear_change {
                            handler.call(String::new());
                        }
                    })
                    .child(Icon::new(freya::icons::lucide::x()).size(16.0).color(p.t3))
            }))
            .maybe_child(
                (!has_text)
                    .then(|| self.shortcut.clone())
                    .flatten()
                    .map(|s| {
                        rect()
                            .height(Size::px(22.0))
                            .padding((0.0, 6.0))
                            .corner_radius(radius::XS)
                            .background(p.s3)
                            .center()
                            .child(
                                label()
                                    .text(s)
                                    .font_size(text::CAPTION)
                                    .font_weight(FontWeight::SEMI_BOLD)
                                    .color(p.t2),
                            )
                    }),
            )
    }
}
