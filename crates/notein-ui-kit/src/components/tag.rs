//! Filter chip / tag pill.

use std::borrow::Cow;

use freya::prelude::*;
use notein_i18n::tr;

use super::icon::Icon;
use super::scroll_content::TapExt;
use crate::theme::palette;
use crate::tokens::{radius, text};

/// Pill used for filters, tags and small status labels.
#[derive(Clone, PartialEq)]
pub struct Tag {
    label: Cow<'static, str>,
    active: bool,
    icon: Option<Bytes>,
    dot: Option<Color>,
    count: Option<u32>,
    closable: bool,
    on_press: Option<EventHandler<Event<PressEventData>>>,
    on_close: Option<EventHandler<()>>,
    key: DiffKey,
}

impl std::fmt::Debug for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tag")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl KeyExt for Tag {
    fn key_mut(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Tag {
    #[must_use]
    pub fn new(label: impl Into<Cow<'static, str>>) -> Self {
        Self {
            label: label.into(),
            active: false,
            icon: None,
            dot: None,
            count: None,
            closable: false,
            on_press: None,
            on_close: None,
            key: DiffKey::None,
        }
    }

    #[must_use]
    pub const fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    #[must_use]
    pub fn icon(mut self, icon: Bytes) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Colour dot before the label (tag colour, mesa colour).
    #[must_use]
    pub const fn dot(mut self, color: Color) -> Self {
        self.dot = Some(color);
        self
    }

    /// Trailing count.
    #[must_use]
    pub const fn count(mut self, count: u32) -> Self {
        self.count = Some(count);
        self
    }

    #[must_use]
    pub fn on_press(mut self, handler: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(handler.into());
        self
    }

    /// Show a trailing × that calls `handler`.
    #[must_use]
    pub fn on_close(mut self, handler: impl Into<EventHandler<()>>) -> Self {
        self.closable = true;
        self.on_close = Some(handler.into());
        self
    }
}

impl Component for Tag {
    #[cfg_attr(feature = "sismo", sismo::function("kit::tag"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let (bg, fg) = if self.active {
            (p.accent_soft, p.accent_text)
        } else {
            (p.s3, p.t2)
        };
        let on_press = self.on_press.clone();
        let on_close = self.on_close.clone();
        rect()
            .height(Size::px(28.0))
            .padding((0.0, 10.0))
            .corner_radius(radius::FULL)
            .background(bg)
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(5.0)
            .a11y_role(AccessibilityRole::Button)
            .a11y_alt(self.label.to_string())
            .map(on_press, |r, handler| r.on_tap(move |e| handler.call(e)))
            .maybe_child(
                self.icon
                    .clone()
                    .map(|icon| Icon::new(icon).size(13.0).color(fg)),
            )
            .maybe_child(self.dot.map(|dot| {
                rect()
                    .width(Size::px(8.0))
                    .height(Size::px(8.0))
                    .corner_radius(radius::FULL)
                    .background(dot)
            }))
            .child(
                label()
                    .text(self.label.clone())
                    .font_size(text::LABEL)
                    .font_weight(FontWeight::MEDIUM)
                    .color(fg)
                    .max_lines(1),
            )
            .maybe_child(self.count.map(|count| {
                label()
                    .text(count.to_string())
                    .font_size(text::CAPTION)
                    .color(p.t3)
            }))
            .maybe_child(self.closable.then(|| {
                rect()
                    .a11y_role(AccessibilityRole::Button)
                    .a11y_alt(tr!("kit-tag-remove"))
                    .map(on_close, |r, handler| r.on_tap(move |_| handler.call(())))
                    .child(Icon::new(freya::icons::lucide::x()).size(12.0).color(p.t3))
            }))
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}
