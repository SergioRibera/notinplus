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
    /// Label colour; the palette's primary text when unset.
    ink: Option<Color>,
    /// Factor applied to every length, so a zoomed host lays the chip out
    /// at its drawn size.
    zoom: f32,
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
            ink: None,
            zoom: 1.0,
            on_press: None,
        }
    }

    /// The target no longer exists: dashed warning outline.
    #[must_use]
    pub const fn broken(mut self, broken: bool) -> Self {
        self.broken = broken;
        self
    }

    /// Colour of the label, for chips inside text that is not the
    /// palette's (a canvas box with its own text colour).
    #[must_use]
    pub const fn ink(mut self, ink: Color) -> Self {
        self.ink = Some(ink);
        self
    }

    /// Lay the chip out at `zoom` times its base size.
    #[must_use]
    pub const fn zoom(mut self, zoom: f32) -> Self {
        self.zoom = zoom;
        self
    }

    #[must_use]
    pub fn on_press(mut self, handler: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(handler.into());
        self
    }
}

impl Component for RefChip {
    #[cfg_attr(feature = "sismo", sismo::function("kit::ref_chip"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let ink = self.ink.unwrap_or(p.t1);
        let (bg, fg, icon, icon_color) = if self.broken {
            (p.s3, p.t2, freya::icons::lucide::triangle_alert(), p.warn)
        } else {
            (self.color.with_a(41), ink, self.icon.clone(), self.color)
        };
        let px = |length: f32| length * self.zoom;
        let on_press = self.on_press.clone();
        rect()
            .height(Size::px(px(26.0)))
            .padding((0.0, px(8.0)))
            .corner_radius(px(radius::SM))
            .background(bg)
            .maybe(self.broken, |r| {
                r.border(
                    Border::new()
                        .fill(p.warn)
                        .width(px(1.0))
                        .alignment(BorderAlignment::Inner),
                )
            })
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(px(6.0))
            .a11y_role(AccessibilityRole::Link)
            .a11y_alt(self.label.to_string())
            .map(on_press, |r, handler| r.on_tap(move |e| handler.call(e)))
            .child(Icon::new(icon).size(px(14.0)).color(icon_color))
            .child(
                label()
                    .text(self.label.clone())
                    .font_size(px(text::META))
                    .font_weight(FontWeight::MEDIUM)
                    .color(fg)
                    .max_lines(1),
            )
    }
}
