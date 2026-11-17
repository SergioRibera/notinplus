//! Content built only once it is about to scroll into view.

use std::cell::Cell;
use std::rc::Rc;

use freya::prelude::*;

/// Pixels past the viewport within which a [`Lazy`] mounts its child.
pub const OVERSCAN: f32 = 400.0;

/// Height a [`Lazy`] holds before its child has ever been measured.
const HEIGHT_HINT: f32 = 48.0;

/// Keeps what a scroll view shows where it is while content above the
/// view changes height.
///
/// Text typed there by someone else, a [`Lazy`] mounting with its real
/// height: provided with [`use_scroll_anchor`]
/// around the scroll view; every [`Lazy`] below it then lets a change of
/// height above the view land in the same frame as the scroll that makes
/// up for it, so nothing in view moves.
#[derive(Clone)]
pub struct ScrollAnchor {
    scroll: ScrollController,
    /// Window position of the top edge of the view.
    view_top: Rc<Cell<f32>>,
    /// Fraction of a pixel not scrolled yet, so rounding never drifts.
    carry: Rc<Cell<f32>>,
}

impl std::fmt::Debug for ScrollAnchor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScrollAnchor")
            .field("view_top", &self.view_top.get())
            .finish_non_exhaustive()
    }
}

impl ScrollAnchor {
    /// The scroll view was laid out at `area`, in window coordinates.
    pub fn view(&self, area: Area) {
        self.view_top.set(area.min_y());
    }

    /// Whether content whose top edge is at `top` (window coordinates)
    /// starts above the view.
    #[must_use]
    pub fn is_above(&self, top: f32) -> bool {
        top < self.view_top.get() - 0.5
    }

    /// Content whose top edge is at `top` grew by `delta` (shrank when
    /// negative): when it starts above the view, scroll by as much.
    pub fn resized(&self, top: f32, delta: f32) {
        if delta.abs() < f32::EPSILON || !self.is_above(top) {
            return;
        }
        let pending = self.carry.get() + delta;
        let whole = pending.round();
        self.carry.set(pending - whole);
        let mut scroll = self.scroll;
        let (_, y): (i32, i32) = scroll.into();
        #[allow(clippy::cast_possible_truncation, reason = "pixel offsets")]
        scroll.scroll_to_y(y - whole as i32);
    }
}

/// A [`ScrollAnchor`] for the scroll view `scroll` drives, provided to
/// everything below. Report the view with [`ScrollAnchor::view`] from the
/// scroll view's `on_sized`.
#[must_use]
pub fn use_scroll_anchor(scroll: ScrollController) -> ScrollAnchor {
    use_provide_context(|| ScrollAnchor {
        scroll,
        view_top: Rc::new(Cell::new(f32::NEG_INFINITY)),
        carry: Rc::default(),
    })
}

/// A child mounted only when it is about to be visible.
///
/// Until then a strut of [`Lazy::height_hint`] holds its place, so the
/// content of a scroll view keeps its length and the scrollbar its shape.
/// An invisible sensor, [`Lazy::overscan`] pixels taller than the strut on
/// each side, listens for visibility inside every clipping ancestor: a
/// `ScrollView`, the body of a sheet. Outside any of them the child mounts
/// at once, so a `Lazy` costs nothing where nothing scrolls.
///
/// Once mounted the child stays and its measured height replaces the hint,
/// unless [`Lazy::recycle`] turns it back into a strut (of that measured
/// height) when it scrolls away again.
#[derive(Clone, PartialEq)]
pub struct Lazy {
    child: Element,
    height_hint: f32,
    overscan: f32,
    pinned: bool,
    recycle: bool,
    key: DiffKey,
}

impl std::fmt::Debug for Lazy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lazy")
            .field("height_hint", &self.height_hint)
            .field("overscan", &self.overscan)
            .field("pinned", &self.pinned)
            .field("recycle", &self.recycle)
            .finish_non_exhaustive()
    }
}

