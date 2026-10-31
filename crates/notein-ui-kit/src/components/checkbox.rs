//! Square check mark.

use freya::prelude::*;

use super::icon::Icon;
use crate::components::TapExt;
use crate::theme::palette;
use crate::tokens::radius;

/// Checkbox. The parent owns the value.
#[derive(Clone, PartialEq)]
pub struct Checkbox {
    checked: bool,
    on_toggle: Option<EventHandler<bool>>,
}

impl std::fmt::Debug for Checkbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Checkbox")
            .field("checked", &self.checked)
            .finish_non_exhaustive()
    }
}

impl Checkbox {
    #[must_use]
    pub const fn new(checked: bool) -> Self {
        Self {
            checked,
            on_toggle: None,
        }
    }

    #[must_use]
    pub fn on_toggle(mut self, handler: impl Into<EventHandler<bool>>) -> Self {
        self.on_toggle = Some(handler.into());
        self
    }
}

impl Component for Checkbox {
    #[cfg_attr(feature = "sismo", sismo::function("kit::checkbox"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let on = self.checked;
        let handler = self.on_toggle.clone();
        rect()
            .a11y_role(AccessibilityRole::CheckBox)
            .width(Size::px(20.0))
            .height(Size::px(20.0))
            .corner_radius(radius::XS)
            .map(on.then_some(p.accent), Rect::background)
            .maybe(!on, |r| {
                r.border(
                    Border::new()
                        .fill(p.line_strong)
                        .width(1.5)
                        .alignment(BorderAlignment::Inner),
                )
            })
            .center()
            .map(handler, |r, handler| r.on_tap(move |_| handler.call(!on)))
            .maybe_child(on.then(|| {
                Icon::new(freya::icons::lucide::check())
                    .size(14.0)
                    .color(p.on_accent)
            }))
    }
}
