//! Text extraction and search over a [`PdfDocument`].
//!
//! [`PdfSearchIndex`] wraps a document with a lazy per-page text
//! cache. Callers hit [`Self::find`] (whole document) or
//! [`Self::find_in_page`] (single page) and receive a
//! [`SearchHit`] for every match. Extraction runs synchronously via
//! pdfium; the text-per-page cost is much lower than rasterisation
//! and matches user expectations for search latency.
//!
//! # Case handling
//!
//! [`SearchOptions::case_sensitive`] toggles ASCII case folding via
//! `to_lowercase` on both haystack and needle. Full Unicode
//! case folding (e.g. German ß ↔ SS) is out of scope for M6 — reach
//! for the `caseless` crate if the app needs it.
//!
//! # Highlight geometry
//!
//! The M6 API returns character offsets only. Bounding rects for
//! rendering highlight overlays land in a follow-up (needs per-char
//! `tight_bounds` calls against pdfium and world-space projection).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use freya_canvas_bg::PageId;

use crate::doc::PdfDocument;
use crate::error::PdfError;

/// Search options. Defaults to case-insensitive whole-substring match.
#[derive(Debug, Clone, Copy, Default)]
pub struct SearchOptions {
    /// When `true`, comparison is byte-exact. When `false`, both
    /// haystack and needle are ASCII-lowercased before matching.
    /// Default (`false`) matches user expectations for search UI.
    pub case_sensitive: bool,
}

/// One match returned by [`PdfSearchIndex::find`] /
/// [`PdfSearchIndex::find_in_page`].
///
/// Offsets are into the page's extracted text — indexing back into
/// [`PdfSearchIndex::page_text`] with `char_offset .. char_offset +
/// match_len` yields the matched substring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchHit {
    /// Page the hit lives on.
    pub page: PageId,
    /// Byte offset into the page's extracted text.
    pub char_offset: usize,
    /// Byte length of the match in the page's text.
    pub match_len: usize,
}

/// Lazy text index over a [`PdfDocument`].
///
/// Extraction is triggered on first access per page and cached in a
/// `RwLock<HashMap>` so repeated queries hit memory instead of
/// pdfium. Cheap to clone (`Arc` shared internally).
#[derive(Debug, Clone)]
pub struct PdfSearchIndex {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    doc: PdfDocument,
    cache: RwLock<HashMap<u64, Arc<String>>>,
}

impl PdfSearchIndex {
    /// Wrap a document with an empty text cache.
    #[must_use]
    pub fn new(doc: PdfDocument) -> Self {
        Self {
            inner: Arc::new(Inner {
                doc,
                cache: RwLock::new(HashMap::new()),
            }),
        }
    }

    /// Underlying document.
    #[must_use]
    pub fn document(&self) -> &PdfDocument {
        &self.inner.doc
    }

    /// Fetch (and cache) the extracted text for one page.
    ///
    /// # Errors
    ///
    /// Propagates any [`PdfError`] from pdfium (e.g. page out of
    /// range, decode failure).
    pub fn page_text(&self, page: PageId) -> Result<Arc<String>, PdfError> {
        if let Some(text) = self.read_cache(page.0) {
            return Ok(text);
        }
        let idx = usize::try_from(page.0).map_err(|_| PdfError::PageOutOfRange {
            requested: usize::MAX,
            page_count: self.inner.doc.page_count(),
        })?;
        let text = Arc::new(self.inner.doc.extract_page_text(idx)?);
        self.write_cache(page.0, Arc::clone(&text));
        Ok(text)
    }

    /// Drop every cached page text. Right after a document swap or
    /// low-memory pressure.
    pub fn clear_cache(&self) {
        self.write_lock().clear();
    }

    /// Find every occurrence of `query` on a single page.
    ///
    /// # Errors
    ///
    /// Propagates [`Self::page_text`] failures.
    pub fn find_in_page(
        &self,
        page: PageId,
        query: &str,
        opts: SearchOptions,
    ) -> Result<Vec<SearchHit>, PdfError> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let text = self.page_text(page)?;
        Ok(scan(&text, query, page, opts))
    }

    /// Find every occurrence of `query` across the whole document.
    ///
    /// Hits are returned in page order. Fails on the first per-page
    /// extraction error.
    ///
    /// # Errors
    ///
    /// Propagates [`Self::page_text`] failures.
    pub fn find(&self, query: &str, opts: SearchOptions) -> Result<Vec<SearchHit>, PdfError> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let count = self.inner.doc.page_count();
        let mut all = Vec::new();
        for i in 0..count {
            #[allow(clippy::cast_possible_truncation)]
            let page = PageId(i as u64);
            let text = self.page_text(page)?;
            all.extend(scan(&text, query, page, opts));
        }
        Ok(all)
    }

    fn read_cache(&self, page: u64) -> Option<Arc<String>> {
        self.read_lock().get(&page).cloned()
    }

    fn write_cache(&self, page: u64, text: Arc<String>) {
        self.write_lock().insert(page, text);
    }

    fn read_lock(&self) -> std::sync::RwLockReadGuard<'_, HashMap<u64, Arc<String>>> {
        match self.inner.cache.read() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn write_lock(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<u64, Arc<String>>> {
        match self.inner.cache.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

fn scan(text: &str, query: &str, page: PageId, opts: SearchOptions) -> Vec<SearchHit> {
    if opts.case_sensitive {
        text.match_indices(query)
            .map(|(offset, m)| SearchHit {
                page,
                char_offset: offset,
                match_len: m.len(),
            })
            .collect()
    } else {
        // ASCII case fold on both sides. Preserving byte offsets means
        // we can't touch multibyte chars — for lowercase mapping that
        // stays a single byte (e.g. ASCII A→a) offsets are stable.
        // Non-ASCII case queries fall back to case-sensitive behaviour
        // via this same path; a full Unicode-aware search is out of
        // scope for M6.
        let hay = text.to_ascii_lowercase();
        let needle = query.to_ascii_lowercase();
        hay.match_indices(&needle)
            .map(|(offset, m)| SearchHit {
                page,
                char_offset: offset,
                match_len: m.len(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_default_case_insensitive() {
        let opts = SearchOptions::default();
        assert!(!opts.case_sensitive);
    }

    #[test]
    fn scan_finds_multiple_case_insensitive() {
        let hits = scan(
            "The quick Brown fox jumps over the brown dog",
            "brown",
            PageId(0),
            SearchOptions { case_sensitive: false },
        );
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].char_offset, 10);
        assert_eq!(hits[1].char_offset, 35);
        assert!(hits.iter().all(|h| h.match_len == 5));
        assert!(hits.iter().all(|h| h.page == PageId(0)));
    }

    #[test]
    fn scan_case_sensitive_skips_wrong_case() {
        let hits = scan(
            "Brown vs brown",
            "brown",
            PageId(1),
            SearchOptions { case_sensitive: true },
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].char_offset, 9);
    }

    #[test]
    fn scan_empty_needle_returns_none_via_caller() {
        // scan() itself doesn't guard empty; caller does. Verify the
        // behavioural contract for find_in_page's empty-needle branch.
        let hits = scan("hello", "", PageId(0), SearchOptions::default());
        // match_indices yields a hit at every position for the empty
        // pattern — that's why the caller short-circuits.
        assert!(hits.len() > 1);
    }
}
