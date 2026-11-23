//! Small device-local records (settings, window state, creation usage,
//! search history…) kept together in one file, `local.bin`, under the
//! data directory.
//!
//! The file maps each record's name to that record's own encoding, so
//! records stay independent: a record whose layout changed, or that no
//! longer decodes, reads as its `Default` without touching the others.
//!
//! Records used to live in a file each (`settings.v2.bin`, …). The
//! first read of a record missing from `local.bin` takes the old file's
//! bytes, keeps them here and removes the old file.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use istmo::codec::{Decode, Encode};

use crate::library::bodies;

/// File holding every record.
const LOCAL_FILE: &str = "local.bin";

/// A record kept in `local.bin` under [`Self::FILE`].
pub trait Persisted: Encode + Decode + Default {
    /// Name of the record. It was the record's own file name, which is
    /// where a record not moved yet is read from.
    const FILE: &'static str;

    /// Read the record, falling back to `Default`.
    #[must_use]
    #[sismo::function("persist::load")]
    fn load() -> Self {
        local()
            .get(Self::FILE)
            .and_then(|bytes| notin_plugin::decode::<Self>(&bytes).ok())
            .unwrap_or_default()
    }

    /// Replace the record and write `local.bin` atomically.
    ///
    /// # Errors
    /// Encoding or filesystem failures.
    #[sismo::function("persist::save")]
    fn save(&self) -> io::Result<()> {
        let bytes = notin_plugin::encode(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        local().put(Self::FILE, bytes)
    }
}

/// What `local.bin` holds.
#[istmo::message]
#[derive(Debug, Default)]
struct LocalFile {
    records: Vec<(String, Vec<u8>)>,
}

/// The records in memory, read from disk on first use.
#[derive(Debug, Default)]
struct Local {
    records: Option<BTreeMap<String, Vec<u8>>>,
}

static LOCAL: Mutex<Local> = Mutex::new(Local { records: None });

fn local() -> MutexGuard<'static, Local> {
    LOCAL.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Local {
    fn dir() -> PathBuf {
        bodies::root_dir()
    }

    fn records(&mut self) -> &mut BTreeMap<String, Vec<u8>> {
        self.records.get_or_insert_with(|| {
            std::fs::read(Self::dir().join(LOCAL_FILE))
                .ok()
                .and_then(|bytes| notin_plugin::decode::<LocalFile>(&bytes).ok())
                .map(|file| file.records.into_iter().collect())
                .unwrap_or_default()
        })
    }

    /// The bytes of `key`, taking them from its old file once.
    fn get(&mut self, key: &str) -> Option<Vec<u8>> {
        if let Some(bytes) = self.records().get(key) {
            return Some(bytes.clone());
        }
        let old = Self::dir().join(key);
        let bytes = std::fs::read(&old).ok()?;
        match self.put(key, bytes.clone()) {
            Ok(()) => {
                if let Err(err) = std::fs::remove_file(&old) {
                    tracing::warn!("removing {}: {err}", old.display());
                }
            }
            // The old file stays; the next read tries again.
            Err(err) => tracing::warn!("moving {key} into {LOCAL_FILE}: {err}"),
        }
        Some(bytes)
    }

    fn put(&mut self, key: &str, bytes: Vec<u8>) -> io::Result<()> {
        self.records().insert(key.to_owned(), bytes);
        let file = LocalFile {
            records: self
                .records()
                .iter()
                .map(|(key, bytes)| (key.clone(), bytes.clone()))
                .collect(),
        };
        let encoded = notin_plugin::encode(&file)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        write_atomic(&Self::dir().join(LOCAL_FILE), &encoded)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[istmo::message]
    #[derive(Debug, Default, PartialEq, Eq)]
    struct Sample {
        count: u32,
    }

    impl Persisted for Sample {
        const FILE: &'static str = "persist-test-sample.bin";
    }

    #[istmo::message]
    #[derive(Debug, Default, PartialEq, Eq)]
    struct Other {
        name: String,
    }

    impl Persisted for Other {
        const FILE: &'static str = "persist-test-other.bin";
    }

    #[test]
    fn records_move_in_from_their_old_files_and_stay_apart() {
        let dir = Local::dir();
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join(Sample::FILE);
        std::fs::write(&old, notin_plugin::encode(&Sample { count: 7 }).unwrap()).unwrap();

        assert_eq!(Sample::load(), Sample { count: 7 });
        assert!(!old.exists(), "the old file is gone once moved");

        Other { name: "x".into() }.save().unwrap();
        // Garbage under one record leaves the other alone.
        local().put(Sample::FILE, vec![0xFF; 3]).unwrap();
        assert_eq!(Sample::load(), Sample::default());
        assert_eq!(Other::load(), Other { name: "x".into() });

        // What is on disk reads back the same.
        local().records = None;
        assert_eq!(Other::load(), Other { name: "x".into() });
    }
}
