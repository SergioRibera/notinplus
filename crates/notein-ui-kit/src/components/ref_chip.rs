//! Inline reference chip (`notein://` links).

use std::borrow::Cow;

use freya::prelude::*;

use super::icon::Icon;
use crate::components::TapExt;
use crate::theme::palette;
use crate::tokens::{radius, text};

/// A reference to another document or a place inside one.
#[derive(Clone, PartialEq)]
pub struct RefChip {
    label: Cow<'static, str>,
    icon: Bytes,
    color: Color,
    broken: bool,
    on_paper: bool,
    on_press: Option<EventHandler<Event<PressEventData>>>,
}

impl std::fmt::Debug for RefChip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefChip")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl RefChip {
    /// `color` and `icon` are the target kind's.
    #[must_use]
    pub fn new(label: impl Into<Cow<'static, str>>, icon: Bytes, color: Color) -> Self {
        Self {
            label: label.into(),
            icon,
            color,
            broken: false,
            on_paper: false,
            on_press: None,
        }
    }

    /// The target no longer exists: dashed warning outline.
    #[must_use]
    pub const fn broken(mut self, broken: bool) -> Self {
        self.broken = broken;
        self
    }

    /// Drawn on paper (canvas text): dark text, stronger tint.
    #[must_use]
    pub const fn on_paper(mut self, on_paper: bool) -> Self {
        self.on_paper = on_paper;
        self
    }

    #[must_use]
    pub fn on_press(mut self, handler: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(handler.into());
        self
    }
}

impl Component for RefChip {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let (bg, fg, icon, icon_color) = if self.broken {
            (p.s3, p.t2, freya::icons::lucide::triangle_alert(), p.warn)
        } else if self.on_paper {
            (
                self.color.with_a(64),
                p.paper_ink,
                self.icon.clone(),
                self.color,
            )
        } else {
            (self.color.with_a(41), p.t1, self.icon.clone(), self.color)
        };
        let on_press = self.on_press.clone();
        rect()
            .height(Size::px(26.0))
            .padding((0.0, 8.0))
            .corner_radius(radius::SM)
            .background(bg)
            .maybe(self.broken, |r| {
                r.border(
                    Border::new()
                        .fill(p.warn)
                        .width(1.0)
                        .alignment(BorderAlignment::Inner),
                )
            })
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(6.0)
            .a11y_role(AccessibilityRole::Link)
            .a11y_alt(self.label.to_string())
            .map(on_press, |r, handler| r.on_tap(move |e| handler.call(e)))
            .child(Icon::new(icon).size(14.0).color(icon_color))
            .child(
                label()
                    .text(self.label.clone())
                    .font_size(text::META)
                    .font_weight(FontWeight::MEDIUM)
                    .color(fg)
                    .max_lines(1),
            )
    }
}
