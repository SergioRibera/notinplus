//! What a scroll view scrolls. A finger that drags it scrolls and
//! nothing else: lifting it does not press what it lifted on.

use std::cell::Cell;
use std::rc::Rc;

use freya::prelude::*;

use super::press::SLOP_PX;

/// Where the finger went down in one [`ScrollContent`], and whether it
/// travelled since.
#[derive(Clone, Copy, Debug, Default)]
struct Touch {
    origin: Option<CursorPoint>,
    dragged: bool,
}

thread_local! {
    /// Where the last finger that scrolled lifted.
    ///
    /// A thread local, not a context: a press handler runs outside any
    /// render, and the rows of a sheet are built by whoever opens it, in
    /// a scope the scroll view is not an ancestor of. The exact point
    /// tells the lift's own press from any later one, so it never needs
    /// clearing.
    static LIFTED_AFTER_DRAG: Cell<Option<CursorPoint>> = const { Cell::new(None) };
}

fn travelled(from: CursorPoint, to: CursorPoint) -> bool {
    (to.x - from.x).hypot(to.y - from.y) >= SLOP_PX
}

/// Whether `press` is the lift of a finger that scrolled.
fn swallows(press: &PressEventData) -> bool {
    let PressEventData::Touch(touch) = press else {
        return false;
    };
    let at = touch.global_location;
    LIFTED_AFTER_DRAG
        .get()
        .is_some_and(|lift| (at.x - lift.x).abs() < 1.0 && (at.y - lift.y).abs() < 1.0)
}

impl Touch {
    fn moved(mut self, at: CursorPoint) -> Self {
        if let Some(origin) = self.origin {
            self.dragged |= travelled(origin, at);
        }
        self
    }

    /// The finger lifted at `at`. Capture events run first, so this
    /// lands before the press of the same lift.
    fn lifted(self, at: CursorPoint) {
        if self.origin.is_some() && self.moved(at).dragged {
            LIFTED_AFTER_DRAG.set(Some(at));
        }
    }
}

/// Wraps the child of a `ScrollView`, or a virtual scroll view itself.
///
/// The scroll view scrolls with the finger, but the lift at the end
/// still reaches the row under it as a press. Rows wired with
/// [`TapExt::on_tap`] ignore that press once the finger moved farther
/// than a tap allows.
///
/// It never cancels the lift itself: `prevent_default` on a capture
/// pointer-up drops every other capture pointer-up of that lift too,
/// the scroll view's own among them, which then keeps dragging from a
/// stale point and jumps on the next touch.
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
        // A plain cell: the gesture never needs a render of its own.
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
                moved.set(moved.get().moved(e.global_location()));
            })
            .on_capture_global_pointer_up(move |e: Event<PointerEventData>| {
                up.replace(Touch::default()).lifted(e.global_location());
            })
            .child(self.child.clone())
    }
}

/// `on_press` that ignores the lift of a finger that scrolled a
/// [`ScrollContent`]. Anywhere else it is `on_press`.
pub trait TapExt {
    #[must_use]
    fn on_tap(self, handler: impl Into<EventHandler<Event<PressEventData>>>) -> Self;
}

impl TapExt for Rect {
    fn on_tap(self, handler: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        let handler = handler.into();
        self.on_press(move |e: Event<PressEventData>| {
            if !swallows(e.data()) {
                handler.call(e);
            }
        })
    }
}
