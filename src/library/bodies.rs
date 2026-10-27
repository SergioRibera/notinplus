//! On-disk layout for doc bodies + PDF attachments.
//!
//! Each library item owns a directory under [`item_root`]. Inside it
//! the stroke doc lives as one snapshot carrying its full history, so
//! a replica can be rebuilt from the file alone. A raw PDF attachment
//! lives in the same item directory so a purge sweeps everything at
//! once.
//!
//! ```text
//! <data_dir>/notinplus/
//! └── items/<item_id>/
//!     ├── doc.loro                      // Doc snapshot + history
//!     ├── attachment.pdf                // raw PDF blob
//!     ├── cover.png                     // cover thumbnail
//!     ├── assets/<name>                 // images linked from the body
//!     └── plugin_snapshots/<plugin>.bincode
//! ```
//!
//! Writes are atomic within a directory: bytes land in a sibling
//! `.tmp` first, then `rename`. `fsync` is skipped (the platform
//! data-store call itself is the durable commit; a truncated body
//! simply fails to decode and the caller resurfaces it as
//! [`crate::library::LibraryError::Codec`]).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::doc::Doc;

use super::error::{LibraryError, Result};
use super::model::ItemId;

/// Root the library carves out under `data_dir`. Kept as a single
/// segment so uninstall / clear-app-data wipes everything at once.
const ROOT_SUBDIR: &str = "notinplus";
const ITEMS_SUBDIR: &str = "items";
const DOC_FILE: &str = "doc.loro";
const PDF_FILE: &str = "attachment.pdf";
const COVER_FILE: &str = "cover.png";
/// Subdir holding per-plugin snapshot blobs under each item root.
/// Keyed by plugin id (sanitised to a filesystem-safe form) so a
/// single item can carry snapshots from several plugins without any
/// name collision at the shell level.
const PLUGIN_SNAPSHOTS_SUBDIR: &str = "plugin_snapshots";
/// Subdir holding the images (and any other file) the item's body links
/// to by relative path.
pub const ASSETS_SUBDIR: &str = "assets";

/// Subdirectories under [`root_dir`] that hold per-item state and are
/// wiped when the library starts over.
const ITEM_STATE_SUBDIRS: [&str; 4] = [ITEMS_SUBDIR, "views", "doc_linked", "pane_layouts"];

/// Absolute path to the library root. Created on demand — call sites
/// that go on to write also call [`ensure_dir`].
#[must_use]
pub fn root_dir() -> PathBuf {
    istmo::path::data_dir().join(ROOT_SUBDIR)
}

/// Per-item directory holding every blob that belongs to `id`. The
/// path is deterministic but not guaranteed to exist on disk;
/// [`ensure_dir`] creates it lazily on each write.
#[must_use]
pub fn item_root(id: ItemId) -> PathBuf {
    root_dir().join(ITEMS_SUBDIR).join(id.to_string())
}

/// Absolute path of the stroke doc snapshot.
#[must_use]
pub fn doc_path(id: ItemId) -> PathBuf {
    item_root(id).join(DOC_FILE)
}

/// Absolute path of the raw PDF attachment.
#[must_use]
pub fn pdf_path(id: ItemId) -> PathBuf {
    item_root(id).join(PDF_FILE)
}

/// Absolute path of a plugin snapshot blob.
///
/// Opaque bincode produced by `CanvasKind::serialize_snapshot`; the
/// shell is pure transport.
/// Plugin id is sanitised to be filesystem-safe (dots preserved,
/// everything else lowercased + non-alnum to `_`).
#[must_use]
pub fn plugin_snapshot_path(id: ItemId, plugin_id: &str) -> PathBuf {
    item_root(id)
        .join(PLUGIN_SNAPSHOTS_SUBDIR)
        .join(format!("{}.bincode", sanitise_plugin_id(plugin_id)))
}

/// Write a plugin snapshot blob. Creates the subdir on first call.
///
/// # Errors
/// Filesystem failure.
pub fn write_plugin_snapshot(id: ItemId, plugin_id: &str, bytes: &[u8]) -> io::Result<()> {
    let path = plugin_snapshot_path(id, plugin_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)
}

/// When the plugin snapshot of `id` was last written, in Unix
/// milliseconds; `None` without one. Doubles as its sync freshness.
#[must_use]
pub fn plugin_snapshot_stamp(id: ItemId, plugin_id: &str) -> Option<u64> {
    let modified = fs::metadata(plugin_snapshot_path(id, plugin_id))
        .ok()?
        .modified()
        .ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    u64::try_from(since.as_millis()).ok()
}

