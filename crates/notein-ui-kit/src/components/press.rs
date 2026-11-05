//! Press gesture: tap runs the primary action; holding still (finger or
//! mouse) or a secondary click asks for the context menu at the
//! pointer.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use freya::prelude::*;

use super::scroll_content::TapExt;

/// Hold time before a press becomes a menu request.
pub const LONG_PRESS: Duration = Duration::from_millis(450);
/// Travel that turns a hold into a scroll or drag.
pub(super) const SLOP_PX: f64 = 8.0;

/// Where the element sat on screen, now and when the pointer went down.
///
/// A scroll view that drags cancels the pointer moves of what it
/// scrolls, so the element cannot rely on them to notice it travelled;
/// its own area moving under a still finger says the same.
#[derive(Clone, Copy, Debug, Default)]
struct Placement {
    now: Area,
    at_down: Area,
}

impl Placement {
    fn travelled(self) -> bool {
        let (a, b) = (self.now.origin, self.at_down.origin);
        f64::from(a.x - b.x).hypot(f64::from(a.y - b.y)) >= SLOP_PX
    }
}

/// Gesture state of one element. Create with [`use_press_gesture`] in
/// the component that owns the element.
#[derive(Clone)]
pub struct PressGesture {
    token: State<u64>,
    held: State<bool>,
    start: State<Option<CursorPoint>>,
    /// Plain cell: layout moves it every scrolled frame, which must not
    /// render the element again.
    placement: Rc<Cell<Placement>>,
}

impl std::fmt::Debug for PressGesture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PressGesture").finish_non_exhaustive()
    }
}

/// Allocate gesture state in the calling component.
#[must_use]
#[cfg_attr(feature = "sismo", sismo::function("kit::use_press_gesture"))]
pub fn use_press_gesture() -> PressGesture {
    PressGesture {
        token: use_state(|| 0),
        held: use_state(|| false),
        start: use_state(|| None),
        placement: use_hook(|| Rc::new(Cell::new(Placement::default()))),
    }
}

impl PressGesture {
    /// Wire `rect`: `on_tap` on a short press, `on_menu` with the global
    /// pointer position on a long press or a secondary click.
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("kit::press_apply"))]
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
            placement,
        } = self;
        let (sized, down, timer, pressed) = (
            Rc::clone(&placement),
            Rc::clone(&placement),
            Rc::clone(&placement),
            placement,
        );
        rect.on_sized(move |e: Event<SizedEventData>| {
            let mut now = sized.get();
            now.now = e.area;
            sized.set(now);
        })
        // One pointer-down handler for both buttons: an element keeps one
        // handler per event, and `on_secondary_down` is a pointer-down
        // handler too, so wiring both drops the first.
        .on_pointer_down(move |e: Event<PointerEventData>| {
            if let PointerEventData::Mouse(mouse) = e.data()
                && mouse.button == Some(MouseButton::Right)
            {
                on_menu(mouse.global_location);
                return;
            }
            if !e.data().is_primary() {
                return;
            }
            held.set(false);
            let at = e.global_location();
            start.set(Some(at));
            let mut now = down.get();
            now.at_down = now.now;
            down.set(now);
            let current = token.peek().wrapping_add(1);
            token.set(current);
            let menu = on_menu.clone();
            let timer = Rc::clone(&timer);
            spawn(traced!("task::kit::press", async move {
                async_io::Timer::after(LONG_PRESS).await;
                if *token.peek() == current && !timer.get().travelled() {
                    held.set(true);
                    menu(at);
                }
            }));
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
        .on_tap(move |e: Event<PressEventData>| {
            token.set(0);
            // A pointer that travelled scrolled or dragged; it does not tap.
            let pointer = matches!(
                e.data(),
                PressEventData::Mouse(_) | PressEventData::Touch(_)
            );
            let still = !pointer || (start.peek().is_some() && !pressed.get().travelled());
            start.set(None);
            if still && !*held.peek() {
                on_tap();
            }
        })
    }
}

/// Where a press happened, in window coordinates: the pointer or the
/// finger.
///
/// A key or screen reader press has no point of its own, so it counts as
/// the middle of what the system bars and the keyboard leave visible.
#[must_use]
pub fn press_point(data: &PressEventData) -> CursorPoint {
    match data {
        PressEventData::Mouse(mouse) => mouse.global_location,
        PressEventData::Touch(touch) => touch.global_location,
        PressEventData::Keyboard(_) | PressEventData::Accessibility(_) => {
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
