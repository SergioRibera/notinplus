//! Press gesture shared by every Home surface with a context menu.
//!
//! Mesa chips, folder and document cards all behave the same: a tap
//! runs the primary action, holding still for a moment or a secondary
//! click opens the menu.
//!
//! Secondary clicks go through [`ContextMenu::open_from_down`]: the
//! pointer-down that opens the menu would otherwise count as the
//! "next click" that closes it, and the menu would vanish on release.
//! A long press opens on a timer while the pointer is still down, so
//! the plain [`ContextMenu::open`] is right there.

use std::time::Duration;

use freya::prelude::*;

/// Hold time (mouse or finger) before the menu opens.
const LONG_PRESS: Duration = Duration::from_millis(450);
/// Travel that turns a hold into a scroll or a drag instead of a menu.
const SLOP_PX: f64 = 8.0;

/// Per-element gesture state. Create it with [`use_menu_gesture`]
/// inside the component owning the element, then [`MenuGesture::apply`]
/// it to that element's rect.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MenuGesture {
    press_token: State<u64>,
    long_fired: State<bool>,
    press_start: State<Option<CursorPoint>>,
}

/// Allocate the gesture state in the calling component's scope.
#[must_use]
pub fn use_menu_gesture() -> MenuGesture {
    MenuGesture {
        press_token: use_state(|| 0),
        long_fired: use_state(|| false),
        press_start: use_state(|| None),
    }
}

impl MenuGesture {
    /// Wire `rect` so a tap runs `tap`, and a long press or secondary
    /// click opens the menu `menu` builds.
    #[must_use]
    pub fn apply(
        self,
        rect: Rect,
        menu: impl Fn() -> Menu + Clone + 'static,
        tap: impl Fn() + 'static,
    ) -> Rect {
        let Self {
            mut press_token,
            mut long_fired,
            mut press_start,
        } = self;
        let menu_for_long = menu.clone();

        rect.on_pointer_down(move |e: Event<PointerEventData>| {
            long_fired.set(false);
            press_start.set(Some(e.global_location()));
            let token = press_token.peek().wrapping_add(1);
            press_token.set(token);
            let menu = menu_for_long.clone();
            // Scoped on purpose: an unmounted element must not open a
            // menu, and the states read below die with it.
            spawn(async move {
                async_io::Timer::after(LONG_PRESS).await;
                if *press_token.peek() == token {
                    long_fired.set(true);
                    ContextMenu::open(menu());
                }
            });
        })
        .on_pointer_move(move |e: Event<PointerEventData>| {
            let Some(start) = *press_start.peek() else {
                return;
            };
            let current = e.global_location();
            let dx = current.x - start.x;
            let dy = current.y - start.y;
            if dx.hypot(dy) >= SLOP_PX {
                press_token.set(0);
                press_start.set(None);
            }
        })
        .on_pointer_leave(move |_: Event<PointerEventData>| {
            press_token.set(0);
            press_start.set(None);
        })
        .on_press(move |_: Event<PressEventData>| {
            press_token.set(0);
            press_start.set(None);
            if !*long_fired.peek() {
                tap();
            }
        })
        .on_secondary_down(move |_: Event<PressEventData>| {
            ContextMenu::open_from_down(menu());
        })
    }
}
