//! On-disk layout for doc bodies + PDF attachments.
//!
//! Each library item owns a directory under [`item_root`]. Inside it
//! the stroke doc lives as a snapshot + Lamport anchor + append-only
//! log, so sync can diff by `OpId` and the log can be compacted
//! independently from the materialised state. A raw PDF attachment
//! lives in the same item directory so a purge sweeps everything at
//! once.
//!
//! ```text
//! <data_dir>/notinplus/
//! └── items/<item_id>/
//!     ├── snapshot.bincode              // Doc with log cleared
//!     ├── snapshot_at.bincode           // OpId the snapshot reflects
//!     ├── log/00001.bincode             // Vec<OpRecord>, append-only
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

use bincode::config::{self, Configuration};

use crate::doc::Doc;
use crate::doc_op::OpRecord;
use crate::op::OpId;

use super::error::{LibraryError, Result};
use super::model::ItemId;

const CODEC: Configuration = config::standard();

/// Root the library carves out under `data_dir`. Kept as a single
/// segment so uninstall / clear-app-data wipes everything at once.
const ROOT_SUBDIR: &str = "notinplus";
const ITEMS_SUBDIR: &str = "items";
const SNAPSHOT_FILE: &str = "snapshot.bincode";
const SNAPSHOT_AT_FILE: &str = "snapshot_at.bincode";
const LOG_SUBDIR: &str = "log";
const PDF_FILE: &str = "attachment.pdf";
/// Subdir holding per-plugin snapshot blobs under each item root.
/// Keyed by plugin id (sanitised to a filesystem-safe form) so a
/// single item can carry snapshots from several plugins without any
/// name collision at the shell level.
const PLUGIN_SNAPSHOTS_SUBDIR: &str = "plugin_snapshots";
/// First (and today only) log segment. Compaction will rotate to
/// `00002.bincode`, `00003.bincode`, … and glob the dir in sorted
/// order on read.
const LOG_SEGMENT_0: &str = "00001.bincode";

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

/// Absolute path of the stroke doc snapshot (= [`Doc`] with the log
/// stripped). Pair with [`snapshot_at_path`] / [`log_dir`].
#[must_use]
pub fn doc_path(id: ItemId) -> PathBuf {
    item_root(id).join(SNAPSHOT_FILE)
}

/// Absolute path of the bincoded [`OpId`] anchoring the stroke doc
/// snapshot. Written next to [`doc_path`] on each save.
#[must_use]
pub fn snapshot_at_path(id: ItemId) -> PathBuf {
    item_root(id).join(SNAPSHOT_AT_FILE)
}

/// Absolute path of the stroke doc log directory.
#[must_use]
pub fn log_dir(id: ItemId) -> PathBuf {
    item_root(id).join(LOG_SUBDIR)
}

/// Absolute path of the raw PDF attachment.
#[must_use]
pub fn pdf_path(id: ItemId) -> PathBuf {
    item_root(id).join(PDF_FILE)
}

/// Absolute path of a plugin snapshot blob. Opaque bincode produced
/// by `CanvasKind::serialize_snapshot`; the shell is pure transport.
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

/// Encode `doc` into the per-item split layout. Compacts first so the
/// log never exceeds the configured threshold on disk.
///
/// # Errors
/// Bubbles up bincode encode errors as [`LibraryError::Codec`] and
/// filesystem failures as [`LibraryError::Io`].
pub fn write_doc(id: ItemId, doc: &mut Doc) -> Result<()> {
    doc.maybe_compact_log();
    let (snapshot_bytes, at_bytes, log_bytes) = encode_doc_parts(doc)?;
    let root = item_root(id);
    fs::create_dir_all(&root)?;
    fs::create_dir_all(root.join(LOG_SUBDIR))?;
    write_blob(&doc_path(id), &snapshot_bytes)?;
    write_blob(&snapshot_at_path(id), &at_bytes)?;
    write_blob(&log_dir(id).join(LOG_SEGMENT_0), &log_bytes)?;
    Ok(())
}

