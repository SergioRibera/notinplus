//! [`CanvasBackground`] impl backed by [`PdfDocument`].
//!
//! M5 pipeline:
//! - Owns an [`Arc<RenderPool>`] driving pdfium off the UI thread.
//! - Paint pass looks up cached tiles in [`Cache`] keyed by
//!   `(page, zoom bucket, tile coord)`.
//! - Small pages (below the [`TILE_THRESHOLD`] on both axes at the
//!   current bucket) render as a single [`TileCoord::Full`] entry.
//! - Larger pages render as a grid of [`TileCoord::Cell`] tiles; only
//!   the visible cells are enqueued so scrolling / zooming touches
//!   bounded memory even for very large PDFs.
//! - Cache miss → queue a request AND draw an upscaled lower-bucket
//!   full-page tile as a placeholder (Preview.app / Kindle feel).
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
use crate::tiles::{CacheKey, TileCoord, TileGrid, bucket_for};

/// Default vertical gap between pages, in world units (= PDF points).
const DEFAULT_GAP: f32 = 12.0;
/// Default backdrop — neutral gray matching common PDF viewers.
const DEFAULT_BACKDROP: Color = Color::from_rgb(90, 90, 96);
/// Fill drawn under a page whose bitmap has not landed yet — matches
/// blank paper so the layout stays legible during first render.
const DEFAULT_PAGE_FILL: Color = Color::from_rgb(245, 245, 245);
/// Pages above / below the viewport we speculatively enqueue on tick.
const PREFETCH_BUFFER: usize = 3;
/// Cache capacity. Bumped from M4 to accommodate multi-tile pages —
/// a tiled page at bucket 3 can easily contribute a dozen entries.
const CACHE_CAPACITY: usize = 256;
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
    /// backdrop, 2 worker threads, 256-tile cache).
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

    /// World-space bounding rects for a search hit — the same rects
    /// returned by [`crate::PdfSearchIndex::hit_rects`] translated by
    /// the hit page's layout offset. The result feeds directly into a
    /// paint overlay drawn on top of the tile.
    ///
    /// # Errors
    ///
    /// Returns [`crate::PdfError::PageOutOfRange`] when the hit
    /// references a page missing from this backend's layout;
    /// propagates any pdfium failure surfaced by
    /// [`crate::PdfSearchIndex::hit_rects`].
    pub fn world_hit_rects(
        &self,
        hit: crate::search::SearchHit,
        index: &crate::search::PdfSearchIndex,
    ) -> Result<Vec<Rect>, crate::error::PdfError> {
        let page = self
            .pages
            .iter()
            .find(|p| p.id == hit.page)
            .ok_or_else(|| {
                let requested = usize::try_from(hit.page.0).unwrap_or(usize::MAX);
                crate::error::PdfError::PageOutOfRange {
                    requested,
                    page_count: self.pages.len(),
                }
            })?;
        let (ox, oy) = (page.rect.min_x, page.rect.min_y);
        let rects = index.hit_rects(hit)?;
        Ok(rects
            .into_iter()
            .map(|r| Rect {
                min_x: r.min_x + ox,
                min_y: r.min_y + oy,
                max_x: r.max_x + ox,
                max_y: r.max_y + oy,
            })
            .collect())
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
            draw_page_fill(cx.canvas, pl.rect);
            match TileGrid::for_page(pl.natural_size, bucket) {
                None => {
                    // Small page: single full-bitmap render.
                    self.draw_or_request(
                        cx,
                        &image_paint,
                        CacheKey { page: pl.id, bucket, tile: TileCoord::Full },
                        pl.rect,
                    );
                }
                Some(grid) => {
                    // Large page: iterate visible tiles only.
                    for row in 0..grid.rows {
                        for col in 0..grid.cols {
                            let tile_rect = grid.tile_world_rect(pl.rect, col, row);
                            if !tile_rect.intersects(&cx.visible) {
                                continue;
                            }
                            let key = CacheKey {
                                page: pl.id,
                                bucket,
                                tile: TileCoord::Cell { x: col, y: row },
                            };
                            self.draw_or_request(cx, &image_paint, key, tile_rect);
                        }
                    }
                }
            }
        }
    }

    fn tick(&self, visible: Rect, scale: f32) {
        let bucket = bucket_for(scale);
        let visible_indices: Vec<usize> = self
            .pages
            .iter()
            .enumerate()
            .filter_map(|(i, pl)| pl.rect.intersects(&visible).then_some(i))
            .collect();
        let Some(&first_vis) = visible_indices.first() else {
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
            match TileGrid::for_page(pl.natural_size, bucket) {
                None => {
                    self.enqueue_missing(CacheKey {
                        page: pl.id,
                        bucket,
                        tile: TileCoord::Full,
                    });
                }
                Some(grid) => {
                    // Prefetch every tile of every page in the buffer
                    // range — sub-tile visibility filtering happens at
                    // paint time; here we're speculatively warming the
                    // cache for likely-imminent scrolls.
                    for row in 0..grid.rows {
                        for col in 0..grid.cols {
                            self.enqueue_missing(CacheKey {
                                page: pl.id,
                                bucket,
                                tile: TileCoord::Cell { x: col, y: row },
                            });
                        }
                    }
                    let _ = visible;
                }
            }
        }
        self.pool.cancel_outside(&keep);
    }
}

