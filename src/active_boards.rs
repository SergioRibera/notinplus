//! Lookup table for the live [`Board`] of each mounted item.
//!
//! Siblings of a `canvas_root` (chip strips, cross-pane callbacks,
//! bookmark toasts) need to reach the mount's board without the Freya
//! parent-child context plumbing. The canvas registers on mount and
//! clears on drop; readers get `None` when nothing is mounted.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::canvas::Board;
use crate::library::ItemId;

type Registry = Mutex<HashMap<ItemId, Arc<Mutex<Board>>>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<ItemId, Arc<Mutex<Board>>>> {
    match registry().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Publish `board` as the active mount for `item`. Called from the
/// canvas root's mount hook. A second mount for the same item wins —
/// matches Freya's "latest render keeps the handle" semantics.
pub fn set(item: ItemId, board: Arc<Mutex<Board>>) {
    lock().insert(item, board);
}

/// Drop the registration if `board` still matches what was stored.
/// Pointer-equality guard so a stale `use_drop` can't evict a newer
/// mount's handle during a route transition that remounts.
pub fn clear_if(item: ItemId, board: &Arc<Mutex<Board>>) {
    let mut g = lock();
    if let Some(current) = g.get(&item) {
        if Arc::ptr_eq(current, board) {
            g.remove(&item);
        }
    }
}

/// Look up the currently mounted board for `item`.
pub fn get(item: ItemId) -> Option<Arc<Mutex<Board>>> {
    lock().get(&item).map(Arc::clone)
}
