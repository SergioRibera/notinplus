//! A panel along one edge of a screen that slides in, pushing what sits
//! beside it aside, and slides back out.

use freya::animation::{
    AnimNum, Ease, Function, OnChange, OnCreation, use_animation_with_dependencies,
};
use freya::prelude::*;

use crate::{reduce_motion, tokens};

/// The edge a [`SidePanel`] comes in from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PanelEdge {
    Left,
    #[default]
    Right,
}

/// A panel of a fixed width that grows from nothing while it slides in
/// from its edge. Place it beside a flexible sibling in a horizontal row:
/// the sibling narrows as the panel opens.
#[derive(Clone, PartialEq)]
pub struct SidePanel {
    open: bool,
    width: f32,
    edge: PanelEdge,
    body: Element,
}

impl std::fmt::Debug for SidePanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SidePanel")
            .field("open", &self.open)
            .field("width", &self.width)
            .field("edge", &self.edge)
            .finish_non_exhaustive()
    }
}

impl SidePanel {
    /// `body` shows while `open`, and while the panel slides out after.
    #[must_use]
    pub fn new(open: bool, width: f32, body: impl IntoElement) -> Self {
        Self {
            open,
            width,
            edge: PanelEdge::default(),
            body: body.into_element(),
        }
    }

    #[must_use]
    pub const fn edge(mut self, edge: PanelEdge) -> Self {
        self.edge = edge;
        self
    }

    /// How far the panel is open, 0 to 1.
    #[cfg_attr(feature = "sismo", sismo::function("kit::side_panel_use_opened"))]
    fn use_opened(open: bool) -> f32 {
        let motion = use_animation_with_dependencies(&open, |conf, open| {
            conf.on_creation(OnCreation::Finish);
            conf.on_change(OnChange::Rerun);
            let (from, to) = if *open { (0.0, 1.0) } else { (1.0, 0.0) };
            AnimNum::new(from, to)
                .time(tokens::motion::SLOW)
                .ease(Ease::Out)
                .function(Function::Cubic)
        });
        if reduce_motion() {
            if open { 1.0 } else { 0.0 }
        } else {
            motion.get().value()
        }
    }
}

impl Component for SidePanel {
    #[cfg_attr(feature = "sismo", sismo::function("kit::side_panel"))]
    fn render(&self) -> impl IntoElement {
        let opened = Self::use_opened(self.open);
        if opened <= 0.0 {
            return rect().into_element();
        }
        let hidden = self.width * (1.0 - opened);
        rect()
            .width(Size::px(self.width * opened))
            .height(Size::fill())
            .overflow(Overflow::Clip)
            .child(
                rect()
                    .width(Size::px(self.width))
                    .height(Size::fill())
                    .offset_x(match self.edge {
                        PanelEdge::Left => -hidden,
                        PanelEdge::Right => hidden,
                    })
                    .child(self.body.clone()),
            )
            .into_element()
    }
}
