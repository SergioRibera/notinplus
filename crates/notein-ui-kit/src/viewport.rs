//! Window-area context for popup placement.
//!
//! `AppShell` provides a [`ViewportArea`] via `provide_context`;
//! popovers consume it to clamp their global position inside the
//! visible window. Without the context, popups fall back to a naive
//! "anchor below" placement (fine for small windows / desktop but
//! can clip on mobile when the trigger sits near the bottom edge).

use freya::prelude::{Area, Platform, State, try_consume_context};

use crate::modal::ModalChrome;

/// Shared window-area signal. Store the `State<Area>` of a top-level
/// rect that `on_sized`-captures the full window.
#[derive(Clone, Copy)]
pub struct ViewportArea(pub State<Area>);

impl PartialEq for ViewportArea {
    fn eq(&self, _other: &Self) -> bool {
        // Instance equality is enough — the inner signal does the diffing.
        true
    }
}

impl std::fmt::Debug for ViewportArea {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViewportArea").finish_non_exhaustive()
    }
}

impl ViewportArea {
    /// Read the current viewport area. Default (0×0) when the probe
    /// has not been laid out yet; callers should treat a zero-sized
    /// area as "unknown" and skip clamping.
    #[must_use]
    pub fn get(self) -> Area {
        *self.0.read()
    }
}

/// Scoped sibling of [`ViewportArea`] for popups that must stay inside
/// a specific sub-region of the window (a canvas pane, a split-view
/// half, etc.) instead of the whole viewport.
///
/// Shells install one of these per pane; descendants consume it to clamp
/// against the pane's bounds.
#[derive(Clone, Copy)]
pub struct PaneArea(pub State<Area>);

impl PartialEq for PaneArea {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for PaneArea {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaneArea").finish_non_exhaustive()
    }
}

impl PaneArea {
    #[must_use]
    pub fn get(self) -> Area {
        *self.0.read()
    }
}

/// Compute a vertical top offset for a popup anchored to `trigger`.
/// `below_top` is the default position (just below the trigger);
/// `popup_height` is the popup's own (or capped) height.
///
/// Flips above the trigger when placing below would overflow the
/// viewport. When neither direction fits (popup larger than the whole
/// viewport) the result clamps against the viewport top so at least
/// the start of the popup stays visible.
#[must_use]
pub fn clamp_popup_top(
    viewport: Area,
    trigger: Area,
    below_top: f32,
    popup_height: f32,
    margin: f32,
) -> f32 {
    if viewport.width() <= 0.0 || viewport.height() <= 0.0 {
        return below_top;
    }
    let viewport_bottom = viewport.max_y();
    let viewport_top = viewport.min_y();
    let fits_below = below_top + popup_height + margin <= viewport_bottom;
    if fits_below {
        return below_top;
    }
    let above_top = trigger.min_y() - popup_height - margin;
    if above_top >= viewport_top + margin {
        above_top
    } else {
        (viewport_bottom - popup_height - margin).max(viewport_top + margin)
    }
}

/// Horizontal clamp — pins the popup inside the viewport width with
/// `margin` keep-out on both sides. Returns the adjusted left offset.
#[must_use]
pub fn clamp_popup_left(viewport: Area, left: f32, popup_width: f32, margin: f32) -> f32 {
    if viewport.width() <= 0.0 {
        return left;
    }
    let min_left = viewport.min_x() + margin;
    let max_left = viewport.max_x() - popup_width - margin;
    if max_left <= min_left {
        min_left
    } else {
        left.clamp(min_left, max_left)
    }
}

/// Point-based variant for callers that only know the anchor Y
/// (gesture location) and not a full trigger area.
///
/// `anchor_y` is the trigger line in viewport coords; `below_top` is where the
/// popup would sit when placed below. Flips above the anchor if below would
/// overflow.
#[must_use]
pub fn clamp_popup_top_from_point(
    viewport: Area,
    anchor_y: f32,
    below_top: f32,
    popup_height: f32,
    margin: f32,
) -> f32 {
    if viewport.width() <= 0.0 || viewport.height() <= 0.0 {
        return below_top;
    }
    let viewport_bottom = viewport.max_y();
    let viewport_top = viewport.min_y();
    let fits_below = below_top + popup_height + margin <= viewport_bottom;
    if fits_below {
        return below_top;
    }
    let above_top = anchor_y - popup_height - margin;
    if above_top >= viewport_top + margin {
        above_top
    } else {
        (viewport_bottom - popup_height - margin).max(viewport_top + margin)
    }
}

