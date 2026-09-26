//! Zoom bucketing.
//!
//! The render cache is keyed by a discrete `bucket` per page instead
//! of the raw viewport scale. Bucketing at powers of `1.5` keeps cache
//! hits high across small zoom deltas (a pinch-zoom that moves scale
//! from 1.0 to 1.2 stays in the same bucket) while still tracking
//! large zooms.

use freya_canvas_bg::{PageId, Rect};

/// Geometric ratio between adjacent zoom buckets.
const BUCKET_BASE: f32 = 1.5;

/// Edge length of a tile, in pixels. Pages whose rasterised dimensions
/// exceed [`TILE_THRESHOLD`] on either axis are sliced into a grid of
/// this size so a single tile stays under ~1 MB (512×512×4 bytes).
pub const TILE_PIXELS: u32 = 512;
/// Above this rasterised dimension (px), a page switches from single-
/// tile rendering to a grid. Below it the page is cheap enough to
/// render as one bitmap.
pub const TILE_THRESHOLD: u32 = 1_024;

/// Lower bucket bound. `1.5^-4 ≈ 0.2` — anything below that is a
/// glorified page thumbnail and the cache would waste entries on
/// per-tile granularity that no one can read.
pub const MIN_BUCKET: i32 = -4;
/// Upper bucket bound. `1.5^5 ≈ 7.6` — capping here keeps a US-Letter
/// page (612×792pt) under `4650×6000` px = ~110 MB when rasterised
/// tile-by-tile. Higher zoom levels reuse this bucket's tiles via
/// Skia's linear upscale, which trades a hint of blur for stable
/// memory and no pdfium bitmap-alloc explosion.
pub const MAX_BUCKET: i32 = 5;

/// Quantize a raw viewport scale into a discrete bucket index. Bucket
/// `n` covers scales in `[1.5^(n-0.5), 1.5^(n+0.5))`, clamped to
/// [`MIN_BUCKET`]..=[`MAX_BUCKET`].
#[must_use]
pub fn bucket_for(scale: f32) -> i32 {
    if !scale.is_finite() || scale <= 0.0 {
        return 0;
    }
    #[allow(clippy::cast_possible_truncation)]
    let b = scale.log(BUCKET_BASE).round() as i32;
    b.clamp(MIN_BUCKET, MAX_BUCKET)
}

/// Effective scale represented by a given bucket. Used to size the
/// bitmap we ask pdfium for so cached tiles all live on the same
/// zoom quantum.
#[must_use]
pub fn bucket_scale(bucket: i32) -> f32 {
    BUCKET_BASE.powi(bucket)
}

/// Which slice of a page a cache entry covers.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum TileCoord {
    /// The whole page fits in one bitmap at this bucket — no tiling.
    Full,
    /// A `TILE_PIXELS`-sized slice at grid position `(x, y)`. Edge
    /// tiles may be smaller than `TILE_PIXELS` when the page's
    /// rasterised size doesn't divide evenly.
    Cell {
        /// Column index in the tile grid, `[0, cols)`.
        x: u16,
        /// Row index in the tile grid, `[0, rows)`.
        y: u16,
    },
}

/// Cache lookup key. `page` selects the pdf page; `bucket` picks the
/// LOD level; `tile` selects the sub-slice.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct CacheKey {
    /// Which page the tile came from.
    pub page: PageId,
    /// LOD level, produced by [`bucket_for`].
    pub bucket: i32,
    /// Which slice of the page.
    pub tile: TileCoord,
}

/// Grid layout for a tiled page at a given bucket.
#[derive(Debug, Clone, Copy)]
pub struct TileGrid {
    /// Number of columns.
    pub cols: u16,
    /// Number of rows.
    pub rows: u16,
    /// Full rasterised page width in pixels at this bucket.
    pub full_width_pixels: u32,
    /// Full rasterised page height in pixels at this bucket.
    pub full_height_pixels: u32,
}

impl TileGrid {
    /// Compute the tile grid for `page_world_size` at `bucket`.
    /// Returns `None` when the page fits under [`TILE_THRESHOLD`] on
    /// both axes — callers use [`TileCoord::Full`] instead.
    #[must_use]
    pub fn for_page(page_world_size: (f32, f32), bucket: i32) -> Option<Self> {
        let scale = bucket_scale(bucket);
        let full_w = ceil_pos(page_world_size.0 * scale)?;
        let full_h = ceil_pos(page_world_size.1 * scale)?;
        if full_w <= TILE_THRESHOLD && full_h <= TILE_THRESHOLD {
            return None;
        }
        let cols = u16::try_from(full_w.div_ceil(TILE_PIXELS)).ok()?;
        let rows = u16::try_from(full_h.div_ceil(TILE_PIXELS)).ok()?;
        Some(Self {
            cols,
            rows,
            full_width_pixels: full_w,
            full_height_pixels: full_h,
        })
    }

