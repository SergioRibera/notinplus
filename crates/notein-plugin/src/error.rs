//! Error enums returned from trait methods.
//!
//! Every variant carries a short diagnostic string so the host can log
//! the plugin's own explanation without needing a dedicated error
//! taxonomy per plugin.

use rkyv::{Archive, Deserialize, Serialize};

/// Error from [`crate::Source`] methods.
#[derive(Clone, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub enum SourceError {
    OpenFailed(String),
    UnknownInstance,
    RenderFailed(String),
    AnchorNotFound,
    SearchFailed(String),
    Other(String),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenFailed(msg) => write!(f, "source open failed: {msg}"),
            Self::UnknownInstance => f.write_str("source instance unknown"),
            Self::RenderFailed(msg) => write!(f, "source render failed: {msg}"),
            Self::AnchorNotFound => f.write_str("anchor not resolvable"),
            Self::SearchFailed(msg) => write!(f, "source search failed: {msg}"),
            Self::Other(msg) => write!(f, "source error: {msg}"),
        }
    }
}

impl std::error::Error for SourceError {}

/// Error from [`crate::CanvasKind`] methods.
#[derive(Clone, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub enum CanvasError {
    OpenFailed(String),
    UnknownInstance,
    UnsupportedCapability(String),
    Other(String),
}

impl std::fmt::Display for CanvasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenFailed(msg) => write!(f, "canvas open failed: {msg}"),
            Self::UnknownInstance => f.write_str("canvas instance unknown"),
            Self::UnsupportedCapability(cap) => {
                write!(f, "canvas does not support capability: {cap}")
            }
            Self::Other(msg) => write!(f, "canvas error: {msg}"),
        }
    }
}

impl std::error::Error for CanvasError {}

/// Error setting a config value.
#[derive(Clone, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub enum ConfigError {
    UnknownKey(String),
    TypeMismatch,
    OutOfRange,
    Rejected(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownKey(k) => write!(f, "unknown config key: {k}"),
            Self::TypeMismatch => f.write_str("config value type mismatch"),
            Self::OutOfRange => f.write_str("config value out of range"),
            Self::Rejected(msg) => write!(f, "config value rejected: {msg}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Error executing a [`crate::CreationEntry`].
#[derive(Clone, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub enum CreationError {
    UnknownEntry(String),
    MissingImportBytes,
    Invalid(String),
    BackendFailed(String),
}

impl std::fmt::Display for CreationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownEntry(id) => write!(f, "unknown creation entry id: {id}"),
            Self::MissingImportBytes => f.write_str("creation entry requires import bytes"),
            Self::Invalid(msg) => write!(f, "creation input invalid: {msg}"),
            Self::BackendFailed(msg) => write!(f, "creation backend failed: {msg}"),
        }
    }
}

impl std::error::Error for CreationError {}
