//! Keeps notinplus libraries in step: between a user's own devices and
//! between people who share documents.
//!
//! - [`identity`] — a device is its keypair; trusted devices make up
//!   "the same user". No accounts.
//! - [`cipher`] — one symmetric key per share, so a relay only ever
//!   holds ciphertext.
//! - [`replica`] — what a library exposes to be exchanged: mergeable
//!   documents plus newest-wins blobs for everything else.
//! - [`engine`] — the announce / answer exchange, transport-agnostic.
//! - [`net`] — the LAN transport: discovery, authenticated channels.
//! - [`driver`] — policy tying the two together: trust, pairing,
//!   when to announce.

pub mod cipher;
pub mod driver;
pub mod engine;
pub mod error;
pub mod identity;
pub mod net;
pub mod relay;
pub mod replica;
pub mod share;
#[cfg(test)]
mod testing;

pub use cipher::ShareKey;
pub use driver::{
    Action, DriverEvent, JoinFailure, PairFailure, PairingOffer, Path, PendingJoin, Removal,
    SyncDriver,
};
pub use engine::{DocStatus, Handled, Message, PROTOCOL_VERSION, SyncEngine};
pub use error::{CipherError, IdentityError, SyncError};
pub use identity::{DeviceId, DeviceKey, DeviceProfile, PairingCode, PairingSecret, TrustedDevice};
pub use net::lan::LanNode;
pub use net::{Beacon, LanConfig, Lane, NetError, NetEvent, PeerInfo};
pub use relay::{
    KnockToken, RelayAddr, RelayConfig, RelayLimits, RelayMessage, RelayObserver, RelayServer,
    Store as RelayStore,
};
pub use replica::{Blob, BlobHead, BlobKey, BlobKind, DocId, Replica, Scope, Version};
pub use share::{
    EndedShare, Invite, LeftShare, Member, Role, Share, ShareBook, ShareEdit, ShareId,
    ShareMessage, ShareMode,
};

/// How bytes from elsewhere are decoded: a length past this limit is
/// refused before anything is allocated, so a forged one can neither
/// exhaust memory nor abort the process. Room for the largest payload a
/// channel carries in parts (256 MiB) and its framing; the relay hands
/// its mailboxes over in pages far below it.
pub(crate) const WIRE: bincode::config::Configuration<
    bincode::config::LittleEndian,
    bincode::config::Varint,
    bincode::config::Limit<{ 1 << 29 }>,
> = bincode::config::standard().with_limit::<{ 1 << 29 }>();
