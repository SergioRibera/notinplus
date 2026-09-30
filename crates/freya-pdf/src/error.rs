//! Error type surfaced by the crate. Hand-implemented `Display` +
//! `Error` to match the workspace stance (no `thiserror`).

use std::fmt;

/// Errors produced by the PDF backend.
#[derive(Debug)]
pub enum PdfError {
    /// Failed to locate or initialise the `PDFium` shared library.
    /// Enable the `bundled` cargo feature to have `pdfium-render`
    /// ship a prebuilt binary.
    PdfiumUnavailable(String),
    /// Failed to open a document (bad path, unreadable, corrupt).
    Open(String),
    /// Referenced a page index outside the document's range.
    PageOutOfRange {
        /// Page index the caller asked for.
        requested: usize,
        /// Actual number of pages in the document.
        page_count: usize,
    },
    /// `PDFium` reported a runtime error while executing a request.
    Backend(String),
}

impl fmt::Display for PdfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PdfiumUnavailable(msg) => {
                write!(f, "pdfium unavailable: {msg}")
            }
            Self::Open(msg) => write!(f, "failed to open pdf: {msg}"),
            Self::PageOutOfRange {
                requested,
                page_count,
            } => write!(
                f,
                "page {requested} out of range (document has {page_count})"
            ),
            Self::Backend(msg) => write!(f, "pdfium backend error: {msg}"),
        }
    }
}

impl std::error::Error for PdfError {}
