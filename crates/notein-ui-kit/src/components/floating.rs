//! Floating UI that hangs from a point of the window and stays visible.

use freya::prelude::*;

use crate::viewport::{place_popup, safe_frame};

/// Space kept between floating UI and the edges it may not cross.
const MARGIN: f32 = 8.0;

/// A child laid over everything, next to `anchor` (window coordinates).
///
/// Under it, or over it when [`Floating::above`] is set, flipped when that
/// side has no room and always inside what the system bars and the
/// keyboard leave visible. [`Floating::bounds`] narrows that further, to
/// keep clear of the bars of the screen around it.
#[derive(Clone, PartialEq)]
pub struct Floating {
    anchor: Area,
    above: bool,
    centered: bool,
    bounds: Option<Area>,
    child: Element,
}

impl std::fmt::Debug for Floating {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Floating")
            .field("anchor", &self.anchor)
            .field("above", &self.above)
            .field("centered", &self.centered)
            .field("bounds", &self.bounds)
            .finish_non_exhaustive()
    }
}

impl Floating {
    #[must_use]
    pub fn new(anchor: Area, child: impl IntoElement) -> Self {
        Self {
            anchor,
            above: false,
            centered: false,
            bounds: None,
            child: child.into_element(),
        }
    }

    /// Prefer the side over the anchor.
    #[must_use]
    pub const fn above(mut self) -> Self {
        self.above = true;
        self
    }

    /// Centre the child on the anchor instead of lining up their left
    /// edges.
    #[must_use]
    pub const fn centered(mut self) -> Self {
        self.centered = true;
        self
    }

    /// Region (window coordinates) the child must also stay inside.
    #[must_use]
    pub const fn bounds(mut self, bounds: Area) -> Self {
        self.bounds = Some(bounds);
        self
    }
}

impl Component for Floating {
    fn render(&self) -> impl IntoElement {
        let mut size = use_state(Area::default);
        let frame = safe_frame();
        let bounds = self
            .bounds
            .and_then(|bounds| bounds.intersection(&frame))
            .unwrap_or(frame);
        let measured = *size.read();
        let anchor = if self.centered {
            Area::new(
                (
                    self.anchor.center().x - measured.width() / 2.0,
                    self.anchor.min_y(),
                )
                    .into(),
                self.anchor.size,
            )
        } else {
            self.anchor
        };
        let (left, top) = place_popup(
            anchor,
            (measured.width(), measured.height()),
            self.above,
            bounds,
            MARGIN,
        );
        rect()
            .position(Position::new_global().left(left).top(top))
            .layer(Layer::Overlay)
            .on_sized(move |e: Event<SizedEventData>| size.set_if_modified(e.area))
            .child(self.child.clone())
    }
}
