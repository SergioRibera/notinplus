//! PDF document handle — thin wrapper around `pdfium-render`.
//!
//! M0 stub: exposes the public surface a downstream app can rely on
//! (`open`, `page_count`, `page_size`) as `unimplemented!` bodies so
//! the API is stable before the pdfium integration lands in M3.

use std::path::Path;
use std::sync::Arc;

use crate::error::PdfError;

/// An open PDF document. Cheap to clone (`Arc` shared internally).
#[derive(Debug, Clone)]
pub struct PdfDocument {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    // Populated in M3 with the pdfium document handle + per-page
    // metadata snapshot (natural size in points, rotation).
    _reserved: (),
}

impl PdfDocument {
    /// Open a document from a filesystem path.
    ///
    /// # Errors
    ///
    /// Returns [`PdfError::PdfiumUnavailable`] if the `PDFium` library
    /// cannot be loaded and [`PdfError::Open`] if the file itself
    /// cannot be read or parsed.
    pub fn open(_path: &Path) -> Result<Self, PdfError> {
        unimplemented!("PdfDocument::open lands in M3 alongside pdfium wiring")
    }

    /// Open a document from an in-memory buffer. Takes ownership so
    /// pdfium can hold the bytes for the lifetime of the handle.
    ///
    /// # Errors
    ///
    /// See [`Self::open`].
    pub fn open_bytes(_bytes: Vec<u8>) -> Result<Self, PdfError> {
        unimplemented!("PdfDocument::open_bytes lands in M3")
    }

    /// Total number of pages.
    #[must_use]
    pub const fn page_count(&self) -> usize {
        let _ = &self.inner;
        0
    }

    /// Natural size of a page in points (72dpi units).
    ///
    /// # Errors
    ///
    /// Returns [`PdfError::PageOutOfRange`] when `page >= page_count`.
    pub const fn page_size(&self, page: usize) -> Result<(f32, f32), PdfError> {
        Err(PdfError::PageOutOfRange { requested: page, page_count: self.page_count() })
    }
}
