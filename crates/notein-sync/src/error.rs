//! Error types, one per module boundary.

use std::fmt;

/// Loading, saving or validating the device profile failed.
#[derive(Debug)]
pub enum IdentityError {
    /// Reading or writing the profile file.
    Io(std::io::Error),
    /// The file is not a profile this build understands.
    Codec(String),
    /// A profile written by a newer or older build.
    Version(u32),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "device profile io: {err}"),
            Self::Codec(msg) => write!(f, "device profile codec: {msg}"),
            Self::Version(v) => write!(f, "device profile version {v} not supported"),
        }
    }
}

impl std::error::Error for IdentityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Codec(_) | Self::Version(_) => None,
        }
    }
}

impl From<std::io::Error> for IdentityError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// Sealing or opening a payload with a share key failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CipherError {
    /// Fewer bytes than a nonce plus a tag.
    TooShort(usize),
    /// Wrong key, or the bytes were tampered with.
    Rejected,
    /// A key string was not 64 hex characters.
    BadKey,
}

impl fmt::Display for CipherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort(len) => write!(f, "sealed payload too short ({len} bytes)"),
            Self::Rejected => f.write_str("sealed payload rejected"),
            Self::BadKey => f.write_str("share key must be 64 hex characters"),
        }
    }
}

impl std::error::Error for CipherError {}

/// An exchange step failed.
#[derive(Debug)]
pub enum SyncError<E> {
    /// The local replica refused an operation.
    Replica(E),
    /// A message could not be encoded or decoded.
    Codec(String),
    /// A message from a protocol version this build does not speak.
    Version(u16),
}

impl<E: fmt::Display> fmt::Display for SyncError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Replica(err) => write!(f, "replica: {err}"),
            Self::Codec(msg) => write!(f, "sync codec: {msg}"),
            Self::Version(v) => write!(f, "sync protocol version {v} not supported"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for SyncError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Replica(err) => Some(err),
            Self::Codec(_) | Self::Version(_) => None,
        }
    }
}
