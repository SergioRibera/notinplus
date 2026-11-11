//! A turning ring for work in progress.

use freya::animation::{AnimNum, Function, OnCreation, OnFinish, use_animation};
use freya::prelude::*;

use super::icon::Icon;
use crate::theme::palette;

/// The loader icon, turning. With reduced motion it stays still: the
/// text beside it already says something is happening.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spinner {
    size: f32,
    color: Option<Color>,
}

impl Default for Spinner {
    fn default() -> Self {
        Self {
            size: 18.0,
            color: None,
        }
    }
}

impl Spinner {
    #[must_use]
    pub const fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Defaults to the accent.
    #[must_use]
    pub const fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
}

impl Component for Spinner {
    #[cfg_attr(feature = "sismo", sismo::function("kit::spinner"))]
    fn render(&self) -> impl IntoElement {
        let turn = use_animation(|conf| {
            conf.on_creation(OnCreation::Run);
            conf.on_finish(OnFinish::restart());
            AnimNum::new(0.0, 360.0)
                .time(900)
                .function(Function::Linear)
        });
        let angle = if crate::theme::reduce_motion() {
            0.0
        } else {
            turn.get().value()
        };
        rect()
            .width(Size::px(self.size))
            .height(Size::px(self.size))
            .rotate(angle)
            .a11y_role(AccessibilityRole::ProgressIndicator)
            .child(
                Icon::new(freya::icons::lucide::loader_circle())
                    .size(self.size)
                    .color(self.color.unwrap_or_else(|| palette().accent)),
            )
    }
}
