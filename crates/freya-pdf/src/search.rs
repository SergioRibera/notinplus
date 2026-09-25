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
//! Each [`SearchHit`] carries character offsets; call
//! [`PdfSearchIndex::hit_rects`] to resolve them into page-local
//! bounding rects (top-left origin, PDF points) suitable for
//! overlaying a highlight during paint. World-space rects — already
//! translated by the page's layout offset — come from
//! [`crate::PdfBackground::world_hit_rects`].

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use freya_canvas_bg::{PageId, Rect};

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
    cache: RwLock<HashMap<u64, PageIndex>>,
}

/// Cached per-page text plus the byte-offset table needed to map a
/// [`SearchHit`] back to a pdfium char range. Byte-offset table has
/// length `char_count + 1`: entry `i` is the byte offset of pdfium
/// char index `i` in `text`; the trailing entry is `text.len()`.
#[derive(Debug, Clone)]
struct PageIndex {
    text: Arc<String>,
    char_byte_starts: Arc<Vec<usize>>,
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
        Ok(self.page_index(page)?.text)
    }

    fn page_index(&self, page: PageId) -> Result<PageIndex, PdfError> {
        if let Some(entry) = self.read_cache(page.0) {
            return Ok(entry);
        }
        let idx = usize::try_from(page.0).map_err(|_| PdfError::PageOutOfRange {
            requested: usize::MAX,
            page_count: self.inner.doc.page_count(),
        })?;
        let (text, starts) = self.inner.doc.extract_page_text_indexed(idx)?;
        let entry = PageIndex {
            text: Arc::new(text),
            char_byte_starts: Arc::new(starts),
        };
        self.write_cache(page.0, entry.clone());
        Ok(entry)
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

    /// Bounding rects for a search hit, in page-local top-left
    /// coordinates (PDF points, y grows down).
    ///
    /// Multiple rects are returned when the match wraps across
    /// lines — each rect covers one contiguous line run of the
    /// matched glyphs. Callers stacking the rects into a paint
    /// overlay should translate by the page's world offset (see
    /// [`crate::PdfBackground::world_hit_rects`]).
    ///
    /// # Errors
    ///
    /// Propagates any [`PdfError`] from pdfium (e.g. page out of
    /// range, tight-bounds failure).
    pub fn hit_rects(&self, hit: SearchHit) -> Result<Vec<Rect>, PdfError> {
        if hit.match_len == 0 {
            return Ok(Vec::new());
        }
        let entry = self.page_index(hit.page)?;
        let starts = entry.char_byte_starts.as_slice();
        let start_byte = hit.char_offset;
        let end_byte = hit.char_offset.saturating_add(hit.match_len);
        let (start_char, end_char) = char_range_for_bytes(starts, start_byte, end_byte);
        let idx = usize::try_from(hit.page.0).map_err(|_| PdfError::PageOutOfRange {
            requested: usize::MAX,
            page_count: self.inner.doc.page_count(),
        })?;
        self.inner.doc.char_rects(idx, start_char, end_char)
    }

    fn read_cache(&self, page: u64) -> Option<PageIndex> {
        self.read_lock().get(&page).cloned()
    }

    fn write_cache(&self, page: u64, entry: PageIndex) {
        self.write_lock().insert(page, entry);
    }

    fn read_lock(&self) -> std::sync::RwLockReadGuard<'_, HashMap<u64, PageIndex>> {
        match self.inner.cache.read() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn write_lock(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<u64, PageIndex>> {
        match self.inner.cache.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Map a `[start_byte, end_byte)` slice of a page's extracted text
/// back to the pdfium char range covering it.
///
/// `starts` is the byte-offset table produced by
/// [`crate::doc::PdfDocument::extract_page_text_indexed`]: entry `i`
/// is the byte offset of pdfium char `i`, with a trailing sentinel at
/// `text.len()`. The returned `(start_char, end_char)` half-open range
/// always satisfies `start_char <= end_char <= chars.len()`; an empty
/// range signals no chars fall within the requested byte span.
fn char_range_for_bytes(starts: &[usize], start_byte: usize, end_byte: usize) -> (usize, usize) {
    let char_count = starts.len().saturating_sub(1);
    let start_char = match starts.binary_search(&start_byte) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    }
    .min(char_count);
    let end_char = match starts.binary_search(&end_byte) {
        Ok(i) | Err(i) => i,
    }
    .min(char_count);
    (start_char, end_char.max(start_char))
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
    fn char_range_ascii_span_matches_exact_bytes() {
        // Text "abcd" — one byte per char, starts = [0,1,2,3,4].
        let starts = [0, 1, 2, 3, 4];
        assert_eq!(char_range_for_bytes(&starts, 1, 3), (1, 3));
        assert_eq!(char_range_for_bytes(&starts, 0, 4), (0, 4));
        assert_eq!(char_range_for_bytes(&starts, 2, 2), (2, 2));
    }

    #[test]
    fn char_range_multibyte_start_snaps_back_to_char_boundary() {
        // "aéb": bytes a=1, é=2, b=1 → starts = [0, 1, 3, 4].
        let starts = [0, 1, 3, 4];
        // Query byte offset 2 (mid-é) → should snap to char index 1.
        assert_eq!(char_range_for_bytes(&starts, 2, 3), (1, 2));
        // Range covering all three chars.
        assert_eq!(char_range_for_bytes(&starts, 0, 4), (0, 3));
    }

    #[test]
    fn char_range_end_past_sentinel_clamps() {
        let starts = [0, 1, 2, 3];
        // end_byte beyond the sentinel — end_char stays at sentinel.
        assert_eq!(char_range_for_bytes(&starts, 0, 99), (0, 3));
    }

    #[test]
    fn char_range_empty_span_is_empty() {
        let starts = [0, 1, 2, 3];
        let (a, b) = char_range_for_bytes(&starts, 1, 1);
        assert_eq!(a, b);
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
