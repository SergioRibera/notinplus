//! Error type surfaced by the crate. Hand-implemented `Display` +
//! `Error` to match the workspace stance (no `thiserror`).

use std::fmt;

/// Errors loading baked handwriting data.
#[derive(Debug)]
pub enum HandwritingError {
    /// The bytes are not a valid file of the expected type.
    Decode(wincode::ReadError),
    /// The value could not be written.
    Encode(wincode::WriteError),
    /// The archive was baked for a different format version; re-run
    /// `glyph-bake`.
    Format {
        /// Version this build understands.
        expected: u32,
        /// Version found in the archive.
        found: u32,
    },
}

impl fmt::Display for HandwritingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(source) => write!(f, "invalid handwriting file: {source}"),
            Self::Encode(source) => write!(f, "could not write handwriting file: {source}"),
            Self::Format { expected, found } => write!(
                f,
                "handwriting archive format {found}, expected {expected}; re-run glyph-bake"
            ),
        }
    }
}

impl std::error::Error for HandwritingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Decode(source) => Some(source),
            Self::Encode(source) => Some(source),
            Self::Format { .. } => None,
        }
    }
}
