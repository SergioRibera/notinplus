//! PDF document handle — thin wrapper around `pdfium-render`.
//!
//! Owns a shared `Pdfium` binding installed lazily on first document
//! open. The binding lives in a static `OnceLock` so every document
//! borrows from `&'static Pdfium` and can therefore hold a
//! `PdfDocument<'static>` payload without carrying lifetime parameters
//! into the public API. Concurrency is delegated to pdfium-render 0.9's
//! `thread_safe` feature (default-on) which mutex-wraps every FPDF_*
//! call, so worker threads can call into the shared document safely.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use freya_canvas_bg::Rect;
use pdfium_render::prelude::{PdfDocument as PdfiumDoc, Pdfium};

use crate::error::PdfError;

/// Process-wide `Pdfium` handle. Populated on first call to
/// [`pdfium`], errored variants remembered so we don't retry the
/// (slow) library binding every open.
static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

/// Environment variable checked before falling back to the system
/// library search. Points at a directory containing the platform
/// pdfium shared library (`libpdfium.so` / `.dylib` / `pdfium.dll`).
/// Right for desktop dev where the binary lives in a devshell path
/// that isn't on the default loader path.
const ENV_PDFIUM_LIB_DIR: &str = "PDFIUM_LIB_PATH";

pub(crate) fn pdfium() -> Result<&'static Pdfium, PdfError> {
    match PDFIUM.get_or_init(load_pdfium) {
        Ok(p) => Ok(p),
        Err(msg) => Err(PdfError::PdfiumUnavailable(msg.clone())),
    }
}

