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

/// Decode `bytes` that came from elsewhere, as a value that may never
/// take more than `cap` bytes (see [`istmo_core::codec::decode_capped`]):
/// a claimed length follows the input, never the cap alone.
pub(crate) fn decode<T: istmo_core::codec::Decode>(
    bytes: &[u8],
    cap: usize,
) -> Result<T, istmo_core::CodecError> {
    istmo_core::codec::decode_capped(bytes, cap).map(|(value, _)| value)
}

/// Feeds `bytes` to every decoder that reads bytes from elsewhere, as
/// plain text (what is left once a seal opened), for `fuzz/`.
#[cfg(fuzzing)]
#[doc(hidden)]
pub fn fuzz_decode(bytes: &[u8]) {
    let _ = decode::<net::Frame>(bytes, cap::FRAME);
    let _ = decode::<net::Beacon>(bytes, cap::BEACON);
    let _ = relay::RelayMessage::from_bytes(bytes);
    let _ = decode::<driver::Payload>(bytes, cap::PAYLOAD);
    let _ = decode::<driver::JoinAnswer>(bytes, cap::ANSWER);
    let _ = engine::Message::from_bytes::<()>(bytes);
    let _ = share::ShareBook::decode(bytes);
    let _ = decode::<share::ShareMessage>(bytes, cap::RECORD);
    let _ = decode::<share::SignedRecord>(bytes, cap::RECORD);
    let _ = decode::<Vec<cipher::ShareKey>>(bytes, cap::KEYS);
    let _ = identity::DeviceProfile::decode(bytes);
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
            let bytes = istmo_core::codec::encode(&vec![7u8; len]).unwrap();
            let back: Vec<u8> = super::decode(&bytes, super::cap::PAYLOAD).unwrap();
            assert_eq!(back.len(), len);
        }
        // Past what the kind may take: refused however much was sent.
        let name = istmo_core::codec::encode(&"x".repeat(8 << 10)).unwrap();
        assert!(super::decode::<String>(&name, super::cap::BEACON).is_err());
        // Nor a little past it, where the limits used to round up.
        let frame = istmo_core::codec::encode(&vec![7u8; 3 << 20]).unwrap();
        assert!(super::decode::<Vec<u8>>(&frame, super::cap::FRAME).is_err());
        let within = istmo_core::codec::encode(&vec![7u8; 1 << 20]).unwrap();
        assert!(super::decode::<Vec<u8>>(&within, super::cap::FRAME).is_ok());
    }
}
