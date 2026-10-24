//! Tinted square with a document kind's icon.

use freya::prelude::*;

use super::icon::Icon;

/// Kind badge: the kind colour at 18 % behind its icon.
#[derive(Clone, PartialEq)]
pub struct KindBadge {
    icon: Bytes,
    color: Color,
    size: f32,
}

impl std::fmt::Debug for KindBadge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KindBadge")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl KindBadge {
    #[must_use]
    pub const fn new(icon: Bytes, color: Color) -> Self {
        Self {
            icon,
            color,
            size: 30.0,
        }
    }

    #[must_use]
    pub const fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
}

impl Component for KindBadge {
    fn render(&self) -> impl IntoElement {
        rect()
            .width(Size::px(self.size))
            .height(Size::px(self.size))
            .corner_radius(self.size * 0.27)
            .background(self.color.with_a(46))
            .center()
            .child(
                Icon::new(self.icon.clone())
                    .size((self.size * 0.53).round())
                    .color(self.color),
            )
    }
}
