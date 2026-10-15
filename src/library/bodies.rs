//! On-disk layout for doc bodies + PDF attachments.
//!
//! Each library item owns a directory under
//! [`item_root`]. Inside it the canonical stroke-canvas doc lives as a
//! snapshot + Lamport anchor + append-only log, so Phase 10 sync can
//! diff by `OpId` and the log can be compacted independently from the
//! materialised state. DocLinked bundles carry a sibling
//! canvas-side split; the raw PDF attachment lives in the same item
//! directory so a purge sweeps everything at once.
//!
//! ```text
//! <data_dir>/notinplus/
//! └── items/<item_id>/
//!     ├── snapshot.bincode              // Doc with log cleared
//!     ├── snapshot_at.bincode           // OpId the snapshot reflects
//!     ├── log/00001.bincode             // Vec<OpRecord>, append-only
//!     ├── canvas_snapshot.bincode       // DocLinked canvas face
//!     ├── canvas_snapshot_at.bincode
//!     ├── canvas_log/00001.bincode
//!     └── attachment.pdf                // PdfCanvas / DocLinked blob
//! ```
//!
//! A one-shot migration folds the legacy
//! `docs/<id>.notinplus` + `pdfs/<id>.pdf` layout into the new tree on
//! first read of an item. Writes always target the new tree.
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
const CANVAS_SNAPSHOT_FILE: &str = "canvas_snapshot.bincode";
const CANVAS_SNAPSHOT_AT_FILE: &str = "canvas_snapshot_at.bincode";
const CANVAS_LOG_SUBDIR: &str = "canvas_log";
const PDF_FILE: &str = "attachment.pdf";
/// First (and today only) log segment. Phase 10 compaction will
/// rotate to `00002.bincode`, `00003.bincode`, … and glob the dir in
/// sorted order on read.
const LOG_SEGMENT_0: &str = "00001.bincode";

/// Legacy layout — resident on-disk before the per-item directory
/// split. Looked up on read and migrated on write.
const LEGACY_DOCS_SUBDIR: &str = "docs";
const LEGACY_PDFS_SUBDIR: &str = "pdfs";
const LEGACY_DOC_EXT: &str = "notinplus";
const LEGACY_PDF_EXT: &str = "pdf";
const LEGACY_CANVAS_DOC_EXT: &str = "canvas.notinplus";

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

/// Absolute path of the stroke-canvas snapshot (= [`Doc`] with the
/// log stripped). Pair with [`snapshot_at_path`] / [`log_dir`].
#[must_use]
pub fn doc_path(id: ItemId) -> PathBuf {
    item_root(id).join(SNAPSHOT_FILE)
}

/// Absolute path of the bincoded [`OpId`] anchoring the stroke-canvas
/// snapshot. Written next to [`doc_path`] on each save.
#[must_use]
pub fn snapshot_at_path(id: ItemId) -> PathBuf {
    item_root(id).join(SNAPSHOT_AT_FILE)
}

/// Absolute path of the stroke-canvas log directory.
#[must_use]
pub fn log_dir(id: ItemId) -> PathBuf {
    item_root(id).join(LOG_SUBDIR)
}

/// Absolute path of the DocLinked bundle's canvas-face snapshot.
#[must_use]
pub fn canvas_doc_path(id: ItemId) -> PathBuf {
    item_root(id).join(CANVAS_SNAPSHOT_FILE)
}

/// Absolute path of the DocLinked bundle's canvas-face OpId anchor.
#[must_use]
pub fn canvas_snapshot_at_path(id: ItemId) -> PathBuf {
    item_root(id).join(CANVAS_SNAPSHOT_AT_FILE)
}

/// Absolute path of the DocLinked bundle's canvas-face log dir.
#[must_use]
pub fn canvas_log_dir(id: ItemId) -> PathBuf {
    item_root(id).join(CANVAS_LOG_SUBDIR)
}

/// Absolute path of the raw PDF attachment.
#[must_use]
pub fn pdf_path(id: ItemId) -> PathBuf {
    item_root(id).join(PDF_FILE)
}

fn ensure_dir(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Stroke-canvas face
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
    retire_legacy_doc(id);
    Ok(())
}

/// Reconstruct a [`Doc`] from the split layout. Falls back to the
/// legacy single-blob file when the new tree is absent — a one-shot
/// migration path for installs that pre-date the split.
///
/// # Errors
/// [`LibraryError::Io`] when neither layout holds the id,
/// [`LibraryError::Codec`] on decode failure.
pub fn read_doc(id: ItemId) -> Result<Doc> {
    if doc_path(id).exists() {
        return decode_doc_parts(
            &fs::read(doc_path(id))?,
            &read_log_dir(&log_dir(id))?,
        );
    }
    if let Some(bytes) = read_legacy_doc_bytes(id) {
        let (doc, _) = bincode::decode_from_slice::<Doc, _>(&bytes, CODEC)
            .map_err(|e| LibraryError::Codec(e.to_string()))?;
        return Ok(doc);
    }
    Err(LibraryError::Io(io::Error::from(io::ErrorKind::NotFound)))
}

/// Persist raw bincoded [`Doc`] bytes verbatim — the bundle importer
/// hands us a payload the exporter already encoded, and we split it
/// into the new layout here. Internally this still goes through the
/// typed [`write_doc`] path so the split happens exactly once per
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
/// Returns `None` when the item has no stroke-canvas face persisted
/// (missing files, or a bare `SourcePdf` that only owns an
/// attachment).
#[must_use]
pub fn read_doc_bytes(id: ItemId) -> Option<Vec<u8>> {
    let doc = read_doc(id).ok()?;
    bincode::encode_to_vec(&doc, CODEC).ok()
}

