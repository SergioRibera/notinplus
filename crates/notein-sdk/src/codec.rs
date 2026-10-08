//! rkyv codec shared between the shell and every plugin.
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
//! Decoding is **validated** via rkyv's `bytecheck` layer so untrusted
//! bytes (bundle imports, remote sync frames) cannot UB.

use notein_plugin::{Bookmark, SourceRef, rkyv};

use rkyv::{from_bytes, rancor::Error, to_bytes};

/// Encode a bookmark payload for the trait boundary (e.g.
/// `update_bookmark`).
pub fn encode_bookmark(bm: &Bookmark) -> Result<Vec<u8>, Error> {
    to_bytes::<Error>(bm).map(|v| v.to_vec())
}

/// Decode a bookmark payload received on the trait boundary.
pub fn decode_bookmark(bytes: &[u8]) -> Result<Bookmark, Error> {
    from_bytes::<Bookmark, Error>(bytes)
}

/// Encode the list payload returned from `list_bookmarks`.
pub fn encode_bookmark_list(bms: &[Bookmark]) -> Result<Vec<u8>, Error> {
    to_bytes::<Error>(&bms.to_vec()).map(|v| v.to_vec())
}

/// Decode the list payload returned from `list_bookmarks`.
pub fn decode_bookmark_list(bytes: &[u8]) -> Result<Vec<Bookmark>, Error> {
    from_bytes::<Vec<Bookmark>, Error>(bytes)
}

/// Encode a `SourceRef` chip body (round-trips the inline `[[…]]`
/// targets authors want to send across the wire).
pub fn encode_source_ref(r: &SourceRef) -> Result<Vec<u8>, Error> {
    to_bytes::<Error>(r).map(|v| v.to_vec())
}

/// Decode a `SourceRef` body.
pub fn decode_source_ref(bytes: &[u8]) -> Result<SourceRef, Error> {
    from_bytes::<SourceRef, Error>(bytes)
}