    /// Bitmap size in pixels for tile `(col, row)`. Edge tiles may be
    /// smaller than [`TILE_PIXELS`] when the grid doesn't divide
    /// evenly.
    #[must_use]
    pub fn tile_pixel_size(&self, col: u16, row: u16) -> (u32, u32) {
        let x0 = u32::from(col) * TILE_PIXELS;
        let y0 = u32::from(row) * TILE_PIXELS;
        let w = self.full_width_pixels.saturating_sub(x0).min(TILE_PIXELS);
        let h = self.full_height_pixels.saturating_sub(y0).min(TILE_PIXELS);
        (w, h)
    }

    /// World-space rectangle covered by tile `(col, row)` inside
    /// `page_rect`.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // pixel counts never approach f32 mantissa limit
    pub fn tile_world_rect(&self, page_rect: Rect, col: u16, row: u16) -> Rect {
        let world_per_pixel_x = page_rect.width() / self.full_width_pixels as f32;
        let world_per_pixel_y = page_rect.height() / self.full_height_pixels as f32;
        let (pix_w, pix_h) = self.tile_pixel_size(col, row);
        let x0 = (f32::from(col) * TILE_PIXELS as f32).mul_add(world_per_pixel_x, page_rect.min_x);
        let y0 = (f32::from(row) * TILE_PIXELS as f32).mul_add(world_per_pixel_y, page_rect.min_y);
        Rect {
            min_x: x0,
            min_y: y0,
            max_x: (pix_w as f32).mul_add(world_per_pixel_x, x0),
            max_y: (pix_h as f32).mul_add(world_per_pixel_y, y0),
        }
    }
}

fn ceil_pos(v: f32) -> Option<u32> {
    if !v.is_finite() || v <= 0.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let n = v.ceil() as u32;
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_stable_around_center() {
        // Any scale near 1.0 lands in bucket 0.
        assert_eq!(bucket_for(1.0), 0);
        assert_eq!(bucket_for(1.1), 0);
        assert_eq!(bucket_for(0.9), 0);
    }

    #[test]
    fn bucket_shifts_at_geometric_boundary() {
        // 1.5 lands in bucket 1 (log_1.5(1.5) = 1).
        assert_eq!(bucket_for(1.5), 1);
        // ~2.25 lands in bucket 2.
        assert_eq!(bucket_for(2.25), 2);
        // 0.5 → negative bucket.
        assert!(bucket_for(0.5) < 0);
    }

    #[test]
    fn bucket_scale_roundtrips() {
        for b in -3..=3 {
            let s = bucket_scale(b);
            assert_eq!(bucket_for(s), b, "bucket {b} scale {s}");
        }
    }

    #[test]
    fn bucket_ignores_non_positive_scale() {
        assert_eq!(bucket_for(0.0), 0);
        assert_eq!(bucket_for(-1.0), 0);
        assert_eq!(bucket_for(f32::NAN), 0);
    }

    #[test]
    fn bucket_clamped_to_max_at_extreme_zoom() {
        // 100× zoom would suggest bucket 12; clamp holds at MAX_BUCKET
        // so pdfium never receives a rasterisation request whose bitmap
        // would exceed the alloc ceiling.
        assert_eq!(bucket_for(100.0), MAX_BUCKET);
        assert_eq!(bucket_for(f32::MAX), MAX_BUCKET);
    }

    #[test]
    fn bucket_clamped_to_min_at_extreme_zoom_out() {
        assert_eq!(bucket_for(0.001), MIN_BUCKET);
    }

    #[test]
    fn tile_grid_none_below_threshold() {
        // A4 (595×842) at bucket 0 (scale 1) = 595×842 pixels, both
        // below 1024 → no tiling.
        assert!(TileGrid::for_page((595.0, 842.0), 0).is_none());
    }

    #[test]
    fn tile_grid_some_above_threshold() {
        // A4 at bucket 2 (scale ~2.25) = ~1339×1895 → tiles.
        let g = TileGrid::for_page((595.0, 842.0), 2).expect("above threshold");
        assert_eq!(g.cols, 3, "1339 / 512 = 3 cols");
        assert_eq!(g.rows, 4, "1895 / 512 = 4 rows");
    }

    #[test]
    fn edge_tile_smaller_than_tile_pixels() {
        let g = TileGrid { cols: 3, rows: 4, full_width_pixels: 1339, full_height_pixels: 1895 };
        // Top-left interior tile = full 512×512.
        assert_eq!(g.tile_pixel_size(0, 0), (512, 512));
        // Rightmost column at col 2: 1339 - 2*512 = 315.
        assert_eq!(g.tile_pixel_size(2, 0).0, 315);
        // Bottom row at row 3: 1895 - 3*512 = 359.
        assert_eq!(g.tile_pixel_size(0, 3).1, 359);
    }

    #[test]
    fn tile_world_rect_partitions_page() {
        use freya_canvas_bg::Rect;
        let g = TileGrid { cols: 3, rows: 4, full_width_pixels: 1339, full_height_pixels: 1895 };
        let page = Rect { min_x: -297.5, min_y: 0.0, max_x: 297.5, max_y: 842.0 };
        let t0 = g.tile_world_rect(page, 0, 0);
        let t2 = g.tile_world_rect(page, 2, 0); // rightmost, partial
        assert!((t0.min_x - page.min_x).abs() < 1e-4);
        assert!((t2.max_x - page.max_x).abs() < 1e-3, "rightmost tile reaches page edge");
    }
}
