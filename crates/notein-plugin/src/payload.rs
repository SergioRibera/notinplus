//! Typed payloads that the shell stores and ships as bytes.
//!
//! A plugin's anchors and document content are ordinary Rust types.
//! The shell never looks inside them: it keeps their bytes, and the
//! adapters in [`crate::erased`] encode and decode them with one codec
//! on both sides.

use bincode::config::{self, Configuration};
use bincode::{Decode, Encode};

const CODEC: Configuration = config::standard();

/// A value that crosses the shell as bytes.
pub trait Payload: Encode + Decode<()> + Clone + Send + Sync + 'static {
    /// Encode with the plugin codec.
    #[must_use]
    fn to_bytes(&self) -> Vec<u8> {
        bincode::encode_to_vec(self, CODEC).unwrap_or_default()
    }

    /// Decode bytes written by [`Payload::to_bytes`].
    ///
    /// # Errors
    /// [`PayloadError`] when the bytes are not a valid encoding.
    fn from_bytes(bytes: &[u8]) -> Result<Self, PayloadError> {
        bincode::decode_from_slice(bytes, CODEC)
            .map(|(value, _)| value)
            .map_err(|e| PayloadError(e.to_string()))
    }
}

impl<T> Payload for T where T: Encode + Decode<()> + Clone + Send + Sync + 'static {}

/// Bytes that do not decode as the expected payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayloadError(pub String);

impl std::fmt::Display for PayloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid payload: {}", self.0)
    }
}

impl std::error::Error for PayloadError {}

#[cfg(test)]
mod tests {
    use super::Payload;

    #[derive(bincode::Encode, bincode::Decode, Clone, Debug, PartialEq)]
    struct Verse {
        book: u8,
        chapter: u16,
    }

    #[test]
    fn round_trip() {
        let verse = Verse {
            book: 45,
            chapter: 8,
        };
        assert_eq!(Verse::from_bytes(&verse.to_bytes()), Ok(verse));
        assert!(Verse::from_bytes(&[0xFF; 1]).is_err());
    }
}
