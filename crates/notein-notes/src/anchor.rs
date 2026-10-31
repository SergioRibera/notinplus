//! Payload of a reference into a note.

use bincode::{Decode, Encode};

/// Where a reference into a note points: the note itself, or one of its
/// headings by its [`freya_markdown_editor::document::Document::outline`]
/// slug.
#[derive(Clone, Debug, Default, PartialEq, Eq, Encode, Decode)]
pub struct NoteAnchor {
    pub heading: Option<String>,
}

impl NoteAnchor {
    #[must_use]
    pub fn heading(slug: impl Into<String>) -> Self {
        Self {
            heading: Some(slug.into()),
        }
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        notein_sdk::Payload::to_bytes(self)
    }

    /// `None` for bytes that are not a note anchor.
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("notes::anchor_decode"))]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        bincode::decode_from_slice(bytes, bincode::config::standard())
            .ok()
            .map(|(anchor, _)| anchor)
    }
}
