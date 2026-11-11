//! Typed payloads that the shell stores and ships as bytes.
//!
//! A plugin's anchors and document content are ordinary Rust types.
//! The shell never looks inside them: it keeps their bytes, and the
//! adapters in [`crate::erased`] encode and decode them with one codec
//! on both sides: istmo's wire codec ([`istmo_core::codec`]).

pub use istmo_core::CodecError;
pub use istmo_core::codec::{Decode, Encode};

/// Bytes one decoded value may claim.
pub const WIRE_LIMIT: usize = 1 << 28;

/// Encode `value` with the wire codec.
///
/// # Errors
/// The value does not encode (a length past what the layout holds).
pub fn encode<T: Encode + ?Sized>(value: &T) -> Result<Vec<u8>, CodecError> {
    istmo_core::codec::encode(value)
}

/// Decode `bytes` that came from elsewhere, claiming at most `cap` bytes.
///
/// A length a value claims is allocated before its elements are read,
/// so the limit follows the input (64 times its size, at least 64 KiB)
/// and never passes `cap`, what a value of this kind may ever take
/// (rounded up to the codec's next rung). Input longer than `cap` is
/// refused before decoding.
///
/// # Errors
/// The bytes are not a valid encoding, or claim more than that.
pub fn decode_bounded<T: Decode>(bytes: &[u8], cap: usize) -> Result<T, CodecError> {
    istmo_core::codec::decode_capped(bytes, cap).map(|(value, _)| value)
}

/// [`decode_bounded`] up to [`WIRE_LIMIT`].
///
/// # Errors
/// As [`decode_bounded`].
pub fn decode<T: Decode>(bytes: &[u8]) -> Result<T, CodecError> {
    decode_bounded(bytes, WIRE_LIMIT)
}

/// A value that crosses the shell as bytes.
pub trait Payload: Encode + Decode + Clone + Send + Sync + 'static {
    /// Encode with the plugin codec.
    #[must_use]
    fn to_bytes(&self) -> Vec<u8> {
        encode(self).unwrap_or_default()
    }

    /// Decode bytes written by [`Payload::to_bytes`].
    ///
    /// # Errors
    /// [`PayloadError`] when the bytes are not a valid encoding.
    fn from_bytes(bytes: &[u8]) -> Result<Self, PayloadError> {
        decode(bytes).map_err(|e| PayloadError(e.to_string()))
    }
}

impl<T> Payload for T where T: Encode + Decode + Clone + Send + Sync + 'static {}

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

    #[derive(wincode::SchemaWrite, wincode::SchemaRead, Clone, Debug, PartialEq)]
    struct Verse {
        book: u8,
        chapter: u16,
    }

    #[test]
    fn a_forged_length_is_an_error() {
        // A string claiming 2^64 - 1 bytes.
        let forged = [253, 255, 255, 255, 255, 255, 255, 255, 255];
        assert!(<String as Payload>::from_bytes(&forged).is_err());
        assert!(<Vec<Vec<u8>> as Payload>::from_bytes(&forged).is_err());
        // Five bytes claiming 256 MiB: refused before anything is allocated.
        let claim = [252, 0, 0, 0, 0x10];
        assert!(super::decode_bounded::<Vec<u8>>(&claim, super::WIRE_LIMIT).is_err());
        let real = vec![9u8; 2 << 20].to_bytes();
        assert_eq!(
            <Vec<u8> as Payload>::from_bytes(&real).map(|v| v.len()),
            Ok(2 << 20)
        );
        // Past the cap given, by a little or a lot, it is refused.
        let over = vec![9u8; 3 << 20].to_bytes();
        assert!(super::decode_bounded::<Vec<u8>>(&over, 2 << 20).is_err());
        assert!(super::decode_bounded::<Vec<u8>>(&real, 4 << 20).is_ok());
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
