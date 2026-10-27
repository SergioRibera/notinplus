//! Segmented control: every option visible, one selected.

use std::borrow::Cow;

use freya::prelude::*;

use super::icon::Icon;
use crate::components::TapExt;
use crate::theme::palette;
use crate::tokens::{radius, text};

/// One segment of a [`Segmented`].
#[derive(Clone, PartialEq, Eq)]
pub struct Segment {
    label: Option<Cow<'static, str>>,
    icon: Option<Bytes>,
}

impl std::fmt::Debug for Segment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Segment")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl Segment {
    #[must_use]
    pub fn label(label: impl Into<Cow<'static, str>>) -> Self {
        Self {
            label: Some(label.into()),
            icon: None,
        }
    }

    #[must_use]
    pub fn with_icon(mut self, icon: Bytes) -> Self {
        self.icon = Some(icon);
        self
    }
}

impl<T: Into<Cow<'static, str>>> From<T> for Segment {
    fn from(label: T) -> Self {
        Self::label(label)
    }
}

/// Exclusive choice between a few options (≤ 4 reads best).
#[derive(Clone, PartialEq)]
pub struct Segmented {
    segments: Vec<Segment>,
    selected: usize,
    full_width: bool,
    height: f32,
    on_select: Option<EventHandler<usize>>,
}

impl std::fmt::Debug for Segmented {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Segmented")
            .field("selected", &self.selected)
            .finish_non_exhaustive()
    }
}

impl Segmented {
    #[must_use]
    pub fn new(segments: impl IntoIterator<Item = impl Into<Segment>>, selected: usize) -> Self {
        Self {
            segments: segments.into_iter().map(Into::into).collect(),
            selected,
            full_width: false,
            height: 36.0,
            on_select: None,
        }
    }

    /// Stretch to the parent's width with equal segments.
    #[must_use]
    pub const fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    #[must_use]
    pub const fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    /// Receives the index of the tapped segment.
    #[must_use]
    pub fn on_select(mut self, handler: impl Into<EventHandler<usize>>) -> Self {
        self.on_select = Some(handler.into());
        self
    }
}

impl Component for Segmented {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let inner = self.height - 6.0;
        let full = self.full_width;
        rect()
            .height(Size::px(self.height))
            .maybe(full, |r| r.width(Size::fill()).content(Content::Flex))
            .corner_radius(radius::MD)
            .background(p.s1)
            .border(
                Border::new()
                    .fill(p.line)
                    .width(1.0)
                    .alignment(BorderAlignment::Inner),
            )
            .padding(3.0)
            .horizontal()
            .spacing(2.0)
            .a11y_role(AccessibilityRole::RadioGroup)
            .children(self.segments.iter().enumerate().map(|(index, segment)| {
                let active = index == self.selected;
                let handler = self.on_select.clone();
                let color = if active { p.t1 } else { p.t2 };
                rect()
                    .key(index)
                    .height(Size::px(inner))
                    .maybe(full, |r| r.width(Size::flex(1.0)))
                    .padding((0.0, 12.0))
                    .corner_radius(radius::SM)
                    .map(active.then_some(p.s4), Rect::background)
                    .horizontal()
                    .center()
                    .spacing(6.0)
                    .a11y_role(AccessibilityRole::RadioButton)
                    .map(handler, |r, handler| r.on_tap(move |_| handler.call(index)))
                    .maybe_child(
                        segment
                            .icon
                            .clone()
                            .map(|icon| Icon::new(icon).size(16.0).color(color)),
                    )
                    .maybe_child(segment.label.clone().map(|text_value| {
                        label()
                            .text(text_value)
                            .font_size(text::META)
                            .font_weight(if active {
                                FontWeight::SEMI_BOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .color(color)
                            .max_lines(1)
                    }))
                    .into_element()
            }))
    }
}
