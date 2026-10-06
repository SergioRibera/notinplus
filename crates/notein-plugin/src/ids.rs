//! Shared identifier types referenced by the trait surface.
//!
//! Bookmark ids are UUID v4 (16-byte) so they survive CRDT merges and
//! cross-device deeplinks without collision. Positions in the host's
//! canvas world are `(f32, f32)`.
//!
//! The host crate carries its own `BookmarkId` under `src/ids.rs`
//! today; these types mirror the shape so plugins have a wire-level
//! contract that does not depend on the host crate.

use bincode::{Decode, Encode};

/// Globally unique bookmark id.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Debug, Encode, Decode)]
pub struct BookmarkId(pub [u8; 16]);

impl BookmarkId {
    /// Fresh UUID v4.
    #[must_use]
    pub fn new_v4() -> Self {
        Self(*uuid::Uuid::new_v4().as_bytes())
    }

    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl std::fmt::Display for BookmarkId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        uuid::Uuid::from_bytes(self.0).fmt(f)
    }
}

/// Point in a canvas's world coordinate system. Z-free, origin top-left.
#[derive(Clone, Copy, PartialEq, Debug, Default, Encode, Decode)]
pub struct WorldPoint {
    pub x: f32,
    pub y: f32,
}

impl WorldPoint {
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}
