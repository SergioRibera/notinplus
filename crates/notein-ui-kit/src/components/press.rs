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
            if let PressEventData::Mouse(mouse) = e.data() {
                on_menu(mouse.global_location);
            }
        })
    }
}

/// Where a press happened, in window coordinates: the pointer or the
/// finger. A key press has no point of its own, so it counts as the middle
/// of what the system bars and the keyboard leave visible.
#[must_use]
pub fn press_point(data: &PressEventData) -> CursorPoint {
    match data {
        PressEventData::Mouse(mouse) => mouse.global_location,
        PressEventData::Touch(touch) => touch.global_location,
        PressEventData::Keyboard(_) => {
            let centre = crate::viewport::safe_frame().center();
            CursorPoint::new(f64::from(centre.x), f64::from(centre.y))
        }
    }
}

/// [`press_point`] as the one pixel area a popover hangs from.
#[must_use]
pub fn press_anchor(data: &PressEventData) -> Area {
    let at = press_point(data);
    #[allow(
        clippy::cast_possible_truncation,
        reason = "window coordinates fit an f32"
    )]
    Area::new((at.x as f32, at.y as f32).into(), (1.0, 1.0).into())
}

/// Whether a pointer event comes from a mouse. Hover follows only the
/// mouse: a finger fires enter when it lands but no leave when it lifts,
/// so a hover set by touch would stay on.
#[must_use]
pub fn is_mouse(e: &Event<PointerEventData>) -> bool {
    matches!(e.data(), PointerEventData::Mouse(_))
}
