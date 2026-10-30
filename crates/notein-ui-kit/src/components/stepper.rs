//! Integer stepper for small ranges.

use freya::prelude::*;
use notein_i18n::tr;

use super::icon_button::IconButton;
use crate::theme::palette;
use crate::tokens::{radius, text};

/// `−  19  +` control. The parent owns the value.
#[derive(Clone, PartialEq)]
pub struct Stepper {
    value: i64,
    min: i64,
    max: i64,
    on_change: Option<EventHandler<i64>>,
}

impl std::fmt::Debug for Stepper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stepper")
            .field("value", &self.value)
            .finish_non_exhaustive()
    }
}

impl Stepper {
    #[must_use]
    pub const fn new(value: i64, min: i64, max: i64) -> Self {
        Self {
            value,
            min,
            max,
            on_change: None,
        }
    }

    #[must_use]
    pub fn on_change(mut self, handler: impl Into<EventHandler<i64>>) -> Self {
        self.on_change = Some(handler.into());
        self
    }
}

impl Component for Stepper {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let (value, min, max) = (self.value, self.min, self.max);
        let dec = self.on_change.clone();
        let inc = self.on_change.clone();
        rect()
            .height(Size::px(36.0))
            .corner_radius(radius::MD)
            .background(p.s3)
            .horizontal()
            .cross_align(Alignment::Center)
            .child(
                IconButton::new(freya::icons::lucide::minus(), tr!("kit-stepper-decrease"))
                    .size(36.0)
                    .enabled(value > min)
                    .on_press(move |_| {
                        if let Some(h) = &dec {
                            h.call((value - 1).max(min));
                        }
                    }),
            )
            .child(
                label()
                    .text(value.to_string())
                    .font_size(text::BODY)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(p.t1)
                    .min_width(Size::px(32.0))
                    .text_align(TextAlign::Center),
            )
            .child(
                IconButton::new(freya::icons::lucide::plus(), tr!("kit-stepper-increase"))
                    .size(36.0)
                    .enabled(value < max)
                    .on_press(move |_| {
                        if let Some(h) = &inc {
                            h.call((value + 1).min(max));
                        }
                    }),
            )
    }
}