fn load_pdfium() -> Result<Pdfium, String> {
    // 1. Explicit override — devshell-friendly and gives packagers a
    //    knob when the library lives outside the default loader path.
    if let Some(dir) = std::env::var_os(ENV_PDFIUM_LIB_DIR) {
        let path = Pdfium::pdfium_platform_library_name_at_path(Path::new(&dir));
        match Pdfium::bind_to_library(&path) {
            Ok(b) => return Ok(Pdfium::new(b)),
            Err(e) => {
                log::warn!(
                    "PDFIUM_LIB_PATH set to {:?} but binding failed: {e}; falling back to system loader",
                    path.display()
                );
            }
        }
    }
    // 2. Next to the executable — where release bundles ship it:
    //    `bin/` (Windows), `../lib` (Linux packages, Nix) and
    //    `../Frameworks` (macOS `.app`). macOS `dlopen` ignores
    //    `LC_RPATH` for bare names, so the system search alone misses it.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    if let Some(exe_dir) = exe_dir {
        let bound = [
            exe_dir.clone(),
            exe_dir.join("../lib"),
            exe_dir.join("../Frameworks"),
        ]
        .iter()
        .map(Pdfium::pdfium_platform_library_name_at_path)
        .filter(|path| path.is_file())
        .find_map(|path| Pdfium::bind_to_library(&path).ok());
        if let Some(b) = bound {
            return Ok(Pdfium::new(b));
        }
    }
    // 3. Default: let dlopen search the process' loader path. Works
    //    on Android (nativeLibraryDir), Linux with pdfium on
    //    LD_LIBRARY_PATH (see the nix devshell), macOS with
    //    DYLD_FALLBACK_LIBRARY_PATH, Windows with pdfium.dll on PATH.
    Pdfium::bind_to_system_library()
        .map(Pdfium::new)
        .map_err(|e| e.to_string())
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
            .ok_or(PdfError::PageOutOfRange {
                requested: page,
                page_count: count,
            })
    }

    /// Snapshot of every page's natural size, in insertion order.
    #[must_use]
    pub fn page_sizes(&self) -> &[(f32, f32)] {
        &self.inner.page_sizes
    }

    /// Extract the plain-text content of a single page.
    ///
    /// Runs synchronously through pdfium; the cost is far below
    /// rasterisation and matches user expectations for search
    /// latency. Callers that want an index across the whole
    /// document should wrap the doc in a
    /// [`crate::search::PdfSearchIndex`] which caches per-page text.
    ///
    /// # Errors
    ///
    /// Returns [`PdfError::PageOutOfRange`] when `page` is out of
    /// range, [`PdfError::Backend`] on pdfium failure.
    pub fn extract_page_text(&self, page: usize) -> Result<String, PdfError> {
        Ok(self.extract_page_text_indexed(page)?.0)
    }

    /// Extract page text plus a per-char byte-offset table.
    ///
    /// The returned `Vec<usize>` has length `char_count + 1`: entry
    /// `i` is the byte offset in the text where pdfium char index
    /// `i` starts; the trailing entry is `text.len()` as a sentinel.
    /// This is what [`crate::PdfSearchIndex::hit_rects`] uses to map
    /// a `SearchHit`'s byte span back to the pdfium char range it
    /// covers.
    ///
    /// Text is built by iterating pdfium's `chars()` in document
    /// order and appending each `unicode_char()`; chars whose
    /// codepoint decodes to `None` (control glyphs, non-BMP without
    /// surrogate pair reconstruction, etc.) contribute a zero-byte
    /// span so the mapping stays aligned with pdfium's char indexing.
    ///
    /// # Errors
    ///
    /// Returns [`PdfError::PageOutOfRange`] when `page` is out of
    /// range, [`PdfError::Backend`] on pdfium failure.
    pub fn extract_page_text_indexed(&self, page: usize) -> Result<(String, Vec<usize>), PdfError> {
        let pdf_page = self.pdf_page(page)?;
        let text = pdf_page
            .text()
            .map_err(|e| PdfError::Backend(e.to_string()))?;
        let char_count = usize::try_from(text.len()).unwrap_or(0);
        let mut out = String::with_capacity(char_count);
        let mut starts = Vec::with_capacity(char_count + 1);
        let chars = text.chars();
        for ch in chars.iter() {
            starts.push(out.len());
            if let Some(c) = ch.unicode_char() {
                out.push(c);
            }
        }
        starts.push(out.len());
        Ok((out, starts))
    }

    /// Tight bounding rects for pdfium char indices `start..end` on
    /// `page`, expressed in page-local top-left coordinates (PDF
    /// points, y grows down). Adjacent chars on the same line are
    /// unioned so a run of glyphs collapses into a single rect;
    /// line breaks emit new rects.
    ///
    /// Returns an empty vec when `end <= start` or the range is empty.
    ///
    /// # Errors
    ///
    /// Returns [`PdfError::PageOutOfRange`] when `page` is out of
    /// range, [`PdfError::Backend`] on pdfium failure.
    pub fn char_rects(&self, page: usize, start: usize, end: usize) -> Result<Vec<Rect>, PdfError> {
        if end <= start {
            return Ok(Vec::new());
        }
        let (_, page_height) = self.page_size(page)?;
        let pdf_page = self.pdf_page(page)?;
        let text = pdf_page
            .text()
            .map_err(|e| PdfError::Backend(e.to_string()))?;
        let char_count = usize::try_from(text.len()).unwrap_or(0);
        let clamped_end = end.min(char_count);
        if start >= clamped_end {
            return Ok(Vec::new());
        }
        let chars = text.chars();
        let mut merged: Vec<Rect> = Vec::new();
        for idx in start..clamped_end {
            let Ok(ch) = chars.get(idx) else {
                continue;
            };
            let Ok(bounds) = ch.tight_bounds() else {
                continue;
            };
            let rect = Rect {
                min_x: bounds.left().value,
                min_y: page_height - bounds.top().value,
                max_x: bounds.right().value,
                max_y: page_height - bounds.bottom().value,
            };
            match merged.last_mut() {
                Some(last) if same_line(last, &rect) => {
                    last.min_x = last.min_x.min(rect.min_x);
                    last.max_x = last.max_x.max(rect.max_x);
                    last.min_y = last.min_y.min(rect.min_y);
                    last.max_y = last.max_y.max(rect.max_y);
                }
                _ => merged.push(rect),
            }
        }
        Ok(merged)
    }

    /// Pdfium char index whose tight bounds contain the page-local
    /// point `(x, y)` (PDF-top-left convention, y grows down), or
    /// `None` when no glyph covers the point.
    ///
    /// Falls back to the nearest char on the same line when the point
    /// hits whitespace between glyphs — this is what the standard
    /// desktop / mobile text selection UX expects (clicking in a gap
    /// snaps to the adjacent glyph rather than failing silently).
    ///
    /// # Errors
    ///
    /// Returns [`PdfError::PageOutOfRange`] when `page` is out of
    /// range, [`PdfError::Backend`] on pdfium failure.
    pub fn hit_char_at(&self, page: usize, x: f32, y: f32) -> Result<Option<usize>, PdfError> {
        let (_, page_height) = self.page_size(page)?;
        let pdf_page = self.pdf_page(page)?;
        let text = pdf_page
            .text()
            .map_err(|e| PdfError::Backend(e.to_string()))?;
        let char_count = usize::try_from(text.len()).unwrap_or(0);
        if char_count == 0 {
            return Ok(None);
        }
        let chars = text.chars();
        // Two-pass: exact containment wins; otherwise nearest glyph on
        // the same line (by x-centre distance).
        let mut best_same_line: Option<(usize, f32)> = None;
        for idx in 0..char_count {
            let Ok(ch) = chars.get(idx) else {
                continue;
            };
            let Ok(bounds) = ch.tight_bounds() else {
                continue;
            };
            let min_y = page_height - bounds.top().value;
            let max_y = page_height - bounds.bottom().value;
            let min_x = bounds.left().value;
            let max_x = bounds.right().value;
            if x >= min_x && x < max_x && y >= min_y && y < max_y {
                return Ok(Some(idx));
            }
            if y >= min_y && y < max_y {
                let centre = (min_x + max_x) * 0.5;
                let dist = (centre - x).abs();
                match best_same_line {
                    Some((_, d)) if d <= dist => {}
                    _ => best_same_line = Some((idx, dist)),
                }
            }
        }
        Ok(best_same_line.map(|(idx, _)| idx))
    }

    fn pdf_page(&self, page: usize) -> Result<pdfium_render::prelude::PdfPage<'_>, PdfError> {
        let count = self.page_count();
        if page >= count {
            return Err(PdfError::PageOutOfRange {
                requested: page,
                page_count: count,
            });
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let idx = page as i32;
        self.inner
            .doc
            .pages()
            .get(idx)
            .map_err(|e| PdfError::Backend(e.to_string()))
    }

    /// Access the underlying pdfium document. Crate-internal — the
    /// backend rasterises through this handle without exposing the
    /// pdfium types on the public surface.
    pub(crate) fn pdfium_doc(&self) -> &PdfiumDoc<'static> {
        &self.inner.doc
    }

    /// Rasterise one page to an RGBA pixel buffer of the requested
    /// size. Returns `(bytes, actual_width, actual_height)` — pdfium
    /// may round the pixel dims to its internal multiples, so callers
    /// that build a Skia image must use the returned dims.
    ///
    /// Synchronous; call off the UI thread for large documents.
    ///
    /// # Errors
    /// [`PdfError::PageOutOfRange`] / [`PdfError::Backend`].
    pub fn render_page_rgba(
        &self,
        page: usize,
        target_width_px: u32,
        target_height_px: u32,
    ) -> Result<(Vec<u8>, u32, u32), PdfError> {
        use pdfium_render::prelude::{PdfRenderConfig, Pixels};
        let pdf_page = self.pdf_page(page)?;
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let config = PdfRenderConfig::new()
            .set_target_size(target_width_px as Pixels, target_height_px as Pixels);
        let bitmap = pdf_page
            .render_with_config(&config)
            .map_err(|e| PdfError::Backend(e.to_string()))?;
        #[allow(clippy::cast_sign_loss)]
        let w = bitmap.width() as u32;
        #[allow(clippy::cast_sign_loss)]
        let h = bitmap.height() as u32;
        Ok((bitmap.as_rgba_bytes(), w, h))
    }
}

