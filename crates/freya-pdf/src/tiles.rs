//! Zoom bucketing.
//!
//! The render cache is keyed by a discrete `bucket` per page instead
//! of the raw viewport scale. Bucketing at powers of `1.5` keeps cache
//! hits high across small zoom deltas (a pinch-zoom that moves scale
//! from 1.0 to 1.2 stays in the same bucket) while still tracking
//! large zooms.

use freya_canvas_bg::PageId;

/// Geometric ratio between adjacent zoom buckets.
const BUCKET_BASE: f32 = 1.5;

/// Quantize a raw viewport scale into a discrete bucket index. Bucket
/// `n` covers scales in `[1.5^(n-0.5), 1.5^(n+0.5))`.
#[must_use]
pub fn bucket_for(scale: f32) -> i32 {
    if !scale.is_finite() || scale <= 0.0 {
        return 0;
    }
    #[allow(clippy::cast_possible_truncation)]
    let b = scale.log(BUCKET_BASE).round() as i32;
    b
}

/// Effective scale represented by a given bucket. Used to size the
/// bitmap we ask pdfium for so cached tiles all live on the same
/// zoom quantum.
#[must_use]
pub fn bucket_scale(bucket: i32) -> f32 {
    BUCKET_BASE.powi(bucket)
}

/// Cache lookup key. `page` selects the pdf page; `bucket` picks the
/// LOD level.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct CacheKey {
    /// Which page the tile came from.
    pub page: PageId,
    /// LOD level, produced by [`bucket_for`].
    pub bucket: i32,
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
}
