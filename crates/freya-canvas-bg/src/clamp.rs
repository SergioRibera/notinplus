//! Viewport translation clamping.
//!
//! A background exposing [`content_bounds`](crate::CanvasBackground::content_bounds)
//! lets the host pin the viewport so the user can't drift into
//! infinity — standard PDF-viewer feel.
//!
//! The clamp is affine-transform aware: pass the current translation +
//! scale + surface size and receive the corrected translation. Content
//! smaller than the surface along either axis snaps to centred;
//! content larger pins so edges never leave the viewport.

use crate::Rect;

/// Clamp `(tx, ty)` so the axis-aligned world rect `bounds` stays
/// visible under a viewport of `(surface_w, surface_h)` screen pixels
/// at `scale` world→screen ratio.
///
/// Screen mapping: `screen = world * scale + t`.
///
/// # Axis behaviour
///
/// - `bounds` narrower than the surface → translation snaps so
///   `bounds` is centred on the surface (can't push it off-screen).
/// - `bounds` wider than the surface → translation clamps so
///   `bounds.min * scale + t <= 0` and `bounds.max * scale + t >= surface`
///   (edges stay flush or inside).
///
/// A zero or negative surface dimension short-circuits to the input
/// translation on that axis — nothing sensible to clamp against.
#[must_use]
pub fn clamp_translation(
    tx: f32,
    ty: f32,
    scale: f32,
    bounds: Rect,
    surface_w: f32,
    surface_h: f32,
) -> (f32, f32) {
    let cx = clamp_axis(tx, scale, bounds.min_x, bounds.max_x, surface_w);
    let cy = clamp_axis(ty, scale, bounds.min_y, bounds.max_y, surface_h);
    (cx, cy)
}

fn clamp_axis(t: f32, scale: f32, min_w: f32, max_w: f32, surface: f32) -> f32 {
    if !surface.is_finite() || surface <= 0.0 || !scale.is_finite() || scale <= 0.0 {
        return t;
    }
    let extent_screen = (max_w - min_w) * scale;
    if extent_screen <= surface {
        // Content narrower than viewport — centre it.
        let centre_world = (min_w + max_w) * 0.5;
        surface.mul_add(0.5, -(centre_world * scale))
    } else {
        // Content wider than viewport — pin edges. `tx <= -min * scale`
        // keeps the left edge at or off-left; `tx >= surface - max * scale`
        // keeps the right edge at or off-right.
        let lo = max_w.mul_add(-scale, surface);
        let hi = -min_w * scale;
        t.clamp(lo, hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> Rect {
        Rect {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    #[test]
    fn centres_when_content_smaller_than_surface() {
        // 100-wide bounds inside 500-wide surface at scale 1.0 →
        // centred means tx puts the centre of bounds at surface centre.
        let (tx, _) = clamp_translation(0.0, 0.0, 1.0, rect(0.0, 0.0, 100.0, 100.0), 500.0, 500.0);
        // centre_world = 50; surface_w/2 = 250; tx = 250 - 50 = 200.
        assert!((tx - 200.0).abs() < 1e-4);
    }

    #[test]
    fn pins_edges_when_content_wider_than_surface() {
        // 1000-wide bounds inside 400-wide surface at scale 1.0 →
        // valid tx range is [400 - 1000, 0] = [-600, 0].
        let (tx_hi, _) = clamp_translation(
            1_000.0,
            0.0,
            1.0,
            rect(0.0, 0.0, 1_000.0, 500.0),
            400.0,
            500.0,
        );
        assert!((tx_hi - 0.0).abs() < 1e-4);
        let (tx_lo, _) = clamp_translation(
            -9_999.0,
            0.0,
            1.0,
            rect(0.0, 0.0, 1_000.0, 500.0),
            400.0,
            500.0,
        );
        assert!((tx_lo + 600.0).abs() < 1e-4);
    }

    #[test]
    fn scale_shrinks_effective_extent() {
        // 1000-wide bounds at scale 0.2 → 200 on screen, fits in 400
        // surface → centres.
        let (tx, _) = clamp_translation(
            0.0,
            0.0,
            0.2,
            rect(0.0, 0.0, 1_000.0, 1_000.0),
            400.0,
            400.0,
        );
        // centre_world = 500; 400*0.5 - 500*0.2 = 200 - 100 = 100.
        assert!((tx - 100.0).abs() < 1e-4);
    }

    #[test]
    fn non_positive_surface_returns_input() {
        let (tx, ty) = clamp_translation(42.0, -17.0, 1.0, rect(0.0, 0.0, 10.0, 10.0), 0.0, -5.0);
        assert_eq!((tx, ty), (42.0, -17.0));
    }
}