impl Lazy {
    #[must_use]
    pub fn new(child: impl IntoElement) -> Self {
        Self {
            child: child.into_element(),
            height_hint: HEIGHT_HINT,
            overscan: OVERSCAN,
            pinned: false,
            recycle: false,
            key: DiffKey::None,
        }
    }

    /// Height the strut takes until the child is measured. The closer
    /// it is to the real height, the less the scrollbar moves as
    /// children mount.
    #[must_use]
    pub const fn height_hint(mut self, height: f32) -> Self {
        self.height_hint = height;
        self
    }

    /// How far past the viewport the child mounts, so it is ready before
    /// a fast scroll reaches it. Defaults to [`OVERSCAN`].
    #[must_use]
    pub const fn overscan(mut self, overscan: f32) -> Self {
        self.overscan = overscan;
        self
    }

    /// Mount the child whether or not it is visible: something off screen
    /// still needs it, like the caret of an editor.
    #[must_use]
    pub const fn pinned(mut self, pinned: bool) -> Self {
        self.pinned = pinned;
        self
    }

    /// Unmount the child again when it leaves the viewport and its
    /// overscan. Only for children that keep no state of their own.
    #[must_use]
    pub const fn recycle(mut self, recycle: bool) -> Self {
        self.recycle = recycle;
        self
    }
}

impl KeyExt for Lazy {
    fn key_mut(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for Lazy {
    #[cfg_attr(feature = "sismo", sismo::function("kit::lazy"))]
    fn render(&self) -> impl IntoElement {
        let mut near = use_state(|| false);
        let mut measured = use_state(|| None::<f32>);
        // Window position of the top edge, from the latest layout.
        let top = use_hook(|| Rc::new(Cell::new(f32::INFINITY)));
        let anchor = try_consume_context::<ScrollAnchor>().filter(|_| !self.pinned);
        let near_now = *near.read();
        let mounted = self.pinned || near_now;
        let height = measured.read().unwrap_or(self.height_hint);
        let hint = self.height_hint;
        let overscan = self.overscan;
        let recycle = self.recycle;
        // Above the view the child keeps the height it had and grows
        // upwards out of sight, until its new height lands together with
        // the scroll that keeps the view where it was.
        let mut above = use_state(|| false);
        let held = mounted && anchor.is_some() && *above.read();
        // Once seen (and so mounted for good) the sensor and its
        // listeners go. A pinned child keeps sensing: when it is no
        // longer pinned it stays only if it was seen meanwhile.
        let sensing = !near_now || recycle;
        let child = rect()
            .width(Size::fill())
            .on_sized({
                let (anchor, top) = (anchor.clone(), Rc::clone(&top));
                move |event: Event<SizedEventData>| {
                    let now = event.area.height();
                    let before = measured.peek().unwrap_or(hint);
                    if let Some(anchor) = &anchor {
                        anchor.resized(top.get(), now - before);
                    }
                    measured.set_if_modified(Some(now));
                }
            })
            .child(self.child.clone());
        rect()
            .width(Size::fill())
            .maybe(!mounted, |strut| strut.height(Size::px(height)))
            .map(anchor, |element, anchor| {
                // Renders again only when the edge crosses the top of the view.
                element.on_sized(move |event: Event<SizedEventData>| {
                    top.set(event.area.min_y());
                    above.set_if_modified(anchor.is_above(event.area.min_y()));
                })
            })
            .maybe(sensing, |element| {
                element.child(
                    rect()
                        .position(Position::new_absolute().top(-overscan).left(0.0))
                        .width(Size::fill())
                        .height(Size::px(2.0f32.mul_add(overscan, height)))
                        .on_visible(move |_| near.set_if_modified(true))
                        .maybe(recycle, |sensor| {
                            sensor.on_hidden(move |_| near.set_if_modified(false))
                        }),
                )
            })
            .maybe(mounted, |element| {
                // Always the same box, so holding never rebuilds the child.
                element.child(
                    rect()
                        .width(Size::fill())
                        .maybe(held, |held| {
                            held.height(Size::px(height))
                                .overflow(Overflow::Clip)
                                .main_align(Alignment::End)
                        })
                        .child(child),
                )
            })
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}
