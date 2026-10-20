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
pub mod replica;
pub mod share;
#[cfg(test)]
mod testing;

pub use cipher::ShareKey;
pub use driver::{Action, DriverEvent, JoinFailure, PairFailure, PairingOffer, SyncDriver};
pub use engine::{DocStatus, Handled, Message, PROTOCOL_VERSION, SyncEngine};
pub use error::{CipherError, IdentityError, SyncError};
pub use identity::{DeviceId, DeviceKey, DeviceProfile, PairingCode, TrustedDevice};
pub use net::lan::LanNode;
pub use net::{Beacon, LanConfig, NetError, NetEvent, PeerInfo};
pub use replica::{Blob, BlobHead, BlobKey, BlobKind, DocId, Replica, Scope, Version};
pub use share::{
    Invite, Member, Share, ShareBook, ShareId, ShareMessage, ShareMode, SharePolicy, Who,
};
