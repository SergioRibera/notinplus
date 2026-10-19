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
//!
//! Transports (LAN, relay) feed [`engine::Message`] bytes in and out;
//! nothing here opens a socket.

pub mod cipher;
pub mod engine;
pub mod error;
pub mod identity;
pub mod replica;

pub use cipher::ShareKey;
pub use engine::{DocStatus, Message, PROTOCOL_VERSION, SyncEngine};
pub use error::{CipherError, IdentityError, SyncError};
pub use identity::{DeviceId, DeviceKey, DeviceProfile, PairingCode, TrustedDevice};
pub use replica::{Blob, BlobHead, BlobKey, BlobKind, DocId, Replica, Scope, Version};
