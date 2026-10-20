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
/// Subdir holding per-plugin snapshot blobs under each item root.
/// Keyed by plugin id (sanitised to a filesystem-safe form) so a
/// single item can carry snapshots from several plugins without any
/// name collision at the shell level.
const PLUGIN_SNAPSHOTS_SUBDIR: &str = "plugin_snapshots";

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
