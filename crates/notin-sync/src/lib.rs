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

/// Decode `bytes` that came from elsewhere.
///
/// bincode allocates a length a value claims before reading it, up to
/// the configured limit, so a few forged bytes could make it allocate
/// the whole limit. Here the limit follows the input: 64 times its size
/// (at least 64 KiB), and never more than `cap`, what a value of this
/// kind may ever take. A longer claim is an error, never an allocation.
pub(crate) fn decode<T: bincode::Decode<()>>(
    bytes: &[u8],
    cap: usize,
) -> Result<T, bincode::error::DecodeError> {
    let need = bytes.len().saturating_mul(64).max(1 << 16).min(cap);
    macro_rules! within {
        ($($limit:expr),*) => {
            $(
                if need <= $limit {
                    let config = bincode::config::standard().with_limit::<{ $limit }>();
                    return bincode::decode_from_slice(bytes, config).map(|(value, _)| value);
                }
            )*
        };
    }
    within!(
        1 << 12,
        1 << 16,
        1 << 18,
        1 << 20,
        1 << 22,
        1 << 24,
        1 << 26,
        1 << 28,
        1 << 29
    );
    let config = bincode::config::standard().with_limit::<{ 1 << 30 }>();
    bincode::decode_from_slice(bytes, config).map(|(value, _)| value)
}

/// What a value of each kind may take once decoded, for [`decode`].
pub(crate) mod cap {
    /// A device's beacon: id, key, name.
    pub(crate) const BEACON: usize = 4 << 10;
    /// One transport frame: at most a part of a payload.
    pub(crate) const FRAME: usize = 2 << 20;
    /// Between a device and its relay: one page of records at most.
    pub(crate) const RELAY: usize = 32 << 20;
    /// A share's keys, for one member.
    pub(crate) const KEYS: usize = 64 << 10;
    /// An owner's answer to a joiner: the share's view.
    pub(crate) const ANSWER: usize = 16 << 20;
    /// One mailbox record as its author signed it.
    pub(crate) const RECORD: usize = 32 << 20;
    /// A whole payload, reassembled from parts (256 MiB) with framing,
    /// and what it carries.
    pub(crate) const PAYLOAD: usize = 512 << 20;
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_claimed_length_is_held_to_the_input_and_the_kind() {
        // Five bytes claiming 256 MiB: refused, not allocated.
        let forged = [252, 0, 0, 0, 0x10];
        assert!(super::decode::<Vec<u8>>(&forged, super::cap::PAYLOAD).is_err());
        // What was really sent decodes, big or small.
        for len in [0, 10, 1 << 20, 3 << 20] {
            let bytes =
                bincode::encode_to_vec(vec![7u8; len], bincode::config::standard()).unwrap();
            let back: Vec<u8> = super::decode(&bytes, super::cap::PAYLOAD).unwrap();
            assert_eq!(back.len(), len);
        }
        // Past what the kind may take: refused however much was sent.
        let name =
            bincode::encode_to_vec("x".repeat(8 << 10), bincode::config::standard()).unwrap();
        assert!(super::decode::<String>(&name, super::cap::BEACON).is_err());
    }
}
