//! Uniform-grid spatial index over committed strokes.
//!
//! Buckets are 128-dp squares — a stroke registers under every bucket
//! any of its points falls into. Eraser queries hit-test against the
//! union of buckets touched by the eraser circle, pruning the O(N)
//! stroke scan to O(K) where K = strokes actually near the cursor.
//!
//! The index stores raw stroke ids ([`crate::brush::Stroke::id`]); it
//! does not own the strokes. Callers keep the index in sync via
//! [`insert`]/[`remove`] when the document mutates.

use std::collections::{HashMap, HashSet};

use crate::brush::InkPoint;

const BUCKET_DP: f32 = 128.0;

pub type StrokeId = u32;
type BucketKey = (i32, i32);

#[derive(Debug, Default)]
pub struct SpatialIndex {
    buckets: HashMap<BucketKey, Vec<StrokeId>>,
}

impl SpatialIndex {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.buckets.clear();
    }

    /// Index every bucket the point set touches. Duplicate keys are
    /// collapsed so a wiggly stroke doesn't register N times in the
    /// same bucket.
    pub fn insert(&mut self, id: StrokeId, points: &[InkPoint]) {
        let mut seen = HashSet::new();
        for p in points {
            let key = bucket_of(p.x, p.y);
            if seen.insert(key) {
                self.buckets.entry(key).or_default().push(id);
            }
        }
    }

    pub fn remove(&mut self, id: StrokeId, points: &[InkPoint]) {
        let mut seen = HashSet::new();
        for p in points {
            let key = bucket_of(p.x, p.y);
            if !seen.insert(key) {
                continue;
            }
            if let Some(v) = self.buckets.get_mut(&key) {
                v.retain(|x| *x != id);
                if v.is_empty() {
                    self.buckets.remove(&key);
                }
            }
        }
    }

    /// All stroke ids in any bucket the `(cx, cy, r)` circle overlaps.
    /// Broad-phase only — caller does the actual segment-vs-circle
    /// hit test.
    #[must_use]
    pub fn query_circle(&self, cx: f32, cy: f32, r: f32) -> Vec<StrokeId> {
        let (min_x, min_y) = bucket_of(cx - r, cy - r);
        let (max_x, max_y) = bucket_of(cx + r, cy + r);
        let mut out = HashSet::new();
        for by in min_y..=max_y {
            for bx in min_x..=max_x {
                if let Some(v) = self.buckets.get(&(bx, by)) {
                    for id in v {
                        out.insert(*id);
                    }
                }
            }
        }
        out.into_iter().collect()
    }
}

#[allow(clippy::cast_possible_truncation)]
fn bucket_of(x: f32, y: f32) -> BucketKey {
    (
        (x / BUCKET_DP).floor() as i32,
        (y / BUCKET_DP).floor() as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f32, y: f32) -> InkPoint {
        InkPoint::new(x, y, 128, 0, 0)
    }

    #[test]
    fn query_finds_nearby() {
        let mut idx = SpatialIndex::new();
        idx.insert(1, &[pt(10.0, 10.0), pt(20.0, 20.0)]);
        idx.insert(2, &[pt(500.0, 500.0)]);
        let hits = idx.query_circle(15.0, 15.0, 20.0);
        assert!(hits.contains(&1));
        assert!(!hits.contains(&2));
    }

    #[test]
    fn remove_evicts() {
        let mut idx = SpatialIndex::new();
        idx.insert(1, &[pt(10.0, 10.0)]);
        idx.remove(1, &[pt(10.0, 10.0)]);
        assert!(idx.query_circle(10.0, 10.0, 5.0).is_empty());
    }
}
