//! Bincode codec with a fixed config shared between the shell and
//! every plugin.
//!
//! Not used on the trait boundary itself — shared types travel typed
//! (see `notein_plugin::CanvasKind::list_bookmarks` etc). These
//! helpers are for:
//!
//! - **Persistence** — plugins that store their own state on disk.
//! - **Bundle export** — `.notein` packaging in Phase 9.
//! - **Phase 3 FFI adapter** — the dylib wrapper serializes typed
//!   values here just before the stabby boundary.
//! - **Phase 10 CRDT op shipping** — `apply_remote_op` payloads.
//!
//! The config is pinned to [`bincode::config::standard()`] once so
//! neither side has to repeat the choice at every call site.

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
