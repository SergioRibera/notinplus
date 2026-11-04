//! Content built only once it is about to scroll into view.

use freya::prelude::*;

/// Pixels past the viewport within which a [`Lazy`] mounts its child.
pub const OVERSCAN: f32 = 400.0;

/// Height a [`Lazy`] holds before its child has ever been measured.
const HEIGHT_HINT: f32 = 48.0;

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
        let near_now = *near.read();
        let mounted = self.pinned || near_now;
        let height = measured.read().unwrap_or(self.height_hint);
        let overscan = self.overscan;
        let recycle = self.recycle;
        // Once seen (and so mounted for good) the sensor and its
        // listeners go. A pinned child keeps sensing: when it is no
        // longer pinned it stays only if it was seen meanwhile.
        let sensing = !near_now || recycle;
        rect()
            .width(Size::fill())
            .maybe(!mounted, |strut| strut.height(Size::px(height)))
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
                element.child(
                    rect()
                        .width(Size::fill())
                        .on_sized(move |event: Event<SizedEventData>| {
                            measured.set_if_modified(Some(event.area.height()));
                        })
                        .child(self.child.clone()),
                )
            })
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}
