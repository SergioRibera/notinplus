//! Typed payloads that the shell stores and ships as bytes.
//!
//! A plugin's anchors and document content are ordinary Rust types.
//! The shell never looks inside them: it keeps their bytes, and the
//! adapters in [`crate::erased`] encode and decode them with one codec
//! on both sides.

use bincode::config::{self, Configuration, Limit, LittleEndian, Varint};
use bincode::{Decode, Encode};

/// The bincode configuration for bytes that come from elsewhere.
///
/// Other devices, imported files, plugins: a length past [`WIRE_LIMIT`] is
/// refused before anything is allocated, so a forged one can neither
/// exhaust memory nor abort the process.
pub type Wire = Configuration<LittleEndian, Varint, Limit<WIRE_LIMIT>>;
/// Bytes one decoded value may claim.
pub const WIRE_LIMIT: usize = 1 << 28;
/// See [`Wire`]. Encodes the same as `bincode::config::standard()`.
pub const WIRE: Wire = config::standard().with_limit::<WIRE_LIMIT>();

const CODEC: Wire = WIRE;

/// Decode `bytes` that came from elsewhere, claiming at most `cap` bytes.
///
/// bincode allocates a length a value claims before reading it, up to
/// the configured limit, so with one large limit a few forged bytes
/// would make it allocate all of it. Here the limit follows the input:
/// 64 times its size (at least 64 KiB), and never more than `cap`, what
/// a value of this kind may ever take.
///
/// # Errors
/// The bytes are not a valid encoding, or claim more than that.
pub fn decode_bounded<T: Decode<()>>(
    bytes: &[u8],
    cap: usize,
) -> Result<T, bincode::error::DecodeError> {
    // Every cap is a power of two, and so is every limit below: none is
    // rounded past the cap it stands for.
    debug_assert!(cap.is_power_of_two());
    if bytes.len() > cap {
        return Err(bincode::error::DecodeError::LimitExceeded);
    }
    let need = bytes.len().saturating_mul(64).max(1 << 16).min(cap);
    macro_rules! within {
        ($($limit:expr),*) => {
            $(
                if need <= $limit {
                    let config = config::standard().with_limit::<{ $limit }>();
                    return bincode::decode_from_slice(bytes, config).map(|(value, _)| value);
                }
            )*
        };
    }
    within!(
        1 << 12,
        1 << 13,
        1 << 14,
        1 << 15,
        1 << 16,
        1 << 17,
        1 << 18,
        1 << 19,
        1 << 20,
        1 << 21,
        1 << 22,
        1 << 23,
        1 << 24,
        1 << 25,
        1 << 26,
        1 << 27,
        1 << 28,
        1 << 29
    );
    bincode::decode_from_slice(bytes, WIRE).map(|(value, _)| value)
}

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
        decode_bounded(bytes, WIRE_LIMIT).map_err(|e| PayloadError(e.to_string()))
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