/// Edges of the window that system bars, display cutouts and the soft
/// keyboard cover, in layout units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Gutters {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

impl Gutters {
    /// `area` without the covered edges.
    #[must_use]
    pub fn inside(self, area: Area) -> Area {
        Area::new(
            (area.min_x() + self.left, area.min_y() + self.top).into(),
            (
                (area.width() - self.left - self.right).max(0.0),
                (area.height() - self.top - self.bottom).max(0.0),
            )
                .into(),
        )
    }
}

/// The part of the window floating UI may use, in window coordinates:
/// the window without the edges the host's [`ModalChrome`] reports as
/// covered. Subscribes the caller to the window size and the gutters.
#[must_use]
pub fn safe_frame() -> Area {
    let window = try_consume_context::<ViewportArea>()
        .map(ViewportArea::get)
        .filter(|area| area.width() > 0.0 && area.height() > 0.0)
        .unwrap_or_else(|| Area::from_size(*Platform::get().root_size.read()));
    try_consume_context::<ModalChrome>()
        .unwrap_or_default()
        .gutters()
        .inside(window)
}

/// Where a popup sized `size` goes for a trigger at `anchor`, as its
/// `(left, top)`.
///
/// Under the trigger (over it when `above` is preferred), flipped to the other side when it does not fit, and always inside
/// `bounds` less `margin`. A popup larger than `bounds` starts at its top.
#[must_use]
pub fn place_popup(
    anchor: Area,
    size: (f32, f32),
    above: bool,
    bounds: Area,
    margin: f32,
) -> (f32, f32) {
    const GAP: f32 = 8.0;
    let (width, height) = size;
    let (min_top, max_bottom) = (bounds.min_y() + margin, bounds.max_y() - margin);
    let below = anchor.max_y() + GAP;
    let over = anchor.min_y() - GAP - height;
    let fits_below = below + height <= max_bottom;
    let fits_over = over >= min_top;
    let top = match (above, fits_below, fits_over) {
        (false, true, _) | (true, true, false) => below,
        (true, _, true) | (false, false, true) => over,
        (_, false, false) => (max_bottom - height).max(min_top),
    };
    let min_left = bounds.min_x() + margin;
    let max_left = (bounds.max_x() - width - margin).max(min_left);
    (anchor.min_x().clamp(min_left, max_left), top)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(x: f32, y: f32, w: f32, h: f32) -> Area {
        Area::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn a_popup_near_the_bottom_opens_over_its_trigger() {
        let bounds = area(0.0, 0.0, 800.0, 600.0);
        let (_, top) = place_popup(
            area(10.0, 540.0, 40.0, 30.0),
            (200.0, 160.0),
            false,
            bounds,
            8.0,
        );
        assert!((top - (540.0 - 8.0 - 160.0)).abs() < 1e-4);
    }

    #[test]
    fn a_popup_never_crosses_the_right_edge() {
        let bounds = area(0.0, 0.0, 800.0, 600.0);
        let (left, _) = place_popup(
            area(760.0, 40.0, 30.0, 30.0),
            (200.0, 100.0),
            false,
            bounds,
            8.0,
        );
        assert!((left - (800.0 - 8.0 - 200.0)).abs() < 1e-4);
    }

    #[test]
    fn a_popup_taller_than_the_bounds_starts_at_their_top() {
        let bounds = area(0.0, 24.0, 800.0, 300.0);
        let (_, top) = place_popup(
            area(10.0, 100.0, 30.0, 30.0),
            (200.0, 500.0),
            false,
            bounds,
            8.0,
        );
        assert!((top - 32.0).abs() < 1e-4);
    }

    #[test]
    fn gutters_cut_every_edge() {
        let inside = Gutters {
            top: 24.0,
            right: 0.0,
            bottom: 300.0,
            left: 10.0,
        }
        .inside(area(0.0, 0.0, 800.0, 1000.0));
        assert_eq!(inside, area(10.0, 24.0, 790.0, 676.0));
    }
}
