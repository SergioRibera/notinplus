//! [`CanvasBackground`] impl backed by [`PdfDocument`].
//!
//! M3 shape: synchronous per-frame rasterisation. Each visible page
//! is rendered on the paint thread via pdfium and drawn into its
//! world-space rectangle. Adequate for small documents and demos; the
//! M4 worker pool + LRU cache replace this loop with cache lookups
//! and off-thread rendering.

use freya_engine::prelude::{
    AlphaType, ColorType, Data, Image, ImageInfo, Paint, Rect as SkRect, raster_from_data,
};
use freya_canvas_bg::{
    AttributionMode, BgPaintCtx, CanvasBackground, Color, PageAttachment, PageId, PageLayout,
    Rect,
};
use pdfium_render::prelude::{PdfRenderConfig, Pixels};

use crate::doc::PdfDocument;

/// Default vertical gap between pages, in world units (= PDF points).
const DEFAULT_GAP: f32 = 12.0;
/// Default backdrop — neutral gray matching common PDF viewers.
const DEFAULT_BACKDROP: Color = Color::from_rgb(90, 90, 96);

/// PDF-backed canvas background. Owns a [`PdfDocument`] handle and the
/// pre-computed page layout.
#[derive(Debug)]
pub struct PdfBackground {
    doc: PdfDocument,
    gap: f32,
    backdrop: Color,
    pages: Vec<PageLayout>,
    bounds: Rect,
}

impl PdfBackground {
    /// Wrap a document with the default gap and backdrop. Layout is
    /// computed eagerly from the document's page-size snapshot so the
    /// first paint pass has nothing to precompute.
    #[must_use]
    pub fn new(doc: PdfDocument) -> Self {
        Self::with_settings(doc, DEFAULT_GAP, DEFAULT_BACKDROP)
    }

    /// Wrap a document with an explicit gap between pages and backdrop
    /// color.
    #[must_use]
    pub fn with_settings(doc: PdfDocument, gap: f32, backdrop: Color) -> Self {
        let (pages, bounds) = compute_layout(&doc, gap);
        Self { doc, gap, backdrop, pages, bounds }
    }

    /// Access the underlying document (for text extraction, page
    /// counts, etc.).
    #[must_use]
    pub const fn document(&self) -> &PdfDocument {
        &self.doc
    }

    /// Vertical gap between pages in world units.
    #[must_use]
    pub const fn gap(&self) -> f32 {
        self.gap
    }
}

impl CanvasBackground for PdfBackground {
    fn content_bounds(&self) -> Option<Rect> {
        if self.pages.is_empty() {
            None
        } else {
            Some(self.bounds)
        }
    }

    fn pages(&self) -> &[PageLayout] {
        &self.pages
    }

    fn viewport_backdrop(&self) -> Option<Color> {
        Some(self.backdrop)
    }

    fn attribution_mode(&self) -> AttributionMode {
        AttributionMode::PerPage
    }

    fn locate(&self, x: f32, y: f32) -> Option<PageAttachment> {
        self.pages.iter().find_map(|pl| {
            if pl.rect.contains(x, y) {
                Some(PageAttachment {
                    page: pl.id,
                    local: (x - pl.rect.min_x, y - pl.rect.min_y),
                })
            } else {
                None
            }
        })
    }

    fn paint(&self, cx: &mut BgPaintCtx<'_>) {
        let paint = Paint::default();
        for pl in &self.pages {
            if !pl.rect.intersects(&cx.visible) {
                continue;
            }
            let target_w = pixel_dimension(pl.rect.width(), cx.scale);
            let target_h = pixel_dimension(pl.rect.height(), cx.scale);
            if target_w == 0 || target_h == 0 {
                continue;
            }
            // NOTE(M3): synchronous per-frame render. M4 replaces this
            // with a worker-pool + LRU cache lookup; the shape of the
            // call stays the same so callers don't churn when the
            // async pipeline lands.
            let Some(image) = render_page(&self.doc, pl.id, target_w, target_h) else {
                continue;
            };
            let dst = SkRect::from_ltrb(pl.rect.min_x, pl.rect.min_y, pl.rect.max_x, pl.rect.max_y);
            cx.canvas.draw_image_rect(&image, None, dst, &paint);
        }
    }
}

fn compute_layout(doc: &PdfDocument, gap: f32) -> (Vec<PageLayout>, Rect) {
    let sizes = doc.page_sizes();
    if sizes.is_empty() {
        return (Vec::new(), EMPTY_RECT);
    }
    let mut pages = Vec::with_capacity(sizes.len());
    let mut y = 0.0_f32;
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    for (idx, &(w, h)) in sizes.iter().enumerate() {
        let x = -(w * 0.5);
        let rect = Rect {
            min_x: x,
            min_y: y,
            max_x: x + w,
            max_y: y + h,
        };
        min_x = min_x.min(rect.min_x);
        max_x = max_x.max(rect.max_x);
        pages.push(PageLayout {
            #[allow(clippy::cast_possible_truncation)]
            id: PageId(idx as u64),
            rect,
            natural_size: (w, h),
        });
        y += h + gap;
    }
    let bottom = y - gap;
    let bounds = Rect { min_x, min_y: 0.0, max_x, max_y: bottom };
    (pages, bounds)
}

fn render_page(doc: &PdfDocument, page: PageId, target_w: u32, target_h: u32) -> Option<Image> {
    #[allow(clippy::cast_possible_truncation)]
    let page_idx = page.0 as u16;
    let pages = doc.pdfium_doc().pages();
    let pdf_page = pages.get(page_idx).ok()?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let config = PdfRenderConfig::new().set_target_size(target_w as Pixels, target_h as Pixels);
    let bitmap = pdf_page.render_with_config(&config).ok()?;
    let bytes = bitmap.as_rgba_bytes();
    let (w, h) = (bitmap.width(), bitmap.height());
    let info = ImageInfo::new((w, h), ColorType::RGBA8888, AlphaType::Unpremul, None);
    let data = Data::new_copy(&bytes);
    #[allow(clippy::cast_sign_loss)]
    let row_bytes = (w as usize) * 4;
    raster_from_data(&info, data, row_bytes)
}

fn pixel_dimension(world: f32, scale: f32) -> u32 {
    let p = (world * scale).round();
    if !p.is_finite() || p <= 0.0 {
        return 0;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let px = p as u32;
    px
}

const EMPTY_RECT: Rect = Rect {
    min_x: 0.0,
    min_y: 0.0,
    max_x: 0.0,
    max_y: 0.0,
};
