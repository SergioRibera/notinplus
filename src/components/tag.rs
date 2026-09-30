//! Interactive tag pill. Supports three orthogonal modes:
//!
//!   * `.selectable(bool)` — renders a checkbox on the left; the bool
//!     is the current selected state (parent drives, tag is dumb).
//!   * `.closable()` — appends an `x` in a soft circle on the right,
//!     for "chip removed from a list" flows.
//!   * `.on_press(...)` — fires on tap; opts the whole pill into
//!     hover/pointer feedback.
//!
//! Purely dark-palette-driven; no theming shell, plain `rect()`.

use freya::{icons::lucide::x, prelude::*};

use crate::components::theme::{
    BORDER, PRIMARY, SECONDARY, SURFACE_SECONDARY, SURFACE_TERTIARY, TEXT_INVERSE, TEXT_PRIMARY,
    TEXT_SECONDARY,
};

#[derive(Clone, PartialEq)]
pub struct Tag {
    label: String,
    closable: bool,
    selectable: Option<bool>,
    key: DiffKey,
    on_press: Option<EventHandler<Event<PressEventData>>>,
}

impl KeyExt for Tag {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Tag {
    #[must_use]
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            on_press: None,
            closable: false,
            selectable: None,
            key: DiffKey::None,
        }
    }

    #[must_use]
    pub const fn selectable(mut self, selected: bool) -> Self {
        self.selectable = Some(selected);
        self
    }

    #[must_use]
    pub const fn closable(mut self) -> Self {
        self.closable = true;
        self
    }

    #[must_use]
    pub fn on_press(mut self, f: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(f.into());
        self
    }
}

impl Component for Tag {
    fn render(&self) -> impl IntoElement {
        let is_selectable = self.selectable.is_some();
        let selected = self.selectable.unwrap_or(false);
        let closable = self.closable;
        let label_text = self.label.clone();
        let on_press = self.on_press.clone();

        let mut is_hover = use_state(|| false);

        let (bg, border_color, text_color) = if selected {
            (PRIMARY.with_a(40), SECONDARY, TEXT_PRIMARY)
        } else {
            (SURFACE_SECONDARY, BORDER, TEXT_SECONDARY)
        };

        rect()
            .rounded_full()
            .border(
                Border::new()
                    .width(if selected { 1.5 } else { 1. })
                    .fill(border_color),
            )
            .background(bg)
            .padding((6., 14., 6., if is_selectable { 8. } else { 14. }))
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(6.)
            .maybe(is_selectable || closable || on_press.is_some(), |r| {
                r.on_pointer_enter(move |_| is_hover.set(true))
                    .on_pointer_leave(move |_| is_hover.set(false))
                    .maybe(on_press.is_some(), move |r| {
                        r.on_press(move |e| {
                            if let Some(handler) = &on_press {
                                handler.call(e);
                            }
                        })
                    })
            })
            .maybe(is_selectable, |r| {
                r.child(
                    rect()
                        .width(Size::px(18.))
                        .height(Size::px(18.))
                        .rounded_full()
                        .center()
                        .border(Border::new().width(1.5).fill(if selected {
                            SECONDARY
                        } else {
                            BORDER
                        }))
                        .background(if selected {
                            SECONDARY
                        } else {
                            Color::TRANSPARENT
                        })
                        .maybe_child(selected.then(|| {
                            SvgViewer::new(freya::icons::lucide::check())
                                .color(TEXT_INVERSE)
                                .width(Size::px(11.))
                                .height(Size::px(11.))
                        })),
                )
            })
            .child(
                label()
                    .text(label_text)
                    .font_size(14.)
                    .font_weight(if selected {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .color(text_color),
            )
            .maybe(closable, |r| {
                r.padding((6., 8., 6., 14.)).child(
                    rect()
                        .center()
                        .width(Size::px(16.))
                        .height(Size::px(16.))
                        .rounded_full()
                        .background(SURFACE_TERTIARY)
                        .child(
                            SvgViewer::new(x())
                                .color(TEXT_SECONDARY)
                                .width(Size::px(10.))
                                .height(Size::px(10.)),
                        ),
                )
            })
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}
