//! Dragging a bottom sheet down to put it away, the way Material UI's
//! swipeable drawer does.
//!
//! - The sheet follows the finger one to one once it moved a few pixels
//!   more down or up than sideways, and never rises past where it rests.
//! - Dragging over its content moves the sheet only while that content is
//!   scrolled to the top and the finger goes down; otherwise it scrolls.
//! - On release it closes when flung down fast enough or dragged past half
//!   its height, and glides back otherwise.
//! - The veil behind clears as the sheet goes down.
//!
//! The header, the handle and the footer drag with the mouse too; the
//! content only with a finger, since a mouse drag there selects or scrolls.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use freya::components::EnclosingScrollViews;
use freya::prelude::*;

use crate::modal::{ModalController, ModalDrag};

/// Travel before a press becomes a drag or a scroll.
const UNCERTAINTY_PX: f64 = 3.0;
/// Share of its height past which a released sheet closes.
const HYSTERESIS: f32 = 0.52;
/// Downward speed, in px/s, past which a released sheet closes.
const MIN_FLING: f32 = 450.0;
/// How long the sheet takes to glide back to rest, or out.
const SETTLE_MS: f32 = 225.0;
const LEAVE_MS: f32 = 170.0;

/// Who a press over the sheet ends up moving.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Claim {
    #[default]
    Undecided,
    Sheet,
    /// The content scrolls, or the mouse selects in it.
    Content,
}

#[derive(Clone, Copy, Debug, Default)]
struct Gesture {
    origin: Option<CursorPoint>,
    from_content: bool,
    claim: Claim,
    /// Offset when the press went down.
    start: f32,
    /// Offset and time of the last move.
    last: Option<(f32, Instant)>,
    /// Smoothed speed, px/s, positive downwards.
    velocity: f32,
}

/// The drag state of one sheet; create with [`SheetDrag::use_new`] in the
/// component that renders it.
#[derive(Clone)]
pub(super) struct SheetDrag {
    enabled: bool,
    offset: State<f32>,
    height: State<f32>,
    scroll: ScrollController,
    gesture: Rc<Cell<Gesture>>,
    /// The scroll view of the content, and those around it.
    enclosing: Rc<RefCell<Option<EnclosingScrollViews>>>,
    /// The veil of the modal around the sheet, cleared as it goes down.
    veil: Option<State<f32>>,
    /// Bumped by every glide; an older one stops when it sees another.
    glide: Rc<Cell<u64>>,
}

impl SheetDrag {
    /// `height` is the measured height of the sheet; `scroll`, the
    /// controller of its content.
    pub(super) fn use_new(enabled: bool, height: State<f32>, scroll: ScrollController) -> Self {
        Self {
            enabled,
            offset: use_state(|| 0.0),
            height,
            scroll,
            gesture: use_hook(|| Rc::new(Cell::new(Gesture::default()))),
            enclosing: use_hook(Rc::default),
            veil: try_consume_context::<ModalDrag>().map(|ModalDrag(veil)| veil),
            glide: use_hook(|| Rc::new(Cell::new(0))),
        }
    }

    /// How far down the sheet sits now.
    pub(super) fn offset(&self) -> f32 {
        *self.offset.read()
    }

    /// Wire the whole sheet: presses outside the content start a drag,
    /// and every move and release of a drag goes through here.
    pub(super) fn sheet(&self, rect: Rect) -> Rect {
        if !self.enabled {
            return rect;
        }
        let (down, moved, up) = (self.clone(), self.clone(), self.clone());
        rect.on_pointer_down(move |e: Event<PointerEventData>| {
            // The content saw this press first and claimed it.
            if down.gesture.get().origin.is_none() && e.data().is_primary() {
                down.press(e.global_location(), false);
            }
        })
        .on_capture_global_pointer_move(move |e: Event<PointerEventData>| {
            if moved.drag_to(e.global_location(), false) {
                e.prevent_default();
            }
        })
        .on_capture_global_pointer_up(move |e: Event<PointerEventData>| {
            if up.release() {
                // The lift ends the drag; it presses nothing.
                e.prevent_default();
            }
        })
    }

    /// Wire the content: a finger drags the sheet from it only while it
    /// is scrolled to the top. Its moves are read inside the scroll view,
    /// before the scroll view takes them.
    fn content(&self, rect: Rect) -> Rect {
        if !self.enabled {
            return rect;
        }
        let (this, moved) = (self.clone(), self.clone());
        rect.on_capture_global_pointer_move(move |e: Event<PointerEventData>| {
            if moved.drag_to(e.global_location(), true) {
                e.prevent_default();
            }
        })
        .on_pointer_down(move |e: Event<PointerEventData>| {
            if !e.data().is_primary() {
                return;
            }
            this.press(e.global_location(), true);
            if !matches!(e.data(), PointerEventData::Touch(_)) {
                let mut g = this.gesture.get();
                g.claim = Claim::Content;
                this.gesture.set(g);
            }
        })
    }

