//! On-disk layout for doc bodies + PDF attachments.
//!
//! Docs and PDFs are the two blob kinds a library item can own. Both
//! sit under a stable app-owned root resolved via
//! [`istmo::path::data_dir`], so the library survives an app upgrade
//! and never clashes with the user's `~/Documents`.
//!
//! ```text
//! <data_dir>/notinplus/
//!     docs/<item_id>.notinplus     // bincode-encoded [`Doc`]
//!     pdfs/<item_id>.pdf           // raw PDF bytes (PdfCanvas only)
//!     thumbs/<item_id>.png         // reserved for future previews
//! ```
//!
//! Writes are atomic: bytes land in a sibling `.tmp` first, `fsync` is
//! skipped (the platform data-store call itself is the durable
//! commit; a truncated body simply fails to decode and the caller
//! resurfaces it as [`crate::library::LibraryError::Codec`]).

use std::fs;
use std::io;
use std::path::PathBuf;

use bincode::config::{self, Configuration};

use crate::doc::Doc;

use super::error::{LibraryError, Result};
use super::model::ItemId;

const CODEC: Configuration = config::standard();

/// Root the library carves out under `data_dir`. Kept as a single
/// segment so uninstall / clear-app-data wipes everything at once.
const ROOT_SUBDIR: &str = "notinplus";
const DOCS_SUBDIR: &str = "docs";
const PDFS_SUBDIR: &str = "pdfs";
const DOC_EXT: &str = "notinplus";
const PDF_EXT: &str = "pdf";

/// Absolute path to the library root. Created on demand — call sites
/// that go on to write also call [`ensure_dir`].
#[must_use]
pub fn root_dir() -> PathBuf {
    istmo::path::data_dir().join(ROOT_SUBDIR)
}

/// Absolute path of the doc body for `id`. The parent dir is not
/// guaranteed to exist; use [`ensure_dir`] before writing.
#[must_use]
pub fn doc_path(id: ItemId) -> PathBuf {
    root_dir()
        .join(DOCS_SUBDIR)
        .join(format!("{}.{DOC_EXT}", id.0))
}

/// Absolute path of the PDF attachment for `id`.
#[must_use]
pub fn pdf_path(id: ItemId) -> PathBuf {
    root_dir()
        .join(PDFS_SUBDIR)
        .join(format!("{}.{PDF_EXT}", id.0))
}

fn ensure_dir(path: &std::path::Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Encode `doc` and land it at [`doc_path`] atomically.
///
/// # Errors
/// Bubbles up bincode encode errors as [`LibraryError::Codec`] and
/// filesystem failures as [`LibraryError::Io`].
pub fn write_doc(id: ItemId, doc: &Doc) -> Result<()> {
    let bytes =
        bincode::encode_to_vec(doc, CODEC).map_err(|e| LibraryError::Codec(e.to_string()))?;
    write_blob(&doc_path(id), &bytes)
}

/// Decode the doc body at [`doc_path`].
///
/// # Errors
/// [`LibraryError::Io`] when the file is missing / unreadable,
/// [`LibraryError::Codec`] when the payload cannot be decoded (usually
/// a schema drift from an older app build).
pub fn read_doc(id: ItemId) -> Result<Doc> {
    let bytes = fs::read(doc_path(id))?;
    let (doc, _) = bincode::decode_from_slice::<Doc, _>(&bytes, CODEC)
        .map_err(|e| LibraryError::Codec(e.to_string()))?;
    Ok(doc)
}

/// Persist raw PDF bytes at [`pdf_path`]. Overwrites any previous
/// attachment for the same id.
///
/// # Errors
/// Filesystem failure.
pub fn write_pdf(id: ItemId, bytes: &[u8]) -> Result<()> {
    write_blob(&pdf_path(id), bytes)
}

/// Read raw PDF bytes back. Returns [`LibraryError::Io`] when the item
/// has no PDF on disk.
///
/// # Errors
/// Filesystem failure.
pub fn read_pdf(id: ItemId) -> Result<Vec<u8>> {
    Ok(fs::read(pdf_path(id))?)
}

/// Remove every on-disk blob owned by `id`. Missing files are ignored
/// so the caller can prune orphaned metadata idempotently.
pub fn purge(id: ItemId) {
    let _ = fs::remove_file(doc_path(id));
    let _ = fs::remove_file(pdf_path(id));
}

fn write_blob(dest: &std::path::Path, bytes: &[u8]) -> Result<()> {
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
