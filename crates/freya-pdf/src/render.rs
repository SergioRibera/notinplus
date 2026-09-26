//! Async worker pool driving pdfium off the UI thread.
//!
//! The pool owns N OS threads pulling [`RenderRequest`]s from a
//! `flume` channel. Each worker rasterises one tile (or, for small
//! pages, the whole page) via pdfium, stores raw RGBA bytes into the
//! shared [`Cache`], and pings the installed [`RedrawHandle`] so
//! freya re-paints on the next frame. Cancellation is cooperative —
//! workers check the request's [`CancelToken`] before entering
//! pdfium and skip cache insertion if the flag flipped while the
//! job was queued.
//!
//! # Tiling
//!
//! Pages whose rasterised dimensions exceed [`TILE_THRESHOLD`] are
//! sliced into [`TILE_PIXELS`]-sized cells via a `translate + clip`
//! pdfium render config. Each cell renders into a bitmap sized to
//! its actual coverage (edge cells shrink to the leftover pixels)
//! so the cache holds exactly the pixel data it needs — no padding.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use flume::{Receiver, Sender};
use freya_canvas_bg::{PageId, RedrawHandle};
use pdfium_render::prelude::{PdfBitmap, PdfBitmapFormat, PdfRenderConfig, Pixels};

use crate::cache::{Cache, CachedTile};
use crate::cancel::CancelToken;
use crate::doc::PdfDocument;
use crate::tiles::{CacheKey, TILE_PIXELS, TileCoord, bucket_scale};

/// Request to render one cache entry.
#[derive(Debug)]
struct RenderRequest {
    key: CacheKey,
    cancel: CancelToken,
}