fn collect_page_sizes(doc: &PdfiumDoc<'_>) -> Vec<(f32, f32)> {
    doc.pages()
        .iter()
        .map(|p| (p.width().value, p.height().value))
        .collect()
}

/// Same-line heuristic used by [`PdfDocument::char_rects`] to fold
/// adjacent glyphs into one highlight rect. Two rects share a line
/// when their vertical extents overlap by at least half the shorter
/// rect's height — tolerant enough for baseline jitter, tight enough
/// that a soft line break breaks the run.
fn same_line(a: &Rect, b: &Rect) -> bool {
    let overlap = a.max_y.min(b.max_y) - a.min_y.max(b.min_y);
    if overlap <= 0.0 {
        return false;
    }
    let shorter = (a.max_y - a.min_y).min(b.max_y - b.min_y);
    shorter > 0.0 && overlap >= shorter * 0.5
}

#[cfg(test)]
mod tests {
    use super::same_line;
    use freya_canvas_bg::Rect;

    fn line(y: f32, h: f32) -> Rect {
        Rect {
            min_x: 0.0,
            min_y: y,
            max_x: 10.0,
            max_y: y + h,
        }
    }

    #[test]
    fn same_line_true_for_aligned_baselines() {
        assert!(same_line(&line(100.0, 12.0), &line(100.0, 12.0)));
    }

