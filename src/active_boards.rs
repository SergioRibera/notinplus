//! Lookup table for the live [`Board`] of each mounted canvas face.
//!
//! Siblings of a `canvas_root` (chip strips, cross-pane callbacks,
//! bookmark toasts) need to reach the mount's board without the Freya
//! parent-child context plumbing. The canvas registers on mount and
//! clears on drop; readers get `None` when nothing is mounted.
//!
//! Keyed on the full [`CanvasMount`] rather than the raw [`ItemId`]
//! so an ephemeral surface never collides with a persisted item. [`get_item`] exists for callers that only know
//! the id and want "any" face currently mounted for it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::app::CanvasMount;
use crate::canvas::Board;
use crate::library::ItemId;

type Registry = Mutex<HashMap<CanvasMount, Arc<Mutex<Board>>>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<CanvasMount, Arc<Mutex<Board>>>> {
    match registry().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Publish `board` as the active mount for `mount`. A second mount for
/// the same key wins — matches Freya's "latest render keeps the
/// handle" semantics.
pub(crate) fn set(mount: CanvasMount, board: Arc<Mutex<Board>>) {
    lock().insert(mount, board);
}

/// Drop the registration if `board` still matches what was stored.
/// Pointer-equality guard so a stale `use_drop` cannot evict a newer
/// mount's handle during a route transition that remounts.
pub(crate) fn clear_if(mount: CanvasMount, board: &Arc<Mutex<Board>>) {
    let mut g = lock();
    if let Some(current) = g.get(&mount) {
        if Arc::ptr_eq(current, board) {
            g.remove(&mount);
        }
    }
}

/// Look up the currently mounted board for `mount`.
#[must_use]
pub(crate) fn get(mount: CanvasMount) -> Option<Arc<Mutex<Board>>> {
    lock().get(&mount).map(Arc::clone)
}

/// Fallback lookup for callers that only know the item id and are
/// happy with "any" face currently mounted (first match wins). Prefer
/// [`get`] when the exact mount flavour is known.
#[must_use]
pub fn get_item(item: ItemId) -> Option<Arc<Mutex<Board>>> {
    lock()
        .iter()
        .find(|(k, _)| k.item() == Some(item))
        .map(|(_, b)| Arc::clone(b))
}

/// Every board currently showing `item`, in no particular order. Two
/// panes of the same document each have one.
#[must_use]
pub(crate) fn for_item(item: ItemId) -> Vec<(CanvasMount, Arc<Mutex<Board>>)> {
    lock()
        .iter()
        .filter(|(k, _)| k.item() == Some(item))
        .map(|(k, v)| (*k, Arc::clone(v)))
        .collect()
}

/// Every board currently mounted.
#[must_use]
pub(crate) fn all() -> Vec<Arc<Mutex<Board>>> {
    lock().values().map(Arc::clone).collect()
}
