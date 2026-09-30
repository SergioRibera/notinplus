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

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use freya_canvas_bg::{
    AttributionMode, BgPaintCtx, CanvasBackground, Color, PageAttachment, PageId, PageLayout, Rect,
};
use freya_engine::prelude::{
    AlphaType, ColorType, Data, FilterMode, Image, ImageInfo, MipmapMode, Paint, Rect as SkRect,
    SamplingOptions, raster_from_data,
};
use skia_safe::canvas::SrcRectConstraint;

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
/// Sampling for the `draw_image_rect` calls that blit cached tiles.
/// The default `SamplingOptions` is `Nearest` filter with no mipmap —
/// visibly pixelated whenever the viewport zoom lands between the
/// bucket's native rasterisation and the target dst rect (which is
/// most of the time). `Linear` costs a hair of GPU work per pixel and
/// makes text at intermediate zooms look like a rendered PDF instead
/// of a screenshot of one. Mipmapping stays off — we already ship
/// per-bucket LODs, so a mip chain would double-sample the same data.
const TILE_SAMPLING: SamplingOptions = SamplingOptions {
    max_aniso: 0,
    use_cubic: false,
    cubic: freya_engine::prelude::CubicResampler { b: 0.0, c: 0.0 },
    filter: FilterMode::Linear,
    mipmap: MipmapMode::None,
};
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
    /// Per-page thumbnail (lowest-bucket `TileCoord::Full` render).
    /// Lives outside the LRU so a placeholder is always available on
    /// zoom transitions even when the cache has churned through many
    /// higher-bucket tiles. Populated opportunistically when a worker
    /// completes a bucket 0 Full render for the page.
    thumbs: Mutex<HashMap<PageId, Arc<CachedTile>>>,
    /// Tiles pinned for the duration of the current paint frame. The
    /// zero-copy `Data::new_bytes` path hands Skia a raw pointer into
    /// each tile's `Arc<Vec<u8>>`; those pointers stay valid as long
    /// as we hold an owning `Arc` here. Cleared at the start of every
    /// paint so previous-frame pins drop after Skia has flushed the
    /// frame that referenced them.
    frame_pins: Mutex<Vec<Arc<CachedTile>>>,
    /// Bits of the effective render scale (viewport × device pixel
    /// ratio) captured on the last `paint` call. Read by `tick` so
    /// prefetch queues match the resolution paint just requested.
    last_effective_scale_bits: AtomicU32,
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
        Self {
            doc,
            gap,
            backdrop,
            pages,
            bounds,
            cache,
            pool,
            thumbs: Mutex::new(HashMap::new()),
            frame_pins: Mutex::new(Vec::new()),
            last_effective_scale_bits: AtomicU32::new(1.0_f32.to_bits()),
        }
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

        // Drop last frame's pinned tiles now that Skia has flushed the
        // frame that referenced them, then reserve for this frame's.
        lock(&self.frame_pins).clear();

        // Bucket picks based on the *effective* device-pixel scale, not
        // the viewport zoom in isolation. `total_matrix().scale_x()`
        // folds in freya's DPI transform, so bucket 0 no longer means
        // "render at 1 pt = 1 physical pixel" on hi-DPI screens.
        let effective_scale = canvas_scale_x(cx.canvas).max(cx.scale);
        self.last_effective_scale_bits
            .store(effective_scale.to_bits(), Ordering::Relaxed);
        let bucket = bucket_for(effective_scale);
        let image_paint = Paint::default();

        for pl in &self.pages {
            if !pl.rect.intersects(&cx.visible) {
                continue;
            }
            draw_page_fill(cx.canvas, pl.rect);
            // Kick off the persistent thumbnail render if we don't have
            // one yet. Bucket 0 Full tiles cost pennies and give every
            // higher-bucket miss a guaranteed placeholder — that's the
            // fix for "screen goes white on zoom until you wiggle it".
            if !self.has_thumb(pl.id) {
                self.pool.request(CacheKey {
                    page: pl.id,
                    bucket: 0,
                    tile: TileCoord::Full,
                });
            }
            match TileGrid::for_page(pl.natural_size, bucket) {
                None => {
                    // Small page: single full-bitmap render.
                    self.draw_or_request(
                        cx,
                        &image_paint,
                        CacheKey {
                            page: pl.id,
                            bucket,
                            tile: TileCoord::Full,
                        },
                        pl.rect,
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
                            self.draw_or_request(cx, &image_paint, key, tile_rect, pl.rect);
                        }
                    }
                }
            }
            // Harvest the persistent bucket-0 Full tile the moment it
            // lands. `draw_or_request` only sees it when the current
            // bucket also is 0 (small-page path); for larger buckets
            // the tile arrives via the explicit `pool.request` above
            // and would otherwise sit in the LRU until eviction.
            if !self.has_thumb(pl.id) {
                if let Some(thumb) = self.cache.peek(CacheKey {
                    page: pl.id,
                    bucket: 0,
                    tile: TileCoord::Full,
                }) {
                    self.remember_thumb(pl.id, &thumb);
                }
            }
        }
    }

    fn tick(&self, visible: Rect, _scale: f32) {
        // Use paint's captured effective scale so prefetch buckets
        // match the visible resolution. Falls back to 1.0 before the
        // first paint call has landed.
        let scale = f32::from_bits(self.last_effective_scale_bits.load(Ordering::Relaxed));
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
        page_rect: Rect,
    ) {
        let dst = SkRect::from_ltrb(
            dst_world.min_x,
            dst_world.min_y,
            dst_world.max_x,
            dst_world.max_y,
        );
        if let Some(tile) = self.cache.get(key) {
            if key.bucket == 0 && matches!(key.tile, TileCoord::Full) {
                self.remember_thumb(key.page, &tile);
            }
            if let Some(img) = self.tile_to_image_pinned(&tile) {
                cx.canvas.draw_image_rect_with_sampling_options(
                    &img,
                    None,
                    dst,
                    TILE_SAMPLING,
                    image_paint,
                );
            }
            return;
        }
        // Miss: prefer the highest cached full-page render below the
        // target bucket, then fall back to the persistent thumbnail
        // (bucket 0) so a placeholder is always available even after
        // heavy LRU churn. For a `TileCoord::Cell`, we sample only the
        // corresponding sub-rect out of the placeholder so the sub-tile
        // paints the *matching* region rather than a stretched copy of
        // the whole page.
        let placeholder = self
            .cache
            .nearest_full_at_or_below(key.page, key.bucket - 1)
            .or_else(|| self.thumb_for(key.page));
        if let Some(placeholder) = placeholder {
            if let Some(img) = self.tile_to_image_pinned(&placeholder) {
                let src = placeholder_src_rect(page_rect, dst_world, &placeholder);
                let src_pair = src.as_ref().map(|r| (r, SrcRectConstraint::Fast));
                cx.canvas.draw_image_rect_with_sampling_options(
                    &img,
                    src_pair,
                    dst,
                    TILE_SAMPLING,
                    image_paint,
                );
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

    /// Wraps `tile` as a Skia `Image` without copying and pins the
    /// owning `Arc<CachedTile>` for the current paint frame. See
    /// [`Self::frame_pins`] for the lifetime contract.
    fn tile_to_image_pinned(&self, tile: &Arc<CachedTile>) -> Option<Image> {
        let img = tile_to_image_zero_copy(tile)?;
        lock(&self.frame_pins).push(Arc::clone(tile));
        Some(img)
    }

    fn has_thumb(&self, page: PageId) -> bool {
        lock(&self.thumbs).contains_key(&page)
    }

    fn thumb_for(&self, page: PageId) -> Option<Arc<CachedTile>> {
        lock(&self.thumbs).get(&page).cloned()
    }

    fn remember_thumb(&self, page: PageId, tile: &Arc<CachedTile>) {
        let mut thumbs = lock(&self.thumbs);
        thumbs.entry(page).or_insert_with(|| Arc::clone(tile));
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn canvas_scale_x(canvas: &freya_engine::prelude::Canvas) -> f32 {
    let m = canvas.local_to_device_as_3x3();
    let sx = m.scale_x().abs();
    let sy = m.scale_y().abs();
    if sx.is_finite() && sy.is_finite() && sx > 0.0 && sy > 0.0 {
        sx.max(sy)
    } else {
        1.0
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
    let bounds = Rect {
        min_x,
        min_y: 0.0,
        max_x,
        max_y: bottom,
    };
    (pages, bounds)
}

/// Sub-rect of the placeholder bitmap covering `dst_world`'s slice of
/// `page_rect`. Returns `None` when `dst_world` matches `page_rect` —
/// the caller should paint the whole placeholder in that case, which
/// is both cheaper and avoids sub-pixel bleeding on the seams.
///
/// The mapping is a straightforward proportional scale: the fraction
/// of the page covered by `dst_world` on each axis is multiplied by
/// the placeholder's pixel dimensions. Edge sub-tiles land on
/// fractional pixels; Skia's sampler handles that fine given
/// [`SrcRectConstraint::Fast`].
fn placeholder_src_rect(
    page_rect: Rect,
    dst_world: Rect,
    placeholder: &CachedTile,
) -> Option<SkRect> {
    let page_w = page_rect.width();
    let page_h = page_rect.height();
    if page_w <= 0.0 || page_h <= 0.0 {
        return None;
    }
    if dst_world.min_x <= page_rect.min_x
        && dst_world.min_y <= page_rect.min_y
        && dst_world.max_x >= page_rect.max_x
        && dst_world.max_y >= page_rect.max_y
    {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let (pw, ph) = (placeholder.width as f32, placeholder.height as f32);
    let sx0 = (dst_world.min_x - page_rect.min_x) / page_w * pw;
    let sy0 = (dst_world.min_y - page_rect.min_y) / page_h * ph;
    let sx1 = (dst_world.max_x - page_rect.min_x) / page_w * pw;
    let sy1 = (dst_world.max_y - page_rect.min_y) / page_h * ph;
    Some(SkRect::from_ltrb(
        sx0.max(0.0),
        sy0.max(0.0),
        sx1.min(pw),
        sy1.min(ph),
    ))
}

/// Zero-copy wrapper around `tile`'s RGBA buffer as a Skia [`Image`].
///
/// # Safety contract
///
/// The returned `Image` holds an `SkData` referencing the tile's
/// `Arc<Vec<u8>>` without copying it. The caller MUST keep the owning
/// `Arc<CachedTile>` alive for the entire lifetime of every Skia
/// command that reads the image — Skia flushes deferred draw ops
/// **after** [`CanvasBackground::paint`] returns, so a locally-scoped
/// `Arc` is not enough. [`PdfBackground::tile_to_image_pinned`] is the
/// only sound caller: it pushes an `Arc<CachedTile>` clone into
/// [`PdfBackground::frame_pins`], which drops the pin at the start of
/// the next paint (well after this frame's flush has happened).
fn tile_to_image_zero_copy(tile: &CachedTile) -> Option<Image> {
    #[allow(clippy::cast_possible_wrap)]
    let (w, h) = (tile.width as i32, tile.height as i32);
    let info = ImageInfo::new((w, h), ColorType::RGBA8888, AlphaType::Unpremul, None);
    let row_bytes = tile.width as usize * 4;
    // SAFETY: pin contract — see fn-level doc. `tile.bytes` is an
    // `Arc<Vec<u8>>`; `Data::new_bytes` produces an `SkData` that
    // references the slice without copying it, and the returned
    // `SkImage`'s raster backend reads through that pointer during
    // every subsequent `Canvas::draw_image_rect` call. The pin held
    // in `PdfBackground::frame_pins` keeps the `Arc` alive across the
    // frame flush that consumes those draw commands.
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

#[cfg(test)]
mod tests {
    use super::{CachedTile, Rect, placeholder_src_rect};
    use std::sync::Arc;

    fn tile(w: u32, h: u32) -> CachedTile {
        CachedTile {
            bytes: Arc::new(Vec::new()),
            width: w,
            height: h,
        }
    }

    #[test]
    fn placeholder_src_rect_none_for_full_page_dst() {
        let page = Rect {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 100.0,
            max_y: 200.0,
        };
        assert!(placeholder_src_rect(page, page, &tile(64, 128)).is_none());
    }

    #[test]
    fn placeholder_src_rect_top_left_quadrant() {
        let page = Rect {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 100.0,
            max_y: 200.0,
        };
        let quad = Rect {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 50.0,
            max_y: 100.0,
        };
        let src = placeholder_src_rect(page, quad, &tile(64, 128)).expect("sub-rect");
        assert!((src.left - 0.0).abs() < 1e-4);
        assert!((src.top - 0.0).abs() < 1e-4);
        assert!((src.right - 32.0).abs() < 1e-4);
        assert!((src.bottom - 64.0).abs() < 1e-4);
    }

    #[test]
    fn placeholder_src_rect_bottom_right_cell_with_translated_page() {
        // Page origin at (-50, 300) — mirrors the layout produced by
        // `compute_layout` for a page that stacks below its neighbour.
        let page = Rect {
            min_x: -50.0,
            min_y: 300.0,
            max_x: 50.0,
            max_y: 500.0,
        };
        let cell = Rect {
            min_x: 0.0,
            min_y: 400.0,
            max_x: 50.0,
            max_y: 500.0,
        };
        let src = placeholder_src_rect(page, cell, &tile(64, 128)).expect("sub-rect");
        assert!((src.left - 32.0).abs() < 1e-4);
        assert!((src.top - 64.0).abs() < 1e-4);
        assert!((src.right - 64.0).abs() < 1e-4);
        assert!((src.bottom - 128.0).abs() < 1e-4);
    }

    #[test]
    fn placeholder_src_rect_none_when_page_degenerate() {
        let page = Rect {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 0.0,
            max_y: 0.0,
        };
        let dst = Rect {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 10.0,
            max_y: 10.0,
        };
        assert!(placeholder_src_rect(page, dst, &tile(64, 64)).is_none());
    }
}