    #[test]
    fn same_line_true_with_small_baseline_drift() {
        // Ascender + descender neighbour with 90% overlap.
        assert!(same_line(&line(100.0, 12.0), &line(101.0, 12.0)));
    }

    #[test]
    fn same_line_false_for_next_line() {
        // Two visually-separated lines (leading > glyph height).
        assert!(!same_line(&line(100.0, 12.0), &line(120.0, 12.0)));
    }

    #[test]
    fn same_line_false_for_touching_but_non_overlapping_rects() {
        assert!(!same_line(&line(100.0, 12.0), &line(112.0, 12.0)));
    }
}

/// One entry of a document's table of contents (its PDF bookmarks).
#[derive(Clone, Debug, PartialEq)]
pub struct TocEntry {
    /// Entry title as written in the file.
    pub title: String,
    /// Zero-based page the entry points at, if it points anywhere.
    pub page: Option<usize>,
    /// Vertical offset inside the page from its top edge, in points.
    pub y_from_top: Option<f32>,
    /// Nested entries.
    pub children: Vec<Self>,
}

impl PdfDocument {
    /// The document's table of contents. Empty when the file has none.
    #[must_use]
    pub fn table_of_contents(&self) -> Vec<TocEntry> {
        use pdfium_render::prelude::PdfBookmark;

        fn walk(node: &PdfBookmark<'_>, heights: &[f32]) -> TocEntry {
            let destination = node.destination();
            let page = destination
                .as_ref()
                .and_then(|d| d.page_index().ok())
                .and_then(|index| usize::try_from(index).ok());
            let y_from_top = destination
                .and_then(|d| d.view_settings().ok())
                .and_then(extract_dest_y_bottom)
                .and_then(|y| page.and_then(|p| heights.get(p)).map(|h| (h - y).max(0.0)));
            let mut children = Vec::new();
            let mut child = node.first_child();
            while let Some(c) = child {
                children.push(walk(&c, heights));
                child = c.next_sibling();
            }
            TocEntry {
                title: node.title().unwrap_or_else(|| "(sin título)".to_owned()),
                page,
                y_from_top,
                children,
            }
        }

        let heights: Vec<f32> = self.page_sizes().iter().map(|&(_, h)| h).collect();
        let bookmarks = self.pdfium_doc().bookmarks();
        let mut entries = Vec::new();
        let mut sibling = bookmarks.root();
        while let Some(node) = sibling {
            entries.push(walk(&node, &heights));
            sibling = node.next_sibling();
        }
        entries
    }
}

/// Pull a bottom-up y (PDF points) out of a destination view setting
/// when the variant specifies one. Fit-type destinations pin the whole
/// page and carry none.
pub(crate) const fn extract_dest_y_bottom(
    settings: pdfium_render::prelude::PdfDestinationViewSettings,
) -> Option<f32> {
    use pdfium_render::prelude::PdfDestinationViewSettings as V;
    match settings {
        V::SpecificCoordinatesAndZoom(_, Some(y), _)
        | V::FitPageHorizontallyToWindow(Some(y))
        | V::FitBoundsHorizontallyToWindow(Some(y)) => Some(y.value),
        _ => None,
    }
}
