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
//! - **`RwLock` with poison-transparent recovery.** Reads happen on
//!   the pen thread at gesture start; writes happen on the UI thread
//!   during layout. Reader-heavy access + zero coordination between
//!   writers (each owns its slot) means contention is essentially
//!   never observed.
//! - **Coordinates are surface-logical pixels.** Same space
//!   [`freya::prelude::SizedEventData::area`] delivers and the pen
//!   backend emits samples in, so no viewport projection is needed
//!   for the containment check.
//! - **Slots per overlay instance.** Every canvas pane has its own
//!   palette, zoom HUD, toolbars, …; with split panes two of them are on
//!   screen at once, so each instance owns a [`MaskSlot`] (see
//!   [`use_mask_slot`]) instead of sharing one slot per kind of overlay.
//!   [`UiRegion`] stays for the app-wide singletons.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

use freya::prelude::{use_drop, use_hook};
use torin::prelude::Area;

/// App-wide overlays that exist once, whatever the pane layout.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiRegion {
    Popup,
    Modal,
}

/// Mask entry owned by one overlay instance. Copy it into handlers;
/// [`use_mask_slot`] clears it when the owner unmounts.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MaskSlot(u64);

/// First id handed to [`MaskSlot`]s; ids below belong to [`UiRegion`]s.
const FIRST_SLOT: u64 = 1024;
static NEXT_SLOT: AtomicU64 = AtomicU64::new(FIRST_SLOT);

impl MaskSlot {
    fn next() -> Self {
        Self(NEXT_SLOT.fetch_add(1, Ordering::Relaxed))
    }

    /// Mask `area` (window coordinates). An empty area clears the slot.
    #[sismo::function("ui_mask::slot_publish")]
    pub fn publish(self, area: Area) {
        write(self.0, rect_of(area));
    }

    pub fn clear(self) {
        write(self.0, None);
    }
}

impl From<UiRegion> for MaskSlot {
    fn from(region: UiRegion) -> Self {
        Self(region as u64)
    }
}

/// Allocate a slot for the calling component, cleared when it unmounts.
pub fn use_mask_slot() -> MaskSlot {
    let slot = use_hook(MaskSlot::next);
    use_drop(move || {
        sismo::scope!("ui_mask::slot_drop");
        slot.clear();
    });
    slot
}

/// Axis-aligned rectangle in surface-logical pixels: `(min_x, min_y,
/// max_x, max_y)`. Kept as a tuple so [`crate::canvas::Board`] does
/// not have to depend on `freya`'s geometry types just to read the
/// mask.
pub type Rect = (f32, f32, f32, f32);

static MASK: OnceLock<RwLock<HashMap<u64, Rect>>> = OnceLock::new();

fn mask() -> &'static RwLock<HashMap<u64, Rect>> {
    MASK.get_or_init(|| RwLock::new(HashMap::new()))
}

/// `area` as a mask rect, `None` when it collapses to zero — a
/// still-laying-out element publishing an empty rect would leave a
/// phantom hole in the mask.
fn rect_of(area: Area) -> Option<Rect> {
    (area.width() > 0.0 && area.height() > 0.0)
        .then(|| (area.min_x(), area.min_y(), area.max_x(), area.max_y()))
}

#[sismo::function("ui_mask::write")]
fn write(key: u64, rect: Option<Rect>) {
    if let Ok(mut slots) = mask().write() {
        match rect {
            Some(rect) => slots.insert(key, rect),
            None => slots.remove(&key),
        };
    }
}

/// Mask `area` for a singleton overlay. An empty area clears it.
#[sismo::function("ui_mask::publish")]
pub fn publish_mask(region: UiRegion, area: Area) {
    MaskSlot::from(region).publish(area);
}

/// Replace the rect associated with `region`. `None` clears the slot
/// — call this when an overlay hides so the mask does not linger.
#[sismo::function("ui_mask::set")]
pub fn set(region: UiRegion, rect: Option<Rect>) {
    write(MaskSlot::from(region).0, rect);
}

/// Does any registered overlay rect contain the surface-pixel point
/// `(x, y)`? Poisoned lock returns `false` — better to accidentally
/// draw one stroke on the canvas than to silently drop pen input
/// forever.
#[must_use]
#[sismo::function("ui_mask::contains")]
pub fn contains(x: f32, y: f32) -> bool {
    mask().read().is_ok_and(|slots| {
        slots.values().any(|(min_x, min_y, max_x, max_y)| {
            x >= *min_x && x <= *max_x && y >= *min_y && y <= *max_y
        })
    })
}