/// Worker pool + pending-request tracker. Cloneable via `Arc`.
#[derive(Debug)]
pub struct RenderPool {
    doc: PdfDocument,
    cache: Arc<Cache>,
    /// Wrapped in `Option` so `Drop` can move it out and close the
    /// channel — workers see `rx.recv()` disconnect and exit.
    tx: Mutex<Option<Sender<RenderRequest>>>,
    pending: Arc<Mutex<HashMap<CacheKey, CancelToken>>>,
    redraw: Arc<Mutex<RedrawHandle>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl RenderPool {
    /// Build a pool with `worker_count` OS threads. Two workers is a
    /// sensible default: pdfium is largely CPU-bound and its internal
    /// mutex serialises heavy operations, so extra threads mostly buy
    /// concurrency for page-level bookkeeping.
    #[must_use]
    pub fn new(doc: &PdfDocument, cache: &Arc<Cache>, worker_count: usize) -> Arc<Self> {
        let worker_count = worker_count.max(1);
        let (tx, rx) = flume::unbounded::<RenderRequest>();
        let pending: Arc<Mutex<HashMap<CacheKey, CancelToken>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let redraw = Arc::new(Mutex::new(RedrawHandle::noop()));

        let this = Arc::new(Self {
            doc: doc.clone(),
            cache: Arc::clone(cache),
            tx: Mutex::new(Some(tx)),
            pending: Arc::clone(&pending),
            redraw: Arc::clone(&redraw),
            workers: Mutex::new(Vec::new()),
        });

        let mut workers = Vec::with_capacity(worker_count);
        for i in 0..worker_count {
            let doc = doc.clone();
            let cache = Arc::clone(cache);
            let rx = rx.clone();
            let pending = Arc::clone(&pending);
            let redraw = Arc::clone(&redraw);
            let handle = std::thread::Builder::new()
                .name(format!("freya-pdf-worker-{i}"))
                .spawn(move || worker_loop(&rx, &doc, &cache, &pending, &redraw))
                .expect("spawn freya-pdf worker thread");
            workers.push(handle);
        }
        *lock(&this.workers) = workers;

        this
    }

    /// Install the wake handle workers ping after a successful render.
    /// The paint pass forwards the freya redraw notifier here so
    /// tile-ready events cause the next frame to be scheduled.
    pub fn set_redraw(&self, redraw: RedrawHandle) {
        *lock(&self.redraw) = redraw;
    }

    /// Queue a render for `key`. No-op if an active (non-cancelled)
    /// request for the same key is already in flight.
    pub fn request(&self, key: CacheKey) {
        let mut pending = lock(&self.pending);
        if let Some(existing) = pending.get(&key) {
            if !existing.is_cancelled() {
                return;
            }
        }
        let cancel = CancelToken::new();
        pending.insert(key, cancel.clone());
        drop(pending);
        let tx = lock(&self.tx);
        if let Some(tx) = tx.as_ref() {
            let _ = tx.send(RenderRequest { key, cancel });
        }
    }

    /// Mark every pending request whose page is outside `keep_pages`
    /// as cancelled. Workers pop those and skip the pdfium call.
    pub fn cancel_outside(&self, keep_pages: &HashSet<PageId>) {
        let pending = lock(&self.pending);
        for (key, token) in pending.iter() {
            if !keep_pages.contains(&key.page) {
                token.cancel();
            }
        }
    }

    /// Access the document handle — callers use it for text
    /// extraction, metadata queries, etc.
    #[must_use]
    pub const fn document(&self) -> &PdfDocument {
        &self.doc
    }

    /// Access the shared cache — the backend reads it during paint.
    #[must_use]
    pub const fn cache(&self) -> &Arc<Cache> {
        &self.cache
    }
}

impl Drop for RenderPool {
    fn drop(&mut self) {
        // Close the channel so worker `rx.recv()` calls return Err
        // and the loop exits cleanly.
        drop(lock(&self.tx).take());
        let mut workers = lock(&self.workers);
        for handle in workers.drain(..) {
            let _ = handle.join();
        }
    }
}

fn worker_loop(
    rx: &Receiver<RenderRequest>,
    doc: &PdfDocument,
    cache: &Cache,
    pending: &Mutex<HashMap<CacheKey, CancelToken>>,
    redraw: &Mutex<RedrawHandle>,
) {
    while let Ok(req) = rx.recv() {
        let key = req.key;
        if req.cancel.is_cancelled() {
            forget_pending(pending, key);
            continue;
        }
        if let Some(tile) = render_entry(doc, key) {
            cache.insert(key, Arc::new(tile));
            lock(redraw).ping();
        }
        forget_pending(pending, key);
    }
}

fn forget_pending(pending: &Mutex<HashMap<CacheKey, CancelToken>>, key: CacheKey) {
    lock(pending).remove(&key);
}

/// Rasterise one cache entry. `TileCoord::Full` renders the whole
/// page; `TileCoord::Cell` renders one grid slice via a
/// `translate + clip` render config so pdfium only rasterises pixels
/// that land in the tile bitmap.
#[allow(clippy::cast_possible_wrap)] // pdfium pixel counts fit i32 for any realistic zoom
fn render_entry(doc: &PdfDocument, key: CacheKey) -> Option<CachedTile> {
    let page_slot = usize::try_from(key.page.0).ok()?;
    let (width_points, height_points) = doc.page_size(page_slot).ok()?;
    let scale = bucket_scale(key.bucket);
    let pdf_doc = doc.pdfium_doc();
    let pages = pdf_doc.pages();
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let pdf_page = pages.get(key.page.0 as i32).ok()?;

    match key.tile {
        TileCoord::Full => {
            let target_w = pixel_dim(width_points * scale)? as Pixels;
            let target_h = pixel_dim(height_points * scale)? as Pixels;
            let config = PdfRenderConfig::new().set_target_size(target_w, target_h);
            let bitmap = pdf_page.render_with_config(&config).ok()?;
            Some(cache_tile_from_bitmap(&bitmap))
        }
        TileCoord::Cell { x, y } => {
            let full_w = pixel_dim(width_points * scale)? as Pixels;
            let full_h = pixel_dim(height_points * scale)? as Pixels;
            let start_x = Pixels::from(x).checked_mul(TILE_PIXELS as Pixels)?;
            let start_y = Pixels::from(y).checked_mul(TILE_PIXELS as Pixels)?;
            let tile_w = (full_w - start_x).min(TILE_PIXELS as Pixels);
            let tile_h = (full_h - start_y).min(TILE_PIXELS as Pixels);
            if tile_w <= 0 || tile_h <= 0 {
                return None;
            }
            let mut bitmap =
                PdfBitmap::empty(tile_w, tile_h, PdfBitmapFormat::BGRA).ok()?;
            // Sub-tile trick: ask pdfium to render the WHOLE page at
            // `full_w × full_h`, but with the page's top-left at
            // `(-start_x, -start_y)` inside a `tile_w × tile_h` bitmap.
            // Pdfium clips to the destination bitmap's own size, so we
            // get exactly the cell we want. This deliberately avoids
            // `FPDF_RenderPageBitmapWithMatrix` (the transform+clip
            // path), which throws `std::bad_variant_access` inside
            // pdfium 7881 and abort()s the process because pdfium is
            // built with `-fno-exceptions`. `set_target_size` +
            // `set_origin` stays on the form-data path
            // (`FPDF_RenderPageBitmap`), which is stable.
            let config = PdfRenderConfig::new()
                .set_target_size(full_w, full_h)
                .set_origin(-start_x, -start_y);
            pdf_page
                .render_into_bitmap_with_config(&mut bitmap, &config)
                .ok()?;
            Some(cache_tile_from_bitmap(&bitmap))
        }
    }
}

fn cache_tile_from_bitmap(bitmap: &PdfBitmap) -> CachedTile {
    let bytes = bitmap.as_rgba_bytes();
    #[allow(clippy::cast_sign_loss)]
    let width = bitmap.width() as u32;
    #[allow(clippy::cast_sign_loss)]
    let height = bitmap.height() as u32;
    CachedTile {
        bytes: Arc::new(bytes),
        width,
        height,
    }
}

#[allow(clippy::unnecessary_wraps)] // callers propagate with `?`; None path is real
fn pixel_dim(v: f32) -> Option<u32> {
    if !v.is_finite() || v <= 0.0 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let n = v.round() as u32;
    Some(n)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}
