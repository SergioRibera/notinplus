//! LRU cache of rendered page bitmaps.
//!
//! Tiles keyed by [`CacheKey`] (page + zoom bucket) so scrolling
//! through the same set of pages at a stable zoom stays hot.
//! `Arc<CachedTile>` so the paint loop can hand out clones without
//! copying the underlying Skia image.

use std::sync::{Arc, Mutex};

use freya_canvas_bg::PageId;
use hashlink::LruCache;

use crate::tiles::CacheKey;

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

/// Shared LRU cache. Cheap to clone (Arc<Mutex<..>>). Interior
/// `Mutex` because `LruCache::get` mutates internal ordering.
#[derive(Debug)]
pub struct Cache {
    inner: Mutex<LruCache<CacheKey, Arc<CachedTile>>>,
}

impl Cache {
    /// New cache with an upper entry count.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(LruCache::new(capacity)),
        }
    }

    /// Fetch a tile — touching LRU ordering on hit.
    #[must_use]
    pub fn get(&self, key: CacheKey) -> Option<Arc<CachedTile>> {
        self.lock().get(&key).cloned()
    }

    /// Peek without disturbing LRU ordering. Right for placeholder
    /// scans that must not evict live buckets.
    #[must_use]
    pub fn peek(&self, key: CacheKey) -> Option<Arc<CachedTile>> {
        self.lock().peek(&key).cloned()
    }

    /// Insert or replace a tile.
    pub fn insert(&self, key: CacheKey, tile: Arc<CachedTile>) {
        self.lock().insert(key, tile);
    }

    /// Best-effort scan for the highest-resolution cached tile of
    /// `page` at bucket `<= max_bucket`. Used to paint an upscaled
    /// placeholder while the target-bucket render is in flight.
    #[must_use]
    pub fn nearest_at_or_below(&self, page: PageId, max_bucket: i32) -> Option<Arc<CachedTile>> {
        let mut best: Option<(i32, Arc<CachedTile>)> = None;
        {
            let guard = self.lock();
            for (k, v) in guard.iter() {
                if k.page != page || k.bucket > max_bucket {
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

    fn lock(&self) -> std::sync::MutexGuard<'_, LruCache<CacheKey, Arc<CachedTile>>> {
        match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}
