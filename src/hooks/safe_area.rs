//! Freya hooks that bridge the platform [`SafeArea`] early-event stream
//! into reactive [`EdgeInsets`] in layout units.
//!
//! The window subscribes once ([`provide_safe_area_root`]) and every
//! descendant reads the same value ([`use_safe_area_insets`]).
//!
//! The stream lives on an OS helper thread outside freya's executor;
//! `State` is `!Send`, so we cannot touch it from the recv thread.
//! The root hook spawns a `flume` bridge whose async drain runs inside
//! freya's executor where `insets.set(..)` is safe.
//!
//! Platforms publish insets in density-independent pixels, while freya
//! lays out in those pixels divided by the app's own text scale
//! (`Platform::custom_scale_factor`), so the insets are divided by it
//! before anyone pads with them.
//!
//! Non-mobile builds (desktop) still get a working handle — `acquire`
//! fails, the hook logs at debug level, and the insets stay at zero so
//! callers can add the padding unconditionally.

use freya::prelude::*;
use istmo::plugins::{EdgeInsets, SafeArea, SafeAreaInsets};

/// Context override: when a parent provides this, descendants' calls to
/// [`use_safe_area_insets`] see the overridden value instead of the live
/// platform stream.
///
/// Routes that have already carved out the physical safe area at an outer frame
/// install `SafeAreaOverride(EdgeInsets::default())` so nested components do
/// not re-apply the same padding and collapse the usable area twice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SafeAreaOverride(pub EdgeInsets);

/// Install an override for every descendant that calls
/// [`use_safe_area_insets`]. Idempotent across re-renders — provider
/// value only matters on first mount of the owning scope.
#[sismo::function("hooks::provide_safe_area_override")]
pub fn provide_safe_area_override(insets: EdgeInsets) {
    use_provide_context(move || SafeAreaOverride(insets));
}

/// The window's live insets, shared from the app root.
#[derive(Clone, Copy, PartialEq)]
struct SafeAreaRoot(Memo<EdgeInsets>);

/// Subscribe to the platform insets once for the whole window and share
/// them with every descendant. Call at the app root, before anything
/// that pads with the insets.
#[sismo::function("hooks::provide_safe_area_root")]
pub fn provide_safe_area_root() -> Memo<EdgeInsets> {
    let mut published = use_state(EdgeInsets::default);
    use_hook(move || {
        let sa = match SafeArea::acquire() {
            Ok(sa) => sa,
            Err(err) => {
                log::debug!("safe_area not ready: {err:?}");
                return;
            }
        };
        published.set(fold_insets(sa.current_or_zero()));
        let (tx, rx) = flume::unbounded::<EdgeInsets>();
        let stream = sa.stream();
        std::thread::spawn(move || {
            while let Ok(next) = stream.recv() {
                if tx.send(fold_insets(next)).is_err() {
                    break;
                }
            }
        });
        spawn(traced!("task::hooks::safe_area", async move {
            while let Ok(next) = rx.recv_async().await {
                sismo::instant!("hooks::insets_changed");
                published.set(next);
            }
        }));
    });
    let insets = use_memo(move || {
        let scale = *Platform::get().current_window().custom_scale_factor.read();
        in_layout_units(*published.read(), scale)
    });
    use_provide_context(move || SafeAreaRoot(insets));
    insets
}

/// The insets to pad with, in layout units. Zero until the platform
/// publishes its first frame, forever on desktop, and outside an app
/// root (previews, tests).
///
/// When an ancestor installed a [`SafeAreaOverride`] via
/// [`provide_safe_area_override`] the override is returned instead: it
/// is authoritative for the whole subtree.
#[must_use]
pub fn use_safe_area_insets() -> Memo<EdgeInsets> {
    let override_val = use_try_consume::<SafeAreaOverride>().map(|o| o.0);
    let root = use_try_consume::<SafeAreaRoot>();
    let fixed = use_memo(move || override_val.unwrap_or_default());
    match (override_val, root) {
        (None, Some(SafeAreaRoot(live))) => live,
        _ => fixed,
    }
}

/// Fold platform-published [`SafeAreaInsets`] (system bars + display
/// cutout + IME) into a single set of edge padding the app applies.
///
/// IME only pushes bottom padding — top/left/right ignore it so
/// floating overlays don't jump when the keyboard opens.
#[must_use]
pub const fn fold_insets(insets: SafeAreaInsets) -> EdgeInsets {
    let base = insets.system_bars.max(insets.display_cutout);
    EdgeInsets {
        top: base.top,
        right: base.right,
        bottom: base.bottom.max(insets.ime.bottom),
        left: base.left,
    }
}

/// Density-independent insets as layout units under the app's text
/// `scale` (`Platform::custom_scale_factor`).
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    reason = "the text scale is a small factor around 1"
)]
fn in_layout_units(insets: EdgeInsets, scale: f64) -> EdgeInsets {
    let scale = if scale > 0.0 { scale as f32 } else { 1.0 };
    EdgeInsets {
        top: insets.top / scale,
        right: insets.right / scale,
        bottom: insets.bottom / scale,
        left: insets.left / scale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insets_shrink_with_a_larger_text_scale() {
        let dp = EdgeInsets {
            top: 24.0,
            right: 0.0,
            bottom: 300.0,
            left: 0.0,
        };
        let at = in_layout_units(dp, 1.5);
        assert!((at.top - 16.0).abs() < 1e-4);
        assert!((at.bottom - 200.0).abs() < 1e-4);
        assert_eq!(in_layout_units(dp, 1.0), dp);
    }

    #[test]
    fn the_keyboard_only_pushes_the_bottom() {
        let bar = EdgeInsets {
            top: 24.0,
            right: 0.0,
            bottom: 48.0,
            left: 0.0,
        };
        let ime = EdgeInsets {
            top: 0.0,
            right: 0.0,
            bottom: 320.0,
            left: 0.0,
        };
        let folded = fold_insets(SafeAreaInsets {
            system_bars: bar,
            ime,
            display_cutout: EdgeInsets::default(),
        });
        assert!((folded.top - 24.0).abs() < 1e-4);
        assert!((folded.bottom - 320.0).abs() < 1e-4);
    }
}
