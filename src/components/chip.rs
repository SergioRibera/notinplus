//! Pill-shaped label used for tags, filters, and status markers.
//!
//! Two variants:
//!   * `Chip::new("Foo")` — auto-colored from a hash of the label,
//!     matching the "Tags" look (soft bg + medium border + accent text).
//!   * `Chip::new("Foo").outline()` — neutral border, no fill.
//!
//! Override the accent with `.color(some_color)`. Attach a press
//! handler with `.on_press(...)`. The chip stays a plain `rect()` —
//! no `freya_components::Chip` theming shell needed.

use freya::prelude::*;

use crate::components::theme::{BORDER, TEXT_PRIMARY};

#[derive(Clone, PartialEq)]
enum ChipVariant {
    Accent(Color),
    Outline,
}

#[derive(Clone, PartialEq)]
pub struct Chip {
    label: String,
    variant: ChipVariant,
    key: DiffKey,
    on_press: Option<EventHandler<Event<PressEventData>>>,
}

impl KeyExt for Chip {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Chip {
    #[must_use]
    pub fn new(label: impl Into<String>) -> Self {
        let label = label.into();
        let accent = auto_accent(&label);
        Self {
            label,
            variant: ChipVariant::Accent(accent),
            on_press: None,
            key: DiffKey::None,
        }
    }

    #[must_use]
    pub fn outline(mut self) -> Self {
        self.variant = ChipVariant::Outline;
        self
    }

    /// Force an explicit accent. Background = accent @ ~15% alpha,
    /// border = accent @ ~60% alpha, text = accent.
    #[must_use]
    pub fn color(mut self, color: Color) -> Self {
        self.variant = ChipVariant::Accent(color);
        self
    }

    #[must_use]
    pub fn on_press(mut self, f: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(f.into());
        self
    }
}

impl Component for Chip {
    fn render(&self) -> impl IntoElement {
        let content = self.label.clone();
        let on_press = self.on_press.clone();

        let (bg, border_color, text_color) = match self.variant {
            ChipVariant::Accent(accent) => (accent.with_a(38), accent.with_a(153), accent),
            ChipVariant::Outline => (Color::TRANSPARENT, BORDER, TEXT_PRIMARY),
        };

        rect()
            .rounded_full()
            .border(Border::new().width(1.).fill(border_color))
            .background(bg)
            .padding((3., 10., 3., 10.))
            .maybe(on_press.is_some(), move |r| {
                r.on_press(move |e| {
                    if let Some(h) = &on_press {
                        h.call(e);
                    }
                })
            })
            .child(
                label()
                    .font_size(12.)
                    .font_weight(FontWeight::MEDIUM)
                    .color(text_color)
                    .text(content),
            )
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

fn auto_accent(label: &str) -> Color {
    let hash = label.bytes().fold(0u32, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(u32::from(b))
    });
    let hue = (hash % 360) as f32;
    Color::from_hsv(hue, 0.65, 0.85)
}