impl PdfBackground {
    fn draw_or_request(
        &self,
        cx: &BgPaintCtx<'_>,
        image_paint: &Paint,
        key: CacheKey,
        dst_world: Rect,
    ) {
        let dst = SkRect::from_ltrb(
            dst_world.min_x,
            dst_world.min_y,
            dst_world.max_x,
            dst_world.max_y,
        );
        if let Some(tile) = self.cache.get(key) {
            if let Some(img) = tile_to_image(&tile) {
                cx.canvas.draw_image_rect(&img, None, dst, image_paint);
            }
            return;
        }
        // Miss: upscale the best lower-bucket full-page render if we
        // have one, then queue the target-bucket render. Placeholder
        // uses the full-page tile so a single lookup covers every
        // sub-tile of the same page. When the current key is a tile
        // (not a full render), the placeholder gets stretched across
        // the sub-tile — visually cheap and better than blank fill;
        // proper sub-region src sampling is a follow-up once the
        // layout carries page rects to `nearest_full_at_or_below`.
        let placeholder_key_bucket = key.bucket - 1;
        if let Some(placeholder) =
            self.cache.nearest_full_at_or_below(key.page, placeholder_key_bucket)
        {
            if let Some(img) = tile_to_image(&placeholder) {
                cx.canvas.draw_image_rect(&img, None, dst, image_paint);
            }
        }
        self.pool.request(key);
    }

    fn enqueue_missing(&self, key: CacheKey) {
        if self.cache.peek(key).is_some() {
            return;
        }
        self.pool.request(key);
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

/// Wraps `tile`'s RGBA buffer as a Skia [`Image`] without copying.
///
/// # Safety invariant
///
/// The caller MUST keep `tile` (and therefore the underlying
/// `Arc<Vec<u8>>`) alive for the entire draw call that consumes the
/// returned `Image`. Every call site in this module holds an
/// `Arc<CachedTile>` on the stack across `Canvas::draw_image_rect`, so
/// the buffer stays live until the paint pipeline is done reading it.
fn tile_to_image(tile: &CachedTile) -> Option<Image> {
    #[allow(clippy::cast_possible_wrap)]
    let (w, h) = (tile.width as i32, tile.height as i32);
    let info = ImageInfo::new((w, h), ColorType::RGBA8888, AlphaType::Unpremul, None);
    let row_bytes = tile.width as usize * 4;
    // SAFETY: `tile.bytes` is an `Arc<Vec<u8>>` owned by the caller
    // (see fn-level invariant). `Data::new_bytes` produces an
    // `SkData` that references the slice without copying; the
    // `SkImage` returned by `raster_from_data` keeps that `SkData`
    // refcounted and its raster backend reads from it synchronously
    // during `Canvas::draw_image_rect`. Both the image and the data
    // drop at end-of-frame, well before the caller releases its
    // `Arc<CachedTile>`.
    #[allow(unsafe_code)]
    let data = unsafe { Data::new_bytes(&tile.bytes) };
    raster_from_data(&info, data, row_bytes)
}

fn draw_page_fill(canvas: &freya_engine::prelude::Canvas, rect: Rect) {
    use freya_engine::prelude::PaintStyle;
    let dst = SkRect::from_ltrb(rect.min_x, rect.min_y, rect.max_x, rect.max_y);
    let mut fill = Paint::default();
    fill.set_color(DEFAULT_PAGE_FILL)
        .set_style(PaintStyle::Fill)
        .set_anti_alias(true);
    canvas.draw_rect(dst, &fill);
}

const EMPTY_RECT: Rect = Rect {
    min_x: 0.0,
    min_y: 0.0,
    max_x: 0.0,
    max_y: 0.0,
};
