//! Tinted SVG icon at one of the kit sizes.

use freya::prelude::*;

use crate::theme::palette;
use crate::tokens::size;

/// A Lucide (or any single-colour SVG) icon.
#[derive(Clone, PartialEq)]
pub struct Icon {
    svg: Bytes,
    size: f32,
    color: Option<Color>,
}

impl std::fmt::Debug for Icon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Icon")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl Icon {
    #[must_use]
    pub const fn new(svg: Bytes) -> Self {
        Self {
            svg,
            size: size::ICON,
            color: None,
        }
    }

    #[must_use]
    pub const fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Stroke colour. Defaults to the secondary text colour.
    #[must_use]
    pub const fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
}

impl Component for Icon {
    fn render(&self) -> impl IntoElement {
        let color = self.color.unwrap_or_else(|| palette().t2);
        SvgViewer::new(self.svg.clone())
            .color(color)
            .width(Size::px(self.size))
            .height(Size::px(self.size))
    }
}
