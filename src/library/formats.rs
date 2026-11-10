//! The shapes a library has been stored in, and which of them this
//! build reads.
//!
//! A library in a shape this build cannot read is never opened over or
//! wiped on its own: the person sees it first and chooses to keep a
//! copy, to start over, or to leave it as it is.

use std::io::Write;

use istmo_data_store::DataStoreClient;

use super::bodies;
use super::error::{LibraryError, Result};
use super::index::INDEX_KEY;

/// One shape the library index has been stored in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredFormat {
    /// data-store key the index lives under in this shape.
    pub key: &'static str,
    /// First release that wrote it.
    pub written_by: &'static str,
    /// This build opens it.
    pub readable: bool,
}

/// Every shape the index has had, oldest first. A new shape that
/// breaks the stored one gets a row here with a new key, and the rows
/// it cannot read turn `readable: false`.
pub const FORMATS: &[StoredFormat] = &[
    StoredFormat {
        key: "library.index.v3",
        written_by: "0.1.0-alpha.1",
        readable: false,
    },
    StoredFormat {
        key: INDEX_KEY,
        written_by: "0.1.0-alpha.2",
        readable: true,
    },
];

/// Prefix of every data-store key a library writes.
const STORE_PREFIX: &str = "library.";

/// What a library keeps on disk next to the store, under
/// [`bodies::root_dir`]. The device profile, its shares and the
/// preferences live there too and are not the library's.
const LIBRARY_DIRS: [&str; 5] = [
    "items",
    "plugin_snapshots",
    "views",
    "doc_linked",
    "pane_layouts",
];
const LIBRARY_FILES: [&str; 2] = ["graveyard.bin", "refs.bincode"];

/// A library on this device in a shape this build cannot read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Incompatible {
    pub format: &'static StoredFormat,
}

/// The library on this device when this build cannot read it; `None`
/// when it can, or when there is none yet.
///
/// # Errors
/// The store could not be read.
#[sismo::function("library::formats::detect")]
pub(super) async fn detect(client: &DataStoreClient) -> Result<Option<Incompatible>> {
    for format in FORMATS.iter().rev() {
        if client.contains(format.key.to_owned()).await? {
            return Ok((!format.readable).then_some(Incompatible { format }));
        }
    }
    Ok(None)
}

/// Everything the library left on this device, store and files, as a
/// `tar.zst` archive: what a build that reads `format` restores from.
///
/// # Errors
/// The store or a file could not be read.
#[sismo::function("library::formats::backup")]
pub async fn backup() -> Result<Vec<u8>> {
    let client = client().await?;
    let encoder = zstd::stream::write::Encoder::new(Vec::new(), 3)?;
    let mut tar = tar::Builder::new(encoder);
    tar.mode(tar::HeaderMode::Deterministic);
    for key in client.keys().await? {
        if !key.starts_with(STORE_PREFIX) {
            continue;
        }
        let Some(bytes) = client.get_bytes(key.clone()).await? else {
            continue;
        };
        append(&mut tar, &format!("store/{key}"), &bytes)?;
    }
    // Only the library: the device profile next to it holds the keys
    // that prove this device to the others, and a copy is no place
    // for them.
    let root = bodies::root_dir();
    for dir in LIBRARY_DIRS {
        let path = root.join(dir);
        if path.is_dir() {
            tar.append_dir_all(format!("files/{dir}"), &path)?;
        }
    }
    for file in LIBRARY_FILES {
        let path = root.join(file);
        if path.is_file() {
            tar.append_path_with_name(&path, format!("files/{file}"))?;
        }
    }
    let encoder = tar.into_inner()?;
    Ok(encoder.finish()?)
}

/// Delete the library on this device: every key it wrote and every
/// file it keeps. The device, its pairings and its preferences stay.
///
/// # Errors
/// The store or a file could not be removed.
#[sismo::function("library::formats::wipe")]
pub async fn wipe() -> Result<()> {
    let client = client().await?;
    for key in client.keys().await? {
        if key.starts_with(STORE_PREFIX) {
            client.remove(key).await?;
        }
    }
    let root = bodies::root_dir();
    for dir in LIBRARY_DIRS {
        gone(std::fs::remove_dir_all(root.join(dir)))?;
    }
    for file in LIBRARY_FILES {
        gone(std::fs::remove_file(root.join(file)))?;
    }
    Ok(())
}

/// [`detect`] on the store of this process.
///
/// # Errors
/// The runtime is not up or the store could not be read.
pub async fn on_this_device() -> Result<Option<Incompatible>> {
    detect(&client().await?).await
}

async fn client() -> Result<DataStoreClient> {
    let runtime = istmo::Runtime::global().map_err(LibraryError::Runtime)?;
    super::store_client(&runtime).await
}

/// A removal that found nothing to remove still succeeded.
fn gone(removed: std::io::Result<()>) -> Result<()> {
    match removed {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(LibraryError::Io(err)),
        _ => Ok(()),
    }
}

fn append<W: Write>(tar: &mut tar::Builder<W>, name: &str, bytes: &[u8]) -> Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append_data(&mut header, name, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::FORMATS;

    #[test]
    fn only_the_newest_format_is_read() {
        let (newest, older) = FORMATS.split_last().expect("formats");
        assert!(newest.readable);
        assert!(older.iter().all(|f| !f.readable));
    }

    #[test]
    fn every_format_has_its_own_key() {
        let mut keys: Vec<_> = FORMATS.iter().map(|f| f.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), FORMATS.len());
    }
}
