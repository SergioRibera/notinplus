//! When the tags on each item and folder last changed.
//!
//! Tags are a person's own way to sort things: they travel between that
//! person's devices on their own record, stamped here, and never inside
//! the item or folder a share carries. Kept apart from the index so its
//! stored shape stays as it was.

use std::collections::HashMap;

/// Last change of the tags on each item or folder, by its id. An id
/// with no entry never had its tags changed.
#[istmo::message]
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct TagStamps {
    at: HashMap<[u8; 16], u64>,
}

impl TagStamps {
    /// When the tags on `id` last changed; `None` when they never did.
    #[must_use]
    pub fn get(&self, id: [u8; 16]) -> Option<u64> {
        self.at.get(&id).copied()
    }

    /// The tags on `id` changed at `at`.
    pub fn set(&mut self, id: [u8; 16], at: u64) {
        self.at.insert(id, at);
    }

    /// `id` is gone; its tags with it.
    pub fn forget(&mut self, id: [u8; 16]) {
        self.at.remove(&id);
    }
}
