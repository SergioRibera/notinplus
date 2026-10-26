//! What a scroll view scrolls. A finger that drags it scrolls and
//! nothing else: lifting it does not press what it lifted on.

use std::cell::Cell;
use std::rc::Rc;

use freya::prelude::*;

use super::press::SLOP_PX;

/// Where the finger went down, and whether it travelled since.
#[derive(Clone, Copy, Debug, Default)]
struct Touch {
    origin: Option<CursorPoint>,
    dragged: bool,
}

/// Wraps the child of a `ScrollView`, or a virtual scroll view itself.
///
/// The scroll view scrolls with the finger, but the lift at the end
/// still reaches the row under it as a press; this cancels that press
/// once the finger moved farther than a tap allows.
#[derive(Clone, PartialEq)]
pub struct ScrollContent {
    child: Element,
    width: Size,
    height: Size,
}

impl std::fmt::Debug for ScrollContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScrollContent").finish_non_exhaustive()
    }
}

impl ScrollContent {
    /// As wide as the scroll view, as tall as `child`.
    #[must_use]
    pub fn new(child: impl IntoElement) -> Self {
        Self {
            child: child.into_element(),
            width: Size::fill(),
            height: Size::auto(),
        }
    }

    #[must_use]
    pub fn width(mut self, width: Size) -> Self {
        self.width = width;
        self
    }

    #[must_use]
    pub fn height(mut self, height: Size) -> Self {
        self.height = height;
        self
    }
}

impl Component for ScrollContent {
    fn render(&self) -> impl IntoElement {
        // Plain cells: the gesture never needs a render of its own.
        let touch = use_hook(|| Rc::new(Cell::new(Touch::default())));
        let (down, moved, up) = (Rc::clone(&touch), Rc::clone(&touch), touch);
        rect()
            .width(self.width.clone())
            .height(self.height.clone())
            .on_pointer_down(move |e: Event<PointerEventData>| {
                if matches!(e.data(), PointerEventData::Touch(_)) {
                    down.set(Touch {
                        origin: Some(e.global_location()),
                        dragged: false,
                    });
                }
            })
            .on_capture_global_pointer_move(move |e: Event<PointerEventData>| {
                let mut now = moved.get();
                if let Some(origin) = now.origin
                    && !now.dragged
                {
                    let at = e.global_location();
                    now.dragged = (at.x - origin.x).hypot(at.y - origin.y) >= SLOP_PX;
                    moved.set(now);
                }
            })
            .on_capture_global_pointer_up(move |e: Event<PointerEventData>| {
                if up.replace(Touch::default()).dragged {
                    e.prevent_default();
                }
            })
            .child(self.child.clone())
    }
}