/// Store a plugin snapshot that came from another device, stamped with
/// the time it was written there so it is neither older nor newer than
/// what that device holds.
///
/// # Errors
/// Filesystem failure.
pub fn write_plugin_snapshot_at(
    id: ItemId,
    plugin_id: &str,
    bytes: &[u8],
    written_at_ms: u64,
) -> io::Result<()> {
    write_plugin_snapshot(id, plugin_id, bytes)?;
    let file = fs::File::options()
        .write(true)
        .open(plugin_snapshot_path(id, plugin_id))?;
    file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(written_at_ms))
}

/// Read a plugin snapshot blob. `Ok(None)` on missing file; IO errors
/// on other failures.
///
/// # Errors
/// Filesystem failure other than a missing file.
pub fn read_plugin_snapshot(id: ItemId, plugin_id: &str) -> io::Result<Option<Vec<u8>>> {
    let path = plugin_snapshot_path(id, plugin_id);
    if !path.exists() {
        return Ok(None);
    }
    fs::read(&path).map(Some)
}

fn sanitise_plugin_id(plugin_id: &str) -> String {
    plugin_id
        .chars()
        .map(|c| match c {
            c if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' => c,
            _ => '_',
        })
        .collect()
}

fn ensure_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Remove every per-item directory and sidecar under [`root_dir`].
/// Global preferences survive. Missing directories are not an error.
pub fn discard_store() {
    let root = root_dir();
    for sub in ITEM_STATE_SUBDIRS {
        let path = root.join(sub);
        if path.exists() {
            if let Err(err) = fs::remove_dir_all(&path) {
                log::warn!("discard {}: {err}", path.display());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Stroke doc
// ---------------------------------------------------------------------------

/// Persist `doc` as the item's snapshot.
///
/// # Errors
/// Store export errors surface as [`LibraryError::Codec`], filesystem
/// failures as [`LibraryError::Io`].
pub fn write_doc(id: ItemId, doc: &Doc) -> Result<()> {
    let bytes = doc
        .snapshot_bytes()
        .map_err(|e| LibraryError::Codec(e.to_string()))?;
    write_doc_bytes(id, &bytes)
}

/// Reconstruct a [`Doc`] from the item's snapshot.
///
/// # Errors
/// [`LibraryError::Io`] with `NotFound` when no doc has been written
/// for `id`, [`LibraryError::Codec`] on decode failure.
pub fn read_doc(id: ItemId) -> Result<Doc> {
    let bytes = fs::read(doc_path(id))?;
    Doc::from_snapshot(&bytes).map_err(|e| LibraryError::Codec(e.to_string()))
}

/// Persist snapshot bytes produced by [`Doc::snapshot_bytes`] (or
/// carried verbatim by a bundle) as the item's doc.
///
/// # Errors
/// Filesystem failures.
pub fn write_doc_bytes(id: ItemId, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(item_root(id))?;
    write_blob(&doc_path(id), bytes)
}

/// Raw snapshot bytes of the item's doc. `None` when the item has no
/// doc persisted (an item that only owns an attachment, or one never
/// drawn on).
#[must_use]
pub fn read_doc_bytes(id: ItemId) -> Option<Vec<u8>> {
    fs::read(doc_path(id)).ok()
}

// ---------------------------------------------------------------------------
// Cover
// ---------------------------------------------------------------------------

/// Absolute path of the item's cover thumbnail.
#[must_use]
pub fn cover_path(id: ItemId) -> PathBuf {
    item_root(id).join(COVER_FILE)
}

/// Store the encoded cover of `id`, or remove it with `None`.
///
/// # Errors
/// Filesystem failure.
pub fn write_cover(id: ItemId, bytes: Option<&[u8]>) -> Result<()> {
    bytes.map_or_else(
        || match fs::remove_file(cover_path(id)) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err.into()),
            _ => Ok(()),
        },
        |bytes| write_blob(&cover_path(id), bytes),
    )
}

/// The encoded cover of `id`, when one was stored.
#[must_use]
pub fn read_cover(id: ItemId) -> Option<Vec<u8>> {
    fs::read(cover_path(id)).ok()
}

// ---------------------------------------------------------------------------
// PDF attachment
// ---------------------------------------------------------------------------

/// # Errors
/// Filesystem failure.
pub fn write_pdf(id: ItemId, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(item_root(id))?;
    write_blob(&pdf_path(id), bytes)
}

/// # Errors
/// Filesystem failure, `NotFound` when the item has no attachment.
pub fn read_pdf(id: ItemId) -> Result<Vec<u8>> {
    Ok(fs::read(pdf_path(id))?)
}

/// Remove every on-disk blob owned by `id`. A missing per-item
/// directory is ignored so the caller can prune orphaned metadata
/// idempotently.
pub fn purge(id: ItemId) {
    let _ = fs::remove_dir_all(item_root(id));
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

fn write_blob(dest: &Path, bytes: &[u8]) -> Result<()> {
    ensure_dir(dest)?;
    let mut tmp = dest.to_path_buf();
    // Append `.tmp` to the file name so `.rename` stays within the
    // same directory (cross-directory renames are not atomic).
    let file_name = tmp
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    let mut with_ext = file_name;
    with_ext.push(".tmp");
    tmp.set_file_name(with_ext);
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, dest)?;
    Ok(())
}

/// Bytes `id` occupies on disk (body, PDF, plugin snapshots, views).
#[must_use]
pub fn disk_usage(id: ItemId) -> u64 {
    dir_size(&item_root(id))
}

fn dir_size(path: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return std::fs::metadata(path).map_or(0, |m| m.len());
    };
    entries
        .filter_map(std::result::Result::ok)
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => dir_size(&entry.path()),
            Ok(_) => entry.metadata().map_or(0, |m| m.len()),
            Err(_) => 0,
        })
        .sum()
}

