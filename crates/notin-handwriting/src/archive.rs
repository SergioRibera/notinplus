//! wincode round-trips for the baked formats.
//!
//! Fixed-width little-endian integers, `u32` lengths and one-byte enum
//! tags: the outlines and traces are mostly `i16`s and verbs, so this
//! keeps the files smaller than any varint layout would. Any alignment
//! is accepted.

use wincode::config::{Configuration, DEFAULT_PREALLOCATION_SIZE_LIMIT};
use wincode::int_encoding::{FixInt, LittleEndian};
use wincode::len::FixIntLen;
use wincode::{SchemaReadOwned, SchemaWrite};

use crate::error::HandwritingError;

/// Layout of every baked file.
type Baked =
    Configuration<true, DEFAULT_PREALLOCATION_SIZE_LIMIT, FixIntLen<u32>, LittleEndian, FixInt, u8>;

/// A baked root type carrying its own layout version.
pub(crate) trait Versioned {
    const FORMAT: u32;

    fn format(&self) -> u32;
}

/// Decodes `bytes` and rejects files baked for another layout.
#[cfg_attr(feature = "sismo", sismo::function("handwriting::archive_decode"))]
pub(crate) fn decode<T>(bytes: &[u8]) -> Result<T, HandwritingError>
where
    T: SchemaReadOwned<Baked, Dst = T> + Versioned,
{
    let value: T = wincode::config::deserialize_exact(bytes, Baked::new())
        .map_err(HandwritingError::Decode)?;
    if value.format() == T::FORMAT {
        Ok(value)
    } else {
        Err(HandwritingError::Format {
            expected: T::FORMAT,
            found: value.format(),
        })
    }
}

pub(crate) fn encode<T>(value: &T) -> Result<Vec<u8>, HandwritingError>
where
    T: SchemaWrite<Baked, Src = T>,
{
    wincode::config::serialize(value, Baked::new()).map_err(HandwritingError::Encode)
}
