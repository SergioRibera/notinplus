//! Screen-space rectangles the pen backend must treat as UI, not canvas.
//!
//! Publishing side is every overlay component that shows on top of
//! the drawing surface (palette, zoom HUD, layers panel, active
//! popup); consuming side is [`crate::canvas::Board::begin_screen`],
//! which returns early when a pen-down sample lands inside any
//! published rect.
//!
//! Design notes:
//!
//! - **Gate at `begin`, not `extend`.** Once a stroke starts outside
//!   every mask rect, subsequent samples belong to it even if the
//!   pen sweeps over an overlay. The alternative would interrupt a
//!   live gesture, which is worse UX than the current behaviour of
//!   letting the finished stroke land where the user aimed.
//! - **Fixed slot array over a `HashMap`.** Handful of overlays, each
//!   with a stable identity — indexing an array is cheaper than
//!   hashing and avoids reallocation on layout changes.
//! - **`RwLock` with poison-transparent recovery.** Reads happen on
//!   the pen thread at gesture start; writes happen on the UI thread
//!   during layout. Reader-heavy access + zero coordination between
//!   writers (each owns its slot) means contention is essentially
//!   never observed.
//! - **Coordinates are surface-logical pixels.** Same space
//!   [`freya::prelude::SizedEventData::area`] delivers and the pen
//!   backend emits samples in, so no viewport projection is needed
//!   for the containment check.

use std::sync::{OnceLock, RwLock};

/// Which overlay owns a given slot in the mask. Ordering here is only
/// used as the array index; new overlays append.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiRegion {
    Palette,
    Popup,
    Zoom,
    Layers,
    Back,
}

impl UiRegion {
    const fn slot(self) -> usize {
        self as usize
    }
}

const N_REGIONS: usize = 5;

/// Axis-aligned rectangle in surface-logical pixels: `(min_x, min_y,
/// max_x, max_y)`. Kept as a tuple so [`crate::canvas::Board`] does
/// not have to depend on `freya`'s geometry types just to read the
/// mask.
pub type Rect = (f32, f32, f32, f32);

struct MaskInner {
    slots: RwLock<[Option<Rect>; N_REGIONS]>,
}

static MASK: OnceLock<MaskInner> = OnceLock::new();

fn mask() -> &'static MaskInner {
    MASK.get_or_init(|| MaskInner {
        slots: RwLock::new([None; N_REGIONS]),
    })
}

/// Replace the rect associated with `region`. `None` clears the slot
/// — call this when an overlay hides so the mask does not linger.
pub fn set(region: UiRegion, rect: Option<Rect>) {
    if let Ok(mut g) = mask().slots.write() {
        g[region.slot()] = rect;
    }
}

/// Does any registered overlay rect contain the surface-pixel point
/// `(x, y)`? Poisoned lock returns `false` — better to accidentally
/// draw one stroke on the canvas than to silently drop pen input
/// forever.
#[must_use]
pub fn contains(x: f32, y: f32) -> bool {
    mask().slots.read().is_ok_and(|g| {
        g.iter().flatten().any(|(min_x, min_y, max_x, max_y)| {
            x >= *min_x && x <= *max_x && y >= *min_y && y <= *max_y
        })
    })
}
