//! Error type surfaced by the crate. Hand-implemented `Display` +
//! `Error` to match the workspace stance (no `thiserror`).

use std::fmt;

/// Errors loading baked handwriting data.
#[derive(Debug)]
pub enum HandwritingError {
    /// The bytes are not a valid archive of the expected type.
    Archive(rkyv::rancor::Error),
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
            Self::Archive(source) => write!(f, "invalid handwriting archive: {source}"),
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
            Self::Archive(source) => Some(source),
            Self::Format { .. } => None,
        }
    }
}

impl From<rkyv::rancor::Error> for HandwritingError {
    fn from(source: rkyv::rancor::Error) -> Self {
        Self::Archive(source)
    }
}