/// Copy every on-disk blob of `from` to a fresh item `to`.
///
/// # Errors
/// Filesystem failure.
pub fn copy_item(from: ItemId, to: ItemId) -> io::Result<()> {
    fn copy_dir(src: &Path, dst: &Path) -> io::Result<()> {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let target = dst.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_dir(&entry.path(), &target)?;
            } else {
                fs::copy(entry.path(), target)?;
            }
        }
        Ok(())
    }
    let src = item_root(from);
    if !src.exists() {
        return Ok(());
    }
    copy_dir(&src, &item_root(to))
}

/// Name and bytes of every file in `id`'s assets folder. Unreadable
/// entries and half written `.tmp` files are skipped.
#[must_use]
pub fn read_assets(id: ItemId) -> Vec<(String, Vec<u8>)> {
    asset_names(id)
        .into_iter()
        .filter_map(|name| {
            let bytes = read_asset(id, &name).ok()?;
            Some((name, bytes))
        })
        .collect()
}

/// Store `bytes` as `name` in `id`'s assets folder. Names with any path
/// separator are refused, so a bundle cannot write outside the folder.
///
/// # Errors
/// I/O failures, or [`LibraryError::Codec`] for a name that is not a
/// plain file name.
pub fn write_asset(id: ItemId, name: &str, bytes: &[u8]) -> Result<()> {
    let mut parts = Path::new(name).components();
    if !matches!(
        (parts.next(), parts.next()),
        (Some(std::path::Component::Normal(_)), None)
    ) {
        return Err(LibraryError::Codec(format!("asset name {name:?}")));
    }
    write_blob(&item_root(id).join(ASSETS_SUBDIR).join(name), bytes)
}

/// Names of the files in `id`'s assets folder, without half written ones.
#[must_use]
pub fn asset_names(id: ItemId) -> Vec<String> {
    let Ok(entries) = fs::read_dir(item_root(id).join(ASSETS_SUBDIR)) else {
        return Vec::new();
    };
    entries
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| Path::new(name).extension().is_none_or(|ext| ext != "tmp"))
        .collect()
}

/// Bytes of the asset `name` of `id`.
///
/// # Errors
/// I/O failures, including a missing file.
pub fn read_asset(id: ItemId, name: &str) -> Result<Vec<u8>> {
    Ok(fs::read(item_root(id).join(ASSETS_SUBDIR).join(name))?)
}

/// Sync identity of the asset `name` of `id`: the first half of the item
/// id, then a hash of the name that is the same on every device.
#[must_use]
pub fn asset_key(id: ItemId, name: &str) -> [u8; 16] {
    // FNV-1a: stable across builds and platforms, unlike `DefaultHasher`.
    let hash = name.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    let mut key = [0; 16];
    key[..8].copy_from_slice(&id.0[..8]);
    key[8..].copy_from_slice(&hash.to_le_bytes());
    key
}

#[cfg(test)]
mod asset_tests {
    use super::{ItemId, asset_key};

    #[test]
    fn asset_keys_are_the_same_everywhere() {
        let item = ItemId([3; 16]);
        let key = asset_key(item, "a.png");
        assert_eq!(&key[..8], &[3; 8]);
        // FNV-1a 64 of "a.png", fixed so every build agrees.
        assert_eq!(key[8..], 0xa292_05b5_6ba8_5779_u64.to_le_bytes());
        assert_ne!(key, asset_key(item, "b.png"));
    }
}
