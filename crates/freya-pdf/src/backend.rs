//! [`CanvasBackground`] impl backed by [`PdfDocument`].
//!
//! M0 stub: exposes the constructor + trait impl the host will wire in
//! M1/M2. Actual page rendering (worker pool, cache, tiling) lands in
//! M3–M5.

use freya_canvas_bg::{AttributionMode, BgPaintCtx, CanvasBackground, Color, PageLayout, Rect};

use crate::doc::PdfDocument;

/// PDF-backed canvas background. Owns a [`PdfDocument`] handle and (in
/// later milestones) the render pool + cache.
#[derive(Debug)]
pub struct PdfBackground {
    doc: PdfDocument,
    // Populated in M2 by the layout pass (StackedPagesBackground-style
    // vertical stack, one entry per page).
    pages: Vec<PageLayout>,
}

impl PdfBackground {
    /// Wrap a document. Layout is computed lazily on the first paint
    /// pass; the returned background reports zero pages until then.
    #[must_use]
    pub const fn new(doc: PdfDocument) -> Self {
        Self { doc, pages: Vec::new() }
    }

    /// Access the underlying document (for text extraction, etc.).
    #[must_use]
    pub const fn document(&self) -> &PdfDocument {
        &self.doc
    }
}

impl CanvasBackground for PdfBackground {
    fn content_bounds(&self) -> Option<Rect> {
        // Union of every page rect. Filled in with the layout pass.
        None
    }

    fn pages(&self) -> &[PageLayout] {
        &self.pages
    }

    fn viewport_backdrop(&self) -> Option<Color> {
        // Neutral gray between pages, matching common PDF viewers.
        Some(Color::from_rgb(90, 90, 96))
    }

    fn attribution_mode(&self) -> AttributionMode {
        AttributionMode::PerPage
    }

    fn paint(&self, _cx: &mut BgPaintCtx<'_>) {
        // M3: cull pages against cx.visible, request bitmaps via the
        // render pool at cx.scale, draw the best cached LOD.
    }
}
