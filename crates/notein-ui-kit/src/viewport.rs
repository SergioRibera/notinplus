//! Window-area context for popup placement.
//!
//! `AppShell` provides a [`ViewportArea`] via `provide_context`;
//! popovers consume it to clamp their global position inside the
//! visible window. Without the context, popups fall back to a naive
//! "anchor below" placement (fine for small windows / desktop but
//! can clip on mobile when the trigger sits near the bottom edge).

use freya::prelude::{Area, State};

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
/// (gesture location) and not a full trigger area. `anchor_y` is the
/// trigger line in viewport coords; `below_top` is where the popup
/// would sit when placed below. Flips above the anchor if below would
/// overflow.
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
