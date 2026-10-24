//! The row every list is built from.

use std::borrow::Cow;

use freya::prelude::*;

use super::checkbox::Checkbox;
use super::icon::Icon;
use super::kind_badge::KindBadge;
use super::switch::Switch;
use crate::theme::palette;
use crate::tokens::{radius, size, text};

/// What sits at the start of a [`ListRow`].
#[derive(Clone, PartialEq)]
pub enum Leading {
    Icon(Bytes),
    Kind { icon: Bytes, color: Color },
    Check(bool),
    Element(Element),
}

impl std::fmt::Debug for Leading {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Icon(_) => f.write_str("Leading::Icon"),
            Self::Kind { .. } => f.write_str("Leading::Kind"),
            Self::Check(on) => write!(f, "Leading::Check({on})"),
            Self::Element(_) => f.write_str("Leading::Element"),
        }
    }
}

/// What sits at the end of a [`ListRow`].
#[derive(Clone, PartialEq)]
pub enum Trailing {
    Text(Cow<'static, str>),
    Chevron,
    Switch {
        on: bool,
        on_toggle: EventHandler<bool>,
    },
    Element(Element),
}

impl std::fmt::Debug for Trailing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text(t) => write!(f, "Trailing::Text({t})"),
            Self::Chevron => f.write_str("Trailing::Chevron"),
            Self::Switch { on, .. } => write!(f, "Trailing::Switch({on})"),
            Self::Element(_) => f.write_str("Trailing::Element"),
        }
    }
}

/// A list row: leading visual, title, optional subtitle, trailing
/// control.
#[derive(Clone, PartialEq, Debug)]
#[allow(clippy::struct_excessive_bools)] // independent display flags of one row
pub struct ListRow {
    title: Cow<'static, str>,
    subtitle: Option<Cow<'static, str>>,
    leading: Option<Leading>,
    trailing: Option<Trailing>,
    selected: bool,
    enabled: bool,
    danger: bool,
    dense: bool,
    indent: f32,
    on_press: Option<EventHandler<Event<PressEventData>>>,
    on_secondary: Option<EventHandler<Event<PressEventData>>>,
    key: DiffKey,
}

impl KeyExt for ListRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl ListRow {
    #[must_use]
    pub fn new(title: impl Into<Cow<'static, str>>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            leading: None,
            trailing: None,
            selected: false,
            enabled: true,
            danger: false,
            dense: false,
            indent: 0.0,
            on_press: None,
            on_secondary: None,
            key: DiffKey::None,
        }
    }

    #[must_use]
    pub fn subtitle(mut self, subtitle: impl Into<Cow<'static, str>>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    #[must_use]
    pub fn leading(mut self, leading: Leading) -> Self {
        self.leading = Some(leading);
        self
    }

    #[must_use]
    pub fn icon(self, icon: Bytes) -> Self {
        self.leading(Leading::Icon(icon))
    }

    #[must_use]
    pub fn kind(self, icon: Bytes, color: Color) -> Self {
        self.leading(Leading::Kind { icon, color })
    }

    #[must_use]
    pub fn trailing(mut self, trailing: Trailing) -> Self {
        self.trailing = Some(trailing);
        self
    }

    #[must_use]
    pub fn meta(self, meta: impl Into<Cow<'static, str>>) -> Self {
        self.trailing(Trailing::Text(meta.into()))
    }

    #[must_use]
    pub fn chevron(self) -> Self {
        self.trailing(Trailing::Chevron)
    }

    #[must_use]
    pub const fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Red title and icon (destructive menu entries).
    #[must_use]
    pub const fn danger(mut self, danger: bool) -> Self {
        self.danger = danger;
        self
    }

    /// 40 px instead of 48 px.
    #[must_use]
    pub const fn dense(mut self, dense: bool) -> Self {
        self.dense = dense;
        self
    }

    /// Extra left padding (outline trees).
    #[must_use]
    pub const fn indent(mut self, indent: f32) -> Self {
        self.indent = indent;
        self
    }

    #[must_use]
    pub fn on_press(mut self, handler: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(handler.into());
        self
    }

    /// Right click / long press.
    #[must_use]
    pub fn on_secondary(mut self, handler: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_secondary = Some(handler.into());
        self
    }
}

