//! A card the user picks among siblings, with a preview of what the
//! choice looks like.

use std::borrow::Cow;

use freya::prelude::*;

use crate::theme::palette;
use crate::tokens::{radius, text};

/// One option of a visual choice (a font, a writing format). Lay
/// siblings in a flex row; each card takes an equal share.
#[derive(Clone, PartialEq)]
pub struct ChoiceCard {
    preview: Element,
    title: Cow<'static, str>,
    detail: Option<Cow<'static, str>>,
    chosen: bool,
    on_pick: Option<EventHandler<()>>,
}

impl std::fmt::Debug for ChoiceCard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChoiceCard")
            .field("title", &self.title)
            .field("chosen", &self.chosen)
            .finish_non_exhaustive()
    }
}

impl ChoiceCard {
    #[must_use]
    pub fn new(preview: impl IntoElement, title: impl Into<Cow<'static, str>>) -> Self {
        Self {
            preview: preview.into_element(),
            title: title.into(),
            detail: None,
            chosen: false,
            on_pick: None,
        }
    }

    /// A line under the title.
    #[must_use]
    pub fn detail(mut self, detail: impl Into<Cow<'static, str>>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    #[must_use]
    pub const fn chosen(mut self, chosen: bool) -> Self {
        self.chosen = chosen;
        self
    }

    #[must_use]
    pub fn on_pick(mut self, handler: impl Into<EventHandler<()>>) -> Self {
        self.on_pick = Some(handler.into());
        self
    }
}

impl Component for ChoiceCard {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut hovering = use_state(|| false);
        let a11y_id = use_a11y();
        let focus = use_focus(a11y_id);
        let chosen = self.chosen;
        let on_pick = self.on_pick.clone();
        let border = if chosen {
            p.accent
        } else if hovering() {
            p.line_strong
        } else {
            p.line
        };
        rect()
            .a11y_id(a11y_id)
            .a11y_role(AccessibilityRole::RadioButton)
            .a11y_alt(self.title.to_string())
            .a11y_focusable(true)
            .width(Size::flex(1.0))
            .padding((12.0, 14.0))
            .spacing(8.0)
            .corner_radius(radius::LG)
            .background(if chosen { p.accent_soft } else { p.s2 })
            .border(
                Border::new()
                    .fill(if focus() == Focus::Keyboard { p.focus } else { border })
                    .width(if chosen { 2.0 } else { 1.0 })
                    .alignment(BorderAlignment::Inner),
            )
            .on_pointer_enter(move |_| hovering.set(true))
            .on_pointer_leave(move |_| hovering.set(false))
            .map(on_pick, |r, handler| r.on_press(move |_| handler.call(())))
            .child(self.preview.clone())
            .child(
                rect()
                    .width(Size::fill())
                    .spacing(2.0)
                    .child(
                        label()
                            .text(self.title.clone())
                            .width(Size::fill())
                            .font_size(text::BODY)
                            .font_weight(FontWeight::SEMI_BOLD)
                            .color(if chosen { p.t1 } else { p.t2 })
                            .max_lines(2)
                            .text_overflow(TextOverflow::Ellipsis),
                    )
                    .maybe_child(self.detail.clone().map(|detail| {
                        label()
                            .text(detail)
                            .width(Size::fill())
                            .font_size(text::LABEL)
                            .color(p.t3)
                            .max_lines(2)
                    })),
            )
    }
}
