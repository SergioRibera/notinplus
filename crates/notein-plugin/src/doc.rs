//! Documents as plugins see them: an id and the shell's metadata.

use bincode::{Decode, Encode};

/// Library id of a document (16-byte UUID).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Encode, Decode)]
pub struct DocId(pub [u8; 16]);

impl DocId {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl From<[u8; 16]> for DocId {
    fn from(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

/// What the shell knows about a document, lent to plugin calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DocMeta<'a> {
    pub id: DocId,
    pub name: &'a str,
    /// Unix millis of the last change.
    pub updated_at: u64,
}
