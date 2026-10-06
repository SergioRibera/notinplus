//! UUID newtypes shared across the CRDT-ready data model.
//!
//! `StrokeId` and `BookmarkId` are global UUID v4s — stable across
//! merges, deeplinks, and multi-device sync. Hash/Eq derive on
//! `[u8; 16]` so they slot into `HashMap` keys with no custom impls.
//! `#[istmo::message]` prepends the bincode derives.

use uuid::{Bytes, Uuid};

/// Globally unique stroke identifier. Replaces the previous `u32`
/// monotonic counter — a merge of two devices never collides.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug)]
pub struct StrokeId(pub Bytes);

impl StrokeId {
    #[must_use]
    pub fn new_v4() -> Self {
        Self(*Uuid::new_v4().as_bytes())
    }

    #[must_use]
    pub const fn from_bytes(bytes: Bytes) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &Bytes {
        &self.0
    }
}

impl std::fmt::Display for StrokeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Uuid::from_bytes(self.0).fmt(f)
    }
}

/// Globally unique bookmark identifier. Unused until Phase 1 lands
/// the bookmark model — declared here so Phase 0 fixes the wire shape.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug)]
pub struct BookmarkId(pub Bytes);

impl BookmarkId {
    #[must_use]
    pub fn new_v4() -> Self {
        Self(*Uuid::new_v4().as_bytes())
    }

    #[must_use]
    pub const fn from_bytes(bytes: Bytes) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &Bytes {
        &self.0
    }
}

impl std::fmt::Display for BookmarkId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Uuid::from_bytes(self.0).fmt(f)
    }
}
