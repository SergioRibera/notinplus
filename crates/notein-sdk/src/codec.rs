//! Bincode codec for the opaque `Vec<u8>` payloads that cross the
//! plugin trait boundary.
//!
//! The shell and plugin must agree exactly on which encoder config
//! they use. This module pins
//! [`bincode::config::standard()`] on both sides and exposes short
//! helpers so neither side has to repeat the choice at every call
//! site.

use bincode::config::{Configuration, standard};
use bincode::error::{DecodeError, EncodeError};
use bincode::{Decode, Encode};

use notein_plugin::{Bookmark, SourceRef};

/// Shared bincode config. Fixed once here so the shell and plugins
/// never drift.
#[must_use]
pub const fn config() -> Configuration {
    standard()
}

/// Encode any bincode-derivable value with the shared config.
pub fn encode<T: Encode>(value: &T) -> Result<Vec<u8>, EncodeError> {
    bincode::encode_to_vec(value, config())
}

/// Decode any bincode-derivable value from the start of `bytes`.
///
/// Returns the decoded value, dropping any trailing bytes.
pub fn decode<T: Decode<()>>(bytes: &[u8]) -> Result<T, DecodeError> {
    bincode::decode_from_slice(bytes, config()).map(|(value, _)| value)
}

/// Encode a bookmark payload for the trait boundary (e.g.
/// `update_bookmark`).
pub fn encode_bookmark(bm: &Bookmark) -> Result<Vec<u8>, EncodeError> {
    encode(bm)
}

/// Decode a bookmark payload received on the trait boundary.
pub fn decode_bookmark(bytes: &[u8]) -> Result<Bookmark, DecodeError> {
    decode(bytes)
}

/// Encode the list payload returned from `list_bookmarks`.
pub fn encode_bookmark_list(bms: &[Bookmark]) -> Result<Vec<u8>, EncodeError> {
    encode(&bms.to_vec())
}

/// Decode the list payload returned from `list_bookmarks`.
pub fn decode_bookmark_list(bytes: &[u8]) -> Result<Vec<Bookmark>, DecodeError> {
    decode(bytes)
}

/// Encode a `SourceRef` chip body (round-trips the inline `[[…]]`
/// targets authors want to send across the wire).
pub fn encode_source_ref(r: &SourceRef) -> Result<Vec<u8>, EncodeError> {
    encode(r)
}

/// Decode a `SourceRef` body.
pub fn decode_source_ref(bytes: &[u8]) -> Result<SourceRef, DecodeError> {
    decode(bytes)
}