    fn press(&self, at: CursorPoint, from_content: bool) {
        // A press stops a glide where it is, and the finger takes it from
        // there.
        self.glide.set(self.glide.get().wrapping_add(1));
        let start = *self.offset.peek();
        self.gesture.set(Gesture {
            origin: Some(at),
            from_content,
            start,
            last: Some((start, Instant::now())),
            ..Gesture::default()
        });
    }

    /// Follow the pointer; `true` while it moves the sheet. The content and
    /// the rest of the sheet each follow only the presses they started.
    fn drag_to(&self, at: CursorPoint, via_content: bool) -> bool {
        let mut g = self.gesture.get();
        let Some(origin) = g.origin.filter(|_| g.from_content == via_content) else {
            return false;
        };
        let (dx, dy) = (at.x - origin.x, at.y - origin.y);
        if g.claim == Claim::Undecided {
            if dx.abs().max(dy.abs()) < UNCERTAINTY_PX {
                return false;
            }
            let vertical = dy.abs() > dx.abs();
            let at_top = {
                let (_, y): (i32, i32) = self.scroll.into();
                y >= 0
            };
            g.claim = if !vertical {
                Claim::Content
            } else if !g.from_content || (dy > 0.0 && at_top) || g.start > 0.0 {
                Claim::Sheet
            } else {
                Claim::Content
            };
            if g.claim == Claim::Sheet
                && let Some(enclosing) = self.enclosing.borrow().as_ref()
            {
                // The content stays where it is while the sheet moves.
                enclosing.take_over_drag();
            }
        }
        if g.claim != Claim::Sheet {
            self.gesture.set(g);
            return false;
        }
        #[allow(
            clippy::cast_possible_truncation,
            reason = "window coordinates fit an f32"
        )]
        let offset = (g.start + dy as f32).clamp(0.0, self.room());
        let now = Instant::now();
        if let Some((before, then)) = g.last {
            let elapsed = now.duration_since(then).as_secs_f32();
            if elapsed > 0.0 {
                let current = (offset - before) / elapsed;
                g.velocity = g.velocity.mul_add(0.4, current * 0.6);
            }
        }
        g.last = Some((offset, now));
        self.gesture.set(g);
        self.set_offset(offset);
        true
    }

    /// End the press; `true` when it was dragging the sheet.
    fn release(&self) -> bool {
        let g = self.gesture.replace(Gesture::default());
        if g.claim != Claim::Sheet {
            return false;
        }
        let offset = *self.offset.peek();
        let room = self.room();
        // A finger that stopped before lifting is not flinging.
        let resting = g
            .last
            .is_some_and(|(_, then)| then.elapsed() > Duration::from_millis(100));
        let velocity = if resting { 0.0 } else { g.velocity };
        if velocity > MIN_FLING || offset > room * HYSTERESIS {
            self.glide_to(room, LEAVE_MS * (1.0 - offset / room).max(0.0));
            ModalController::get().close();
        } else {
            self.glide_to(0.0, SETTLE_MS * (offset / room).clamp(0.3, 1.0));
        }
        true
    }

    /// The farthest the sheet goes down: all of it.
    fn room(&self) -> f32 {
        self.height.peek().max(1.0)
    }

    fn set_offset(&self, offset: f32) {
        let mut state = self.offset;
        state.set(offset);
        if let Some(mut veil) = self.veil {
            veil.set_if_modified(offset / self.room());
        }
    }

    /// Ease the sheet to `to` over `ms`, decelerating.
    fn glide_to(&self, to: f32, ms: f32) {
        let token = self.glide.get().wrapping_add(1);
        self.glide.set(token);
        let from = *self.offset.peek();
        if crate::theme::reduce_motion() || ms <= 0.0 || (from - to).abs() < 0.5 {
            self.set_offset(to);
            return;
        }
        let this = self.clone();
        spawn(async move {
            let started = Instant::now();
            loop {
                async_io::Timer::after(Duration::from_millis(16)).await;
                if this.glide.get() != token {
                    return;
                }
                let t = (started.elapsed().as_secs_f32() * 1000.0 / ms).min(1.0);
                let eased = 1.0 - (1.0 - t).powi(3);
                this.set_offset((to - from).mul_add(eased, from));
                if t >= 1.0 {
                    return;
                }
            }
        });
    }
}

impl PartialEq for SheetDrag {
    fn eq(&self, other: &Self) -> bool {
        self.enabled == other.enabled && Rc::ptr_eq(&self.gesture, &other.gesture)
    }
}

/// The content of a sheet, inside its scroll view: where a finger can take
/// the sheet down from.
#[derive(Clone, PartialEq)]
pub(super) struct SheetContent {
    pub(super) drag: SheetDrag,
    pub(super) body: Element,
}

impl Component for SheetContent {
    fn render(&self) -> impl IntoElement {
        *self.drag.enclosing.borrow_mut() = Some(EnclosingScrollViews::get());
        self.drag
            .content(rect().width(Size::fill()))
            .child(self.body.clone())
    }
}
