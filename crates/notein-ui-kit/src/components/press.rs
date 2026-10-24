//! Press gesture: tap runs the primary action; holding still (finger or
//! mouse) or a secondary click asks for the context menu at the
//! pointer.

use std::time::Duration;

use freya::prelude::*;

/// Hold time before a press becomes a menu request.
pub const LONG_PRESS: Duration = Duration::from_millis(450);
/// Travel that turns a hold into a scroll or drag.
const SLOP_PX: f64 = 8.0;

/// Gesture state of one element. Create with [`use_press_gesture`] in
/// the component that owns the element.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PressGesture {
    token: State<u64>,
    held: State<bool>,
    start: State<Option<CursorPoint>>,
}

impl std::fmt::Debug for PressGesture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PressGesture").finish_non_exhaustive()
    }
}

/// Allocate gesture state in the calling component.
#[must_use]
pub fn use_press_gesture() -> PressGesture {
    PressGesture {
        token: use_state(|| 0),
        held: use_state(|| false),
        start: use_state(|| None),
    }
}

impl PressGesture {
    /// Wire `rect`: `on_tap` on a short press, `on_menu` with the global
    /// pointer position on a long press or a secondary click.
    #[must_use]
    pub fn apply(
        self,
        rect: Rect,
        on_tap: impl Fn() + 'static,
        on_menu: impl Fn(CursorPoint) + Clone + 'static,
    ) -> Rect {
        let Self {
            mut token,
            mut held,
            mut start,
        } = self;
        let menu_on_hold = on_menu.clone();
        rect.on_pointer_down(move |e: Event<PointerEventData>| {
            if !e.data().is_primary() {
                return;
            }
            held.set(false);
            let at = e.global_location();
            start.set(Some(at));
            let current = token.peek().wrapping_add(1);
            token.set(current);
            let menu = menu_on_hold.clone();
            spawn(async move {
                async_io::Timer::after(LONG_PRESS).await;
                if *token.peek() == current {
                    held.set(true);
                    menu(at);
                }
            });
        })
        .on_pointer_move(move |e: Event<PointerEventData>| {
            let Some(origin) = *start.peek() else {
                return;
            };
            let now = e.global_location();
            if (now.x - origin.x).hypot(now.y - origin.y) >= SLOP_PX {
                token.set(0);
                start.set(None);
            }
        })
        .on_pointer_leave(move |_| {
            token.set(0);
            start.set(None);
        })
        .on_press(move |_| {
            token.set(0);
            start.set(None);
            if !*held.peek() {
                on_tap();
            }
        })
        .on_secondary_down(move |e: Event<PressEventData>| {
            if let PressEventData::Mouse(mouse) = &*e.data() {
                on_menu(mouse.global_location);
            }
        })
    }
}
