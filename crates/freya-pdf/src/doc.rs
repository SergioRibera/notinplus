//! PDF document handle — thin wrapper around `pdfium-render`.
//!
//! Owns a shared `Pdfium` binding installed lazily on first document
//! open. The binding lives in a static `OnceLock` so every document
//! borrows from `&'static Pdfium` and can therefore hold a
//! `PdfDocument<'static>` payload without carrying lifetime parameters
//! into the public API.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use pdfium_render::prelude::{PdfDocument as PdfiumDoc, Pdfium};

use crate::error::PdfError;

/// Process-wide `Pdfium` handle. Populated on first call to
/// [`pdfium`], errored variants remembered so we don't retry the
/// (slow) library binding every open.
static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

fn pdfium() -> Result<&'static Pdfium, PdfError> {
    match PDFIUM.get_or_init(|| {
        Pdfium::bind_to_system_library()
            .map(Pdfium::new)
            .map_err(|e| e.to_string())
    }) {
        Ok(p) => Ok(p),
        Err(msg) => Err(PdfError::PdfiumUnavailable(msg.clone())),
    }
}

/// An open PDF document. Cheap to clone (`Arc` shared internally),
/// `Send + Sync` under the crate's `pdfium-render/sync` feature so
/// worker pools can drive rendering off the UI thread.
#[derive(Debug, Clone)]
pub struct PdfDocument {
    inner: Arc<Inner>,
}

struct Inner {
    /// Live pdfium document. Borrows from the static `Pdfium` binding,
    /// so its lifetime is `'static` — safe to store here.
    doc: PdfiumDoc<'static>,
    /// Snapshot of per-page natural size (in PDF points). Populated at
    /// open time so hot `page_size` / layout calls don't traverse
    /// pdfium's page collection on every access.
    page_sizes: Vec<(f32, f32)>,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PdfDocumentInner")
            .field("page_count", &self.page_sizes.len())
            .finish_non_exhaustive()
    }
}

impl PdfDocument {
    /// Open a document from a filesystem path (no password).
    ///
    /// # Errors
    ///
    /// Returns [`PdfError::PdfiumUnavailable`] if the `PDFium` library
    /// cannot be loaded and [`PdfError::Open`] if the file itself
    /// cannot be read or parsed.
    pub fn open(path: &Path) -> Result<Self, PdfError> {
        Self::open_with_password(path, None)
    }

    /// Open a document from a filesystem path, supplying a password
    /// when the file is encrypted.
    ///
    /// Reads the entire file into memory and forwards to
    /// [`Self::open_bytes_with_password`] so pdfium can retain
    /// ownership of the buffer — avoids a lifetime tangle where
    /// pdfium-render's `load_pdf_from_file` unifies the document
    /// lifetime with the password reference, which would forbid the
    /// `'static` payload the crate stores here.
    ///
    /// # Errors
    ///
    /// See [`Self::open`].
    pub fn open_with_password(path: &Path, password: Option<&str>) -> Result<Self, PdfError> {
        let bytes = std::fs::read(path).map_err(|e| PdfError::Open(e.to_string()))?;
        Self::open_bytes_with_password(bytes, password)
    }

    /// Open a document from an in-memory buffer. Takes ownership so
    /// pdfium can hold the bytes for the lifetime of the handle.
    ///
    /// # Errors
    ///
    /// See [`Self::open`].
    pub fn open_bytes(bytes: Vec<u8>) -> Result<Self, PdfError> {
        Self::open_bytes_with_password(bytes, None)
    }

    /// Open a document from an in-memory buffer with a password.
    ///
    /// # Errors
    ///
    /// See [`Self::open`].
    pub fn open_bytes_with_password(
        bytes: Vec<u8>,
        password: Option<&str>,
    ) -> Result<Self, PdfError> {
        let pdfium = pdfium()?;
        let doc = pdfium
            .load_pdf_from_byte_vec(bytes, password)
            .map_err(|e| PdfError::Open(e.to_string()))?;
        let page_sizes = collect_page_sizes(&doc);
        Ok(Self {
            inner: Arc::new(Inner { doc, page_sizes }),
        })
    }

    /// Total number of pages.
    #[must_use]
    pub fn page_count(&self) -> usize {
        self.inner.page_sizes.len()
    }

    /// Natural size of a page in points (72dpi units).
    ///
    /// # Errors
    ///
    /// Returns [`PdfError::PageOutOfRange`] when `page >= page_count`.
    pub fn page_size(&self, page: usize) -> Result<(f32, f32), PdfError> {
        let count = self.page_count();
        self.inner
            .page_sizes
            .get(page)
            .copied()
            .ok_or(PdfError::PageOutOfRange { requested: page, page_count: count })
    }

    /// Snapshot of every page's natural size, in insertion order.
    #[must_use]
    pub fn page_sizes(&self) -> &[(f32, f32)] {
        &self.inner.page_sizes
    }

    /// Access the underlying pdfium document. Crate-internal — the
    /// backend rasterises through this handle without exposing the
    /// pdfium types on the public surface.
    pub(crate) fn pdfium_doc(&self) -> &PdfiumDoc<'static> {
        &self.inner.doc
    }
}

fn collect_page_sizes(doc: &PdfiumDoc<'_>) -> Vec<(f32, f32)> {
    doc.pages()
        .iter()
        .map(|p| (p.width().value, p.height().value))
        .collect()
}
