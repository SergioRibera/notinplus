//! LRU cache of rendered page bitmaps.
//!
//! Tiles keyed by [`CacheKey`] (page + zoom bucket) so scrolling
//! through the same set of pages at a stable zoom stays hot.
//! `Arc<CachedTile>` so the paint loop can hand out clones without
//! copying the underlying Skia image.

use std::sync::{Arc, Mutex};

use freya_canvas_bg::PageId;
use hashlink::LruCache;

use crate::tiles::{CacheKey, TileCoord};

/// One cached page render at a specific zoom bucket.
///
/// Stores raw RGBA bytes rather than a Skia `Image`: `Image` handles
/// are only conditionally `Send` (unique refcount), which makes them
/// unsafe to hand across the worker → paint-thread boundary. The
/// paint pass wraps the bytes into an `Image` just-in-time via
/// `raster_from_data`.
#[derive(Debug)]
pub struct CachedTile {
    /// RGBA pixel buffer, `width * height * 4` bytes.
    pub bytes: Arc<Vec<u8>>,
    /// Width in pixels the tile was rasterised at.
    pub width: u32,
    /// Height in pixels the tile was rasterised at.
    pub height: u32,
}

/// Shared LRU cache, bounded by entry count and by the bytes its bitmaps
/// hold. Interior `Mutex` because `LruCache::get` mutates internal
/// ordering.
#[derive(Debug)]
pub struct Cache {
    inner: Mutex<Tiles>,
}

#[derive(Debug)]
struct Tiles {
    lru: LruCache<CacheKey, Arc<CachedTile>>,
    bytes: usize,
    budget: usize,
}

impl CachedTile {
    fn size(&self) -> usize {
        self.bytes.len()
    }
}

impl Cache {
    /// New cache holding at most `capacity` tiles and `budget` bytes of
    /// pixels; the least recently used tiles go first.
    #[must_use]
    pub fn new(capacity: usize, budget: usize) -> Self {
        Self {
            inner: Mutex::new(Tiles {
                lru: LruCache::new(capacity),
                bytes: 0,
                budget,
            }),
        }
    }

    /// Fetch a tile — touching LRU ordering on hit.
    #[must_use]
    pub fn get(&self, key: CacheKey) -> Option<Arc<CachedTile>> {
        self.lock().lru.get(&key).cloned()
    }

    /// Peek without disturbing LRU ordering. Right for placeholder
    /// scans that must not evict live buckets.
    #[must_use]
    pub fn peek(&self, key: CacheKey) -> Option<Arc<CachedTile>> {
        self.lock().lru.peek(&key).cloned()
    }

    /// Insert or replace a tile.
    pub fn insert(&self, key: CacheKey, tile: Arc<CachedTile>) {
        let mut tiles = self.lock();
        tiles.bytes += tile.size();
        let full = tiles.lru.len() == tiles.lru.capacity() && !tiles.lru.contains_key(&key);
        if full && let Some((_, gone)) = tiles.lru.remove_lru() {
            tiles.bytes -= gone.size();
        }
        if let Some(old) = tiles.lru.insert(key, tile) {
            tiles.bytes -= old.size();
        }
        while tiles.bytes > tiles.budget && tiles.lru.len() > 1 {
            let Some((_, gone)) = tiles.lru.remove_lru() else {
                break;
            };
            tiles.bytes -= gone.size();
        }
    }

    /// Bytes of pixels held.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.lock().bytes
    }

    /// Best-effort scan for the highest-resolution cached tile of
    /// `page` at bucket `<= max_bucket`. Used to paint an upscaled
    /// placeholder while the target-bucket render is in flight.
    #[must_use]
    pub fn nearest_at_or_below(&self, page: PageId, max_bucket: i32) -> Option<Arc<CachedTile>> {
        self.nearest_with(page, max_bucket, |_| true)
    }

    /// Best full-page render (i.e. [`TileCoord::Full`]) for `page` at
    /// bucket `<= max_bucket`. Right for placeholder rendering on
    /// tiled pages — a full-page tile at a lower bucket can be
    /// upscaled to cover every sub-tile of the same page.
    #[must_use]
    pub fn nearest_full_at_or_below(
        &self,
        page: PageId,
        max_bucket: i32,
    ) -> Option<Arc<CachedTile>> {
        self.nearest_with(page, max_bucket, |k| matches!(k.tile, TileCoord::Full))
    }

    fn nearest_with(
        &self,
        page: PageId,
        max_bucket: i32,
        pred: impl Fn(&CacheKey) -> bool,
    ) -> Option<Arc<CachedTile>> {
        let mut best: Option<(i32, Arc<CachedTile>)> = None;
        {
            let guard = self.lock();
            for (k, v) in guard.lru.iter() {
                if k.page != page || k.bucket > max_bucket || !pred(k) {
                    continue;
                }
                match &best {
                    None => best = Some((k.bucket, Arc::clone(v))),
                    Some((b, _)) if k.bucket > *b => best = Some((k.bucket, Arc::clone(v))),
                    _ => {}
                }
            }
        }
        best.map(|(_, v)| v)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Tiles> {
        match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(page: u64) -> CacheKey {
        CacheKey {
            page: PageId(page),
            bucket: 0,
            tile: TileCoord::Full,
        }
    }

    fn tile(bytes: usize) -> Arc<CachedTile> {
        Arc::new(CachedTile {
            bytes: Arc::new(vec![0; bytes]),
            width: 1,
            height: 1,
        })
    }

    #[test]
    fn oldest_tiles_go_once_the_bytes_run_over() {
        let cache = Cache::new(16, 100);
        for page in 0..4 {
            cache.insert(key(page), tile(40));
        }
        assert_eq!(cache.bytes(), 80);
        assert!(cache.peek(key(1)).is_none());
        assert!(cache.peek(key(3)).is_some());
    }

    #[test]
    fn replacing_or_evicting_by_count_keeps_the_byte_count() {
        let cache = Cache::new(2, 1000);
        cache.insert(key(0), tile(10));
        cache.insert(key(0), tile(30));
        assert_eq!(cache.bytes(), 30);
        cache.insert(key(1), tile(10));
        cache.insert(key(2), tile(10));
        assert_eq!(cache.bytes(), 20);
    }
}
