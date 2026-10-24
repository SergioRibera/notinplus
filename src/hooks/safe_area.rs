//! Freya hook that bridges the platform [`SafeArea`] early-event stream
//! into a reactive [`State<EdgeInsets>`].
//!
//! The stream lives on an OS helper thread outside freya's executor;
//! `State` is `!Send`, so we cannot touch it from the recv thread.
//! The hook spawns a `flume` bridge whose async drain runs inside
//! freya's executor where `insets.set(..)` is safe.
//!
//! Non-mobile builds (desktop) still get a working handle — `acquire`
//! fails, the hook logs at debug level, and the state stays at the
//! zero `EdgeInsets` default so callers can add the padding
//! unconditionally.

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
pub fn provide_safe_area_override(insets: EdgeInsets) {
    use_provide_context(move || SafeAreaOverride(insets));
}

/// Subscribe to platform safe-area updates. The returned state resolves
/// to the zero `EdgeInsets` until the first frame arrives (or forever
/// on desktop, where no publisher exists).
///
/// When an ancestor installed a [`SafeAreaOverride`] via
/// [`provide_safe_area_override`] the state is seeded with the override
/// value and the platform subscription is skipped — the override is
/// authoritative for the whole subtree.
#[must_use]
pub fn use_safe_area_insets() -> State<EdgeInsets> {
    let override_val = use_try_consume::<SafeAreaOverride>().map(|o| o.0);
    let mut insets = use_state(move || override_val.unwrap_or_default());
    use_hook(move || {
        if override_val.is_some() {
            return;
        }
        let sa = match SafeArea::acquire() {
            Ok(sa) => sa,
            Err(err) => {
                log::debug!("safe_area not ready: {err:?}");
                return;
            }
        };
        let initial = fold_insets(sa.current_or_zero());
        insets.set(initial);
        let (tx, rx) = flume::unbounded::<EdgeInsets>();
        let stream = sa.stream();
        std::thread::spawn(move || {
            while let Ok(next) = stream.recv() {
                if tx.send(fold_insets(next)).is_err() {
                    break;
                }
            }
        });
        spawn(async move {
            while let Ok(next) = rx.recv_async().await {
                insets.set(next);
            }
        });
    });
    insets
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