// ---------------------------------------------------------------------------
// DocLinked canvas face (mirror of the stroke-canvas functions above)
// ---------------------------------------------------------------------------

/// # Errors
/// Codec / filesystem failures (see [`write_doc`]).
pub fn write_canvas_doc(id: ItemId, doc: &mut Doc) -> Result<()> {
    doc.maybe_compact_log();
    let (snapshot_bytes, at_bytes, log_bytes) = encode_doc_parts(doc)?;
    let root = item_root(id);
    fs::create_dir_all(&root)?;
    fs::create_dir_all(root.join(CANVAS_LOG_SUBDIR))?;
    write_blob(&canvas_doc_path(id), &snapshot_bytes)?;
    write_blob(&canvas_snapshot_at_path(id), &at_bytes)?;
    write_blob(&canvas_log_dir(id).join(LOG_SEGMENT_0), &log_bytes)?;
    retire_legacy_canvas_doc(id);
    Ok(())
}

/// # Errors
/// Same shape as [`read_doc`].
pub fn read_canvas_doc(id: ItemId) -> Result<Doc> {
    if canvas_doc_path(id).exists() {
        return decode_doc_parts(
            &fs::read(canvas_doc_path(id))?,
            &read_log_dir(&canvas_log_dir(id))?,
        );
    }
    if let Some(bytes) = read_legacy_canvas_doc_bytes(id) {
        let (doc, _) = bincode::decode_from_slice::<Doc, _>(&bytes, CODEC)
            .map_err(|e| LibraryError::Codec(e.to_string()))?;
        return Ok(doc);
    }
    Err(LibraryError::Io(io::Error::from(io::ErrorKind::NotFound)))
}

/// # Errors
/// Codec / filesystem failures.
pub fn write_canvas_doc_bytes(id: ItemId, bytes: &[u8]) -> Result<()> {
    let (mut doc, _) = bincode::decode_from_slice::<Doc, _>(bytes, CODEC)
        .map_err(|e| LibraryError::Codec(e.to_string()))?;
    write_canvas_doc(id, &mut doc)
}

// ---------------------------------------------------------------------------
// PDF attachment
// ---------------------------------------------------------------------------

/// # Errors
/// Filesystem failure.
pub fn write_pdf(id: ItemId, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(item_root(id))?;
    write_blob(&pdf_path(id), bytes)?;
    retire_legacy_pdf(id);
    Ok(())
}

/// # Errors
/// Filesystem failure.
pub fn read_pdf(id: ItemId) -> Result<Vec<u8>> {
    if let Ok(bytes) = fs::read(pdf_path(id)) {
        return Ok(bytes);
    }
    if let Some(bytes) = read_legacy_pdf_bytes(id) {
        return Ok(bytes);
    }
    Err(LibraryError::Io(io::Error::from(io::ErrorKind::NotFound)))
}

/// Remove every on-disk blob owned by `id`. Missing files + missing
/// per-item directory are ignored so the caller can prune orphaned
/// metadata idempotently.
pub fn purge(id: ItemId) {
    let _ = fs::remove_dir_all(item_root(id));
    retire_legacy_doc(id);
    retire_legacy_canvas_doc(id);
    retire_legacy_pdf(id);
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
    let snapshot_bytes = bincode::encode_to_vec(&snapshot, CODEC)
        .map_err(|e| LibraryError::Codec(e.to_string()))?;
    let at_bytes = bincode::encode_to_vec(&at, CODEC)
        .map_err(|e| LibraryError::Codec(e.to_string()))?;
    let log_bytes = bincode::encode_to_vec(doc.log(), CODEC)
        .map_err(|e| LibraryError::Codec(e.to_string()))?;
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

fn legacy_doc_path(id: ItemId) -> PathBuf {
    root_dir()
        .join(LEGACY_DOCS_SUBDIR)
        .join(format!("{id}.{LEGACY_DOC_EXT}"))
}

fn legacy_canvas_doc_path(id: ItemId) -> PathBuf {
    root_dir()
        .join(LEGACY_DOCS_SUBDIR)
        .join(format!("{id}.{LEGACY_CANVAS_DOC_EXT}"))
}

fn legacy_pdf_path(id: ItemId) -> PathBuf {
    root_dir()
        .join(LEGACY_PDFS_SUBDIR)
        .join(format!("{id}.{LEGACY_PDF_EXT}"))
}

fn read_legacy_doc_bytes(id: ItemId) -> Option<Vec<u8>> {
    fs::read(legacy_doc_path(id)).ok()
}

fn read_legacy_canvas_doc_bytes(id: ItemId) -> Option<Vec<u8>> {
    fs::read(legacy_canvas_doc_path(id)).ok()
}

fn read_legacy_pdf_bytes(id: ItemId) -> Option<Vec<u8>> {
    fs::read(legacy_pdf_path(id)).ok()
}

fn retire_legacy_doc(id: ItemId) {
    let _ = fs::remove_file(legacy_doc_path(id));
}

fn retire_legacy_canvas_doc(id: ItemId) {
    let _ = fs::remove_file(legacy_canvas_doc_path(id));
}

fn retire_legacy_pdf(id: ItemId) {
    let _ = fs::remove_file(legacy_pdf_path(id));
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
