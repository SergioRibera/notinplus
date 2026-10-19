//! rkyv round-trips for the baked formats.
//!
//! Callers hand in plain byte slices (from `include_bytes!`, a file,
//! the network); decoding copies into an aligned buffer first so no
//! alignment burden leaks into the public API.

use rkyv::api::high::{HighDeserializer, HighSerializer, HighValidator};
use rkyv::bytecheck::CheckBytes;
use rkyv::rancor::Error;
use rkyv::ser::allocator::ArenaHandle;
use rkyv::util::AlignedVec;
use rkyv::{Archive, Deserialize, Serialize};

use crate::error::HandwritingError;

/// A baked root type carrying its own layout version.
pub(crate) trait Versioned {
    const FORMAT: u32;

    fn format(&self) -> u32;
}

/// Decodes `bytes` and rejects archives baked for another layout.
pub(crate) fn decode<T>(bytes: &[u8]) -> Result<T, HandwritingError>
where
    T: Archive + Versioned,
    T::Archived:
        for<'a> CheckBytes<HighValidator<'a, Error>> + Deserialize<T, HighDeserializer<Error>>,
{
    let mut aligned = AlignedVec::<16>::with_capacity(bytes.len());
    aligned.extend_from_slice(bytes);
    let value = rkyv::from_bytes::<T, Error>(&aligned)?;
    if value.format() == T::FORMAT {
        Ok(value)
    } else {
        Err(HandwritingError::Format {
            expected: T::FORMAT,
            found: value.format(),
        })
    }
}

pub(crate) fn encode<T>(value: &T) -> Result<Vec<u8>, Error>
where
    T: for<'a> Serialize<HighSerializer<AlignedVec, ArenaHandle<'a>, Error>>,
{
    rkyv::to_bytes::<Error>(value).map(|bytes| bytes.to_vec())
}