/// Reconstruct a [`Doc`] from the split layout.
///
/// # Errors
/// [`LibraryError::Io`] with `NotFound` when no doc has been written
/// for `id`, [`LibraryError::Codec`] on decode failure.
pub fn read_doc(id: ItemId) -> Result<Doc> {
    if !doc_path(id).exists() {
        return Err(LibraryError::Io(io::Error::from(io::ErrorKind::NotFound)));
    }
    decode_doc_parts(&fs::read(doc_path(id))?, &read_log_dir(&log_dir(id))?)
}

/// Persist raw bincoded [`Doc`] bytes verbatim — the bundle importer
/// hands us a payload the exporter already encoded, and we split it
/// into the on-disk layout here. Internally this still goes through
/// the typed [`write_doc`] path so the split happens exactly once per
/// save.
///
/// # Errors
/// Codec / filesystem failures.
pub fn write_doc_bytes(id: ItemId, bytes: &[u8]) -> Result<()> {
    let (mut doc, _) = bincode::decode_from_slice::<Doc, _>(bytes, CODEC)
        .map_err(|e| LibraryError::Codec(e.to_string()))?;
    write_doc(id, &mut doc)
}

/// Reassemble a bincoded [`Doc`] blob for an item. Bundle export
/// needs a single-blob round-trip even though the item lives split on
/// disk.
///
/// Returns `None` when the item has no stroke doc persisted (missing
/// files, or an item that only owns an attachment).
#[must_use]
pub fn read_doc_bytes(id: ItemId) -> Option<Vec<u8>> {
    let doc = read_doc(id).ok()?;
    bincode::encode_to_vec(&doc, CODEC).ok()
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

fn encode_doc_parts(doc: &Doc) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let at = doc.log().last().map_or_else(OpId::default, |r| r.id);
    let snapshot = Doc {
        log: Vec::new(),
        ..doc.clone()
    };
    let snapshot_bytes =
        bincode::encode_to_vec(&snapshot, CODEC).map_err(|e| LibraryError::Codec(e.to_string()))?;
    let at_bytes =
        bincode::encode_to_vec(&at, CODEC).map_err(|e| LibraryError::Codec(e.to_string()))?;
    let log_bytes =
        bincode::encode_to_vec(doc.log(), CODEC).map_err(|e| LibraryError::Codec(e.to_string()))?;
    Ok((snapshot_bytes, at_bytes, log_bytes))
}

fn decode_doc_parts(snapshot_bytes: &[u8], log_bytes: &[u8]) -> Result<Doc> {
    let (mut doc, _) = bincode::decode_from_slice::<Doc, _>(snapshot_bytes, CODEC)
        .map_err(|e| LibraryError::Codec(e.to_string()))?;
    if !log_bytes.is_empty() {
        let (log, _) = bincode::decode_from_slice::<Vec<OpRecord>, _>(log_bytes, CODEC)
            .map_err(|e| LibraryError::Codec(e.to_string()))?;
        doc.log = log;
    }
    Ok(doc)
}

fn read_log_dir(dir: &Path) -> Result<Vec<u8>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut segments: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    segments.sort();
    if segments.len() == 1 {
        return Ok(fs::read(&segments[0])?);
    }
    let mut combined: Vec<OpRecord> = Vec::new();
    for seg in &segments {
        let bytes = fs::read(seg)?;
        if bytes.is_empty() {
            continue;
        }
        let (chunk, _) = bincode::decode_from_slice::<Vec<OpRecord>, _>(&bytes, CODEC)
            .map_err(|e| LibraryError::Codec(e.to_string()))?;
        combined.extend(chunk);
    }
    bincode::encode_to_vec(&combined, CODEC).map_err(|e| LibraryError::Codec(e.to_string()))
}

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
