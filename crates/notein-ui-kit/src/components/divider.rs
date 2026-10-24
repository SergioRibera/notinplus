//! Hairline separators.

use freya::prelude::*;

use crate::theme::palette;

/// Orientation of a [`Divider`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Axis {
    #[default]
    Horizontal,
    Vertical,
}

/// One-pixel line in the theme's line colour.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Divider {
    axis: Axis,
    length: Option<f32>,
    strong: bool,
}

impl Divider {
    /// Full-width horizontal line.
    #[must_use]
    pub const fn horizontal() -> Self {
        Self {
            axis: Axis::Horizontal,
            length: None,
            strong: false,
        }
    }

    /// Vertical line `length` pixels tall, for toolbars.
    #[must_use]
    pub const fn vertical(length: f32) -> Self {
        Self {
            axis: Axis::Vertical,
            length: Some(length),
            strong: false,
        }
    }

    /// Use the strong line colour.
    #[must_use]
    pub const fn strong(mut self) -> Self {
        self.strong = true;
        self
    }
}

impl Component for Divider {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let color = if self.strong { p.line_strong } else { p.line };
        let length = self.length.map_or_else(Size::fill, Size::px);
        match self.axis {
            Axis::Horizontal => rect().width(length).height(Size::px(1.0)).background(color),
            Axis::Vertical => rect().width(Size::px(1.0)).height(length).background(color),
        }
    }
}

/// Flexible empty space inside a `Content::Flex` row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Spacer;

impl Component for Spacer {
    fn render(&self) -> impl IntoElement {
        rect().width(Size::flex(1.0)).height(Size::px(1.0))
    }
}
