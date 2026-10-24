//! Plugin-private anchor payload.
//!
//! `BibleAnchor` is the shape every `SourceRef::Plugin { plugin_id =
//! "io.notein.bible", anchor }` carries. Only this crate decodes it.
//! The on-wire form is bincoded; the shell is pure transport.
//!
//! Keep the layout additive: downstream chips, deeplinks and shared
//! bundles may embed older encodings. Future variants should go in
//! `BibleAnchor::V*` enums rather than mutating `V1` fields.

use bincode::{Decode, Encode};

/// Reference to a single verse or a verse range inside a specific
/// translation.
///
/// `end_verse == start_verse` + `end_chapter == start_chapter` for a
/// single-verse pin. Cross-chapter ranges are allowed but conventionally
/// avoided — the UI collapses "Juan 3:16" and "Juan 3:16-17" the same
/// way, but a range spanning chapters pays a layout cost on scroll.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct BibleAnchor {
    /// Translation id, e.g. `"spa_rv1960"`. Resolver falls back to the
    /// user's default Bible when the exact id is missing (chip shows a
    /// `⚠` marker in that case).
    pub bible_id: String,
    /// OSIS book code, e.g. `"EPH"` for Ephesians. Fixed small set per
    /// canon so the field is cheap even when embedded in every chip.
    pub book_id: String,
    pub start_chapter: u16,
    pub start_verse: u16,
    pub end_chapter: u16,
    pub end_verse: u16,
    /// Optional character offsets inside the start/end verses. `None`
    /// means "whole verse". Used for highlight-style bookmarks that
    /// span a sub-sentence; chip rendering ignores them.
    pub start_char: Option<u32>,
    pub end_char: Option<u32>,
}

impl BibleAnchor {
    #[must_use]
    pub fn single_verse(bible_id: impl Into<String>, book_id: impl Into<String>, chapter: u16, verse: u16) -> Self {
        Self {
            bible_id: bible_id.into(),
            book_id: book_id.into(),
            start_chapter: chapter,
            start_verse: verse,
            end_chapter: chapter,
            end_verse: verse,
            start_char: None,
            end_char: None,
        }
    }

    /// Bincode-encode for storage inside `SourceRef::Plugin::anchor`.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        notein_sdk::Payload::to_bytes(self)
    }

    /// Decode a payload previously produced by [`Self::encode`]. Returns
    /// `None` on any decode error — callers surface as "broken anchor"
    /// (chip ⚠) rather than propagating a hard failure.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        bincode::decode_from_slice(bytes, bincode::config::standard())
            .ok()
            .map(|(v, _)| v)
    }
}
