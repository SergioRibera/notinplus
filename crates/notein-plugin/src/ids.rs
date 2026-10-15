//! Shared identifier types for the plugin surface.
//!
//! Bookmark + stroke ids are UUID v4 (16-byte) so they survive CRDT
//! merges and cross-device deeplinks without collision. Hash / Eq
//! derive straight off the byte array so both ids slot into
//! `HashMap` keys with no custom impls.
//!
//! Device + user identifiers combine into [`ActorId`]; the Lamport
//! stamp + actor combine into [`OpId`], the globally orderable id
//! every CRDT op carries. Keeping these on the plugin crate lets the
//! trait signatures reference them without the host having to lower
//! them to opaque bytes at the FFI boundary.

use bincode::{Decode, Encode};
use rkyv::{Archive, Deserialize, Serialize};

/// Globally unique stroke identifier.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Debug,
    Encode,
    Decode,
    Archive,
    Serialize,
    Deserialize,
)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct StrokeId(pub [u8; 16]);

impl StrokeId {
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

impl std::fmt::Display for StrokeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        uuid::Uuid::from_bytes(self.0).fmt(f)
    }
}

/// Globally unique bookmark identifier.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Debug,
    Encode,
    Decode,
    Archive,
    Serialize,
    Deserialize,
)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct BookmarkId(pub [u8; 16]);

impl BookmarkId {
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

/// Per-install device identifier. The host persists a single value per
/// filesystem install and stamps every emitted op with it.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Debug,
    Encode,
    Decode,
    Archive,
    Serialize,
    Deserialize,
)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct DeviceId(pub [u8; 16]);

impl DeviceId {
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

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        uuid::Uuid::from_bytes(self.0).fmt(f)
    }
}

/// Per-user identifier. One `UserId` fans out across every device a
/// user signs in on.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Debug,
    Encode,
    Decode,
    Archive,
    Serialize,
    Deserialize,
)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct UserId(pub [u8; 16]);

impl UserId {
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

impl std::fmt::Display for UserId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        uuid::Uuid::from_bytes(self.0).fmt(f)
    }
}

/// Pair of user + device. Travels on every emitted op so the
/// Lamport tiebreak stays total across a multi-device, multi-user
/// mesh.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Debug,
    Encode,
    Decode,
    Archive,
    Serialize,
    Deserialize,
)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct ActorId {
    pub user: UserId,
    pub device: DeviceId,
}

/// Globally orderable op identifier. `(lamport, actor)` lexicographic
/// comparison is total and deterministic — concurrent writes converge
/// on the same winner without external coordination.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Default,
    Debug,
    Encode,
    Decode,
    Archive,
    Serialize,
    Deserialize,
)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct OpId {
    pub lamport: u64,
    pub actor: ActorId,
}

impl Ord for OpId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.lamport
            .cmp(&other.lamport)
            .then_with(|| self.actor.cmp(&other.actor))
    }
}

impl PartialOrd for OpId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
