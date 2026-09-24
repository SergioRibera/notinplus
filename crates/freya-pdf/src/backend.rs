//! [`CanvasBackground`] impl backed by [`PdfDocument`].
//!
//! M4 pipeline:
//! - Owns an [`Arc<RenderPool>`] driving pdfium off the UI thread.
//! - Paint pass looks up cached tiles in [`Cache`] keyed by
//!   `(page, zoom bucket)`.
//! - Cache miss → queue a request AND draw a lower-bucket tile
//!   upscaled as a placeholder (Preview.app / Kindle feel).
//! - `tick` prefetches a small buffer of pages above/below the
//!   viewport and cancels pending requests for pages that scrolled
//!   out of range.

use std::collections::HashSet;
use std::sync::Arc;

use freya_canvas_bg::{
    AttributionMode, BgPaintCtx, CanvasBackground, Color, PageAttachment, PageId, PageLayout,
    Rect,
};
use freya_engine::prelude::{
    AlphaType, ColorType, Data, Image, ImageInfo, Paint, Rect as SkRect, raster_from_data,
};

use crate::cache::{Cache, CachedTile};
use crate::doc::PdfDocument;
use crate::render::RenderPool;
use crate::tiles::{CacheKey, bucket_for};

/// Default vertical gap between pages, in world units (= PDF points).
const DEFAULT_GAP: f32 = 12.0;
/// Default backdrop — neutral gray matching common PDF viewers.
const DEFAULT_BACKDROP: Color = Color::from_rgb(90, 90, 96);
/// Fill drawn under a page whose bitmap has not landed yet — matches
/// blank paper so the layout stays legible during first render.
const DEFAULT_PAGE_FILL: Color = Color::from_rgb(245, 245, 245);
/// Pages above / below the viewport we speculatively enqueue on tick.
const PREFETCH_BUFFER: usize = 3;
/// Cache capacity — small enough for mobile, big enough for a few
/// zoom levels of a ~50-page doc.
const CACHE_CAPACITY: usize = 96;
/// Number of worker threads. Two is enough — pdfium serialises heavy
/// operations behind an internal mutex so extra threads mostly buy
/// prefetch parallelism.
const DEFAULT_WORKERS: usize = 2;

/// PDF-backed canvas background. Owns the render pool, tile cache,
/// and precomputed layout.
#[derive(Debug)]
pub struct PdfBackground {
    doc: PdfDocument,
    gap: f32,
    backdrop: Color,
    pages: Vec<PageLayout>,
    bounds: Rect,
    cache: Arc<Cache>,
    pool: Arc<RenderPool>,
}

impl PdfBackground {
    /// Wrap a document with default settings (12pt gap, neutral gray
    /// backdrop, 2 worker threads, 96-tile cache).
    #[must_use]
    pub fn new(doc: PdfDocument) -> Self {
        Self::with_settings(doc, DEFAULT_GAP, DEFAULT_BACKDROP)
    }

    /// Wrap a document with an explicit gap between pages and backdrop
    /// color.
    #[must_use]
    pub fn with_settings(doc: PdfDocument, gap: f32, backdrop: Color) -> Self {
        let (pages, bounds) = compute_layout(&doc, gap);
        let cache = Arc::new(Cache::new(CACHE_CAPACITY));
        let pool = RenderPool::new(&doc, &cache, DEFAULT_WORKERS);
        Self { doc, gap, backdrop, pages, bounds, cache, pool }
    }

    /// Underlying document — text extraction, page counts, etc.
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
        // Route worker wake-ups through the freya redraw notifier.
        // Called every frame; cheap since the pool just swaps an
        // internal `Arc<Mutex<RedrawHandle>>`.
        self.pool.set_redraw(cx.redraw.clone());

        let bucket = bucket_for(cx.scale);
        let image_paint = Paint::default();

        for pl in &self.pages {
            if !pl.rect.intersects(&cx.visible) {
                continue;
            }
            let dst = SkRect::from_ltrb(pl.rect.min_x, pl.rect.min_y, pl.rect.max_x, pl.rect.max_y);
            let key = CacheKey { page: pl.id, bucket };
            if let Some(tile) = self.cache.get(key) {
                if let Some(img) = tile_to_image(&tile) {
                    cx.canvas.draw_image_rect(&img, None, dst, &image_paint);
                }
                continue;
            }
            // Miss: paint blank page + upscale a nearby lower-bucket
            // tile (if any) while the target bucket renders.
            draw_page_fill(cx.canvas, dst);
            if let Some(placeholder) = self.cache.nearest_at_or_below(pl.id, bucket - 1) {
                if let Some(img) = tile_to_image(&placeholder) {
                    cx.canvas.draw_image_rect(&img, None, dst, &image_paint);
                }
            }
            let target_w = pixel_dimension(pl.rect.width(), cx.scale);
            let target_h = pixel_dimension(pl.rect.height(), cx.scale);
            self.pool.request(key, target_w, target_h);
        }
    }

    fn tick(&self, visible: Rect, scale: f32) {
        let bucket = bucket_for(scale);
        // Range of pages intersecting the viewport, expanded by
        // PREFETCH_BUFFER on each side so scrolling doesn't stutter.
        let visible_indices: Vec<usize> = self
            .pages
            .iter()
            .enumerate()
            .filter_map(|(i, pl)| pl.rect.intersects(&visible).then_some(i))
            .collect();
        let Some(&first_vis) = visible_indices.first() else {
            // Nothing on screen — cancel every pending request; no
            // point rendering pages the user isn't looking at.
            self.pool.cancel_outside(&HashSet::new());
            return;
        };
        let last_vis = *visible_indices.last().unwrap_or(&first_vis);
        let start = first_vis.saturating_sub(PREFETCH_BUFFER);
        let end = (last_vis + PREFETCH_BUFFER + 1).min(self.pages.len());

        let mut keep: HashSet<PageId> = HashSet::with_capacity(end - start);
        for i in start..end {
            let pl = &self.pages[i];
            keep.insert(pl.id);
            let key = CacheKey { page: pl.id, bucket };
            if self.cache.peek(key).is_some() {
                continue;
            }
            let target_w = pixel_dimension(pl.rect.width(), scale);
            let target_h = pixel_dimension(pl.rect.height(), scale);
            self.pool.request(key, target_w, target_h);
        }
        self.pool.cancel_outside(&keep);
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

fn tile_to_image(tile: &CachedTile) -> Option<Image> {
    #[allow(clippy::cast_possible_wrap)]
    let (w, h) = (tile.width as i32, tile.height as i32);
    let info = ImageInfo::new((w, h), ColorType::RGBA8888, AlphaType::Unpremul, None);
    // `Data::new_copy` allocates + memcpys; acceptable while we
    // convert once per visible page per frame. Zero-copy via
    // `Data::new_bytes` requires proving the `Arc<Vec<u8>>` outlives
    // the returned `Image`; deferred until a profile shows the
    // memcpy dominates.
    let data = Data::new_copy(&tile.bytes);
    let row_bytes = tile.width as usize * 4;
    raster_from_data(&info, data, row_bytes)
}

fn draw_page_fill(canvas: &freya_engine::prelude::Canvas, dst: SkRect) {
    use freya_engine::prelude::PaintStyle;
    let mut fill = Paint::default();
    fill.set_color(DEFAULT_PAGE_FILL)
        .set_style(PaintStyle::Fill)
        .set_anti_alias(true);
    canvas.draw_rect(dst, &fill);
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
