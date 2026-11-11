//! Small device-local records stored as one encoded file each under
//! the data directory (settings, toolbar state, creation usage).
//!
//! A record that is missing or no longer decodes reads as its
//! `Default`: a schema change never surfaces as an error.

use std::io;
use std::path::{Path, PathBuf};

use istmo::codec::{Decode, Encode};

use crate::library::bodies;

/// A record persisted as `<data dir>/<FILE>`.
pub trait Persisted: Encode + Decode + Default {
    /// File name inside the data directory.
    const FILE: &'static str;

    /// Full path of the record.
    #[must_use]
    fn path() -> PathBuf {
        bodies::root_dir().join(Self::FILE)
    }

    /// Read the record, falling back to `Default`.
    #[must_use]
    #[sismo::function("persist::load")]
    fn load() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|bytes| notin_plugin::decode::<Self>(&bytes).ok())
            .unwrap_or_default()
    }

    /// Atomically replace the record on disk.
    ///
    /// # Errors
    /// Encoding or filesystem failures.
    #[sismo::function("persist::save")]
    fn save(&self) -> io::Result<()> {
        let bytes = notin_plugin::encode(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        write_atomic(&Self::path(), &bytes)
    }
}

/// Write `bytes` to `dest` through a sibling temp file and a rename.
///
/// # Errors
/// Filesystem failures.
#[sismo::function("persist::write_atomic")]
pub fn write_atomic(dest: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut name = dest
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(".tmp");
    let tmp = dest.with_file_name(name);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, dest)
}
