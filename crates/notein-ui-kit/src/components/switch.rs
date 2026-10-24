//! On/off toggle.

use freya::prelude::*;

use crate::theme::palette;
use crate::tokens::radius;

/// A switch that applies instantly. The parent owns the value.
#[derive(Clone, PartialEq)]
pub struct Switch {
    on: bool,
    enabled: bool,
    on_toggle: Option<EventHandler<bool>>,
}

impl std::fmt::Debug for Switch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Switch")
            .field("on", &self.on)
            .finish_non_exhaustive()
    }
}

impl Switch {
    #[must_use]
    pub const fn new(on: bool) -> Self {
        Self {
            on,
            enabled: true,
            on_toggle: None,
        }
    }

    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Receives the new value.
    #[must_use]
    pub fn on_toggle(mut self, handler: impl Into<EventHandler<bool>>) -> Self {
        self.on_toggle = Some(handler.into());
        self
    }
}

impl Component for Switch {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let a11y_id = use_a11y();
        let focus = use_focus(a11y_id);
        let on = self.on;
        let handler = self.on_toggle.clone();
        rect()
            .a11y_id(a11y_id)
            .a11y_focusable(self.enabled)
            .a11y_role(AccessibilityRole::Switch)
            .width(Size::px(40.0))
            .height(Size::px(24.0))
            .corner_radius(radius::FULL)
            .background(if on { p.accent } else { p.s4 })
            .opacity(if self.enabled { 1.0 } else { 0.4 })
            .padding(3.0)
            .horizontal()
            .main_align(if on { Alignment::End } else { Alignment::Start })
            .maybe(focus() == Focus::Keyboard, |r| {
                r.border(
                    Border::new()
                        .fill(p.focus)
                        .width(p.focus_width)
                        .alignment(BorderAlignment::Outer),
                )
            })
            .maybe(self.enabled, |r| {
                r.map(handler, |r, handler| r.on_press(move |_| handler.call(!on)))
            })
            .child(
                rect()
                    .width(Size::px(18.0))
                    .height(Size::px(18.0))
                    .corner_radius(radius::FULL)
                    .background(if on { Color::WHITE } else { p.t2 }),
            )
    }
}
