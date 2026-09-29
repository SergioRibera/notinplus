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

/// Subscribe to platform safe-area updates. The returned state resolves
/// to the zero `EdgeInsets` until the first frame arrives (or forever
/// on desktop, where no publisher exists).
pub fn use_safe_area_insets() -> State<EdgeInsets> {
    let mut insets = use_state(EdgeInsets::default);
    use_hook(move || {
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
/// IME only pushes bottom padding — top/left/right ignore it so
/// floating overlays don't jump when the keyboard opens.
pub const fn fold_insets(insets: SafeAreaInsets) -> EdgeInsets {
    let base = insets.system_bars.max(insets.display_cutout);
    EdgeInsets {
        top: base.top,
        right: base.right,
        bottom: base.bottom.max(insets.ime.bottom),
        left: base.left,
    }
}