impl Leading {
    fn render(self, danger: bool) -> Element {
        let p = palette();
        match self {
            Self::Icon(icon) => Icon::new(icon)
                .size(18.0)
                .color(if danger { p.danger } else { p.t2 })
                .into_element(),
            Self::Kind { icon, color } => KindBadge::new(icon, color).into_element(),
            Self::Check(on) => Checkbox::new(on).into_element(),
            Self::Element(element) => element,
        }
    }
}

impl Trailing {
    fn render(self) -> Element {
        let p = palette();
        match self {
            Self::Text(t) => label()
                .text(t)
                .font_size(text::LABEL)
                .color(p.t3)
                .max_lines(1)
                .into_element(),
            Self::Chevron => Icon::new(freya::icons::lucide::chevron_right())
                .size(16.0)
                .color(p.t3)
                .into_element(),
            Self::Switch { on, on_toggle } => Switch::new(on).on_toggle(on_toggle).into_element(),
            Self::Element(element) => element,
        }
    }
}

impl ListRow {
    /// Title and subtitle.
    fn text(&self, title_color: Color) -> Rect {
        rect()
            .width(Size::flex(1.0))
            .spacing(2.0)
            .child(
                label()
                    .text(self.title.clone())
                    .font_size(text::BODY)
                    .font_weight(FontWeight::MEDIUM)
                    .color(title_color)
                    .max_lines(1)
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .maybe_child(self.subtitle.clone().map(|sub| {
                label()
                    .text(sub)
                    .font_size(text::LABEL)
                    .color(palette().t3)
                    .max_lines(2)
                    .text_overflow(TextOverflow::Ellipsis)
            }))
    }
}

impl Component for ListRow {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut hovering = use_state(|| false);
        let a11y_id = use_a11y();
        let focus = use_focus(a11y_id);
        let clickable = self.on_press.is_some() && self.enabled;
        let background = if self.selected {
            Some(p.s3)
        } else if hovering() && clickable {
            Some(p.s3.with_a(150))
        } else {
            None
        };
        let title_color = if self.danger { p.danger } else { p.t1 };
        let on_press = self.on_press.clone();
        let on_secondary = self.on_secondary.clone();
        let height = if self.subtitle.is_some() {
            if self.dense { 48.0 } else { 56.0 }
        } else if self.dense {
            size::ROW_DENSE
        } else {
            size::ROW
        };

        rect()
            .a11y_id(a11y_id)
            .a11y_focusable(clickable)
            .a11y_role(if clickable {
                AccessibilityRole::Button
            } else {
                AccessibilityRole::ListItem
            })
            .a11y_alt(self.title.to_string())
            .width(Size::fill())
            .min_height(Size::px(height))
            .padding((6.0, 12.0, 6.0, 12.0 + self.indent))
            .corner_radius(radius::MD)
            .opacity(if self.enabled { 1.0 } else { 0.45 })
            .map(background, Rect::background)
            .maybe(focus() == Focus::Keyboard, |r| {
                r.border(
                    Border::new()
                        .fill(p.focus)
                        .width(p.focus_width)
                        .alignment(BorderAlignment::Inner),
                )
            })
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(12.0)
            .maybe(clickable, |r| {
                r.on_pointer_enter(move |_| hovering.set(true))
                    .on_pointer_leave(move |_| hovering.set(false))
                    .map(on_press, |r, handler| r.on_press(move |e| handler.call(e)))
            })
            .map(on_secondary, |r, handler| {
                r.on_secondary_down(move |e| handler.call(e))
            })
            .maybe_child(
                self.leading
                    .clone()
                    .map(|leading| leading.render(self.danger)),
            )
            .child(self.text(title_color))
            .maybe_child(self.trailing.clone().map(Trailing::render))
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}
