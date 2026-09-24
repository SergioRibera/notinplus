//! Async worker pool driving pdfium off the UI thread.
//!
//! The pool owns N OS threads pulling [`RenderRequest`]s from a
//! `flume` channel. Each worker rasterises one page → bucket at a
//! time, stores raw RGBA bytes into the shared [`Cache`], and pings
//! the installed [`RedrawHandle`] so freya re-paints on the next
//! frame. Cancellation is cooperative — workers check the request's
//! [`CancelToken`] before entering pdfium and skip cache insertion if
//! the flag flipped while the job was queued.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use flume::{Receiver, Sender};
use freya_canvas_bg::{PageId, RedrawHandle};
use pdfium_render::prelude::{PdfRenderConfig, Pixels};

use crate::cache::{Cache, CachedTile};
use crate::cancel::CancelToken;
use crate::doc::PdfDocument;
use crate::tiles::CacheKey;

/// Request to render one page at a specific bucket.
#[derive(Debug)]
struct RenderRequest {
    key: CacheKey,
    target_w: u32,
    target_h: u32,
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
    pub fn request(&self, key: CacheKey, target_w: u32, target_h: u32) {
        if target_w == 0 || target_h == 0 {
            return;
        }
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
            let _ = tx.send(RenderRequest { key, target_w, target_h, cancel });
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
        if let Some(tile) = render_page(doc, key.page, req.target_w, req.target_h) {
            cache.insert(key, Arc::new(tile));
            lock(redraw).ping();
        }
        forget_pending(pending, key);
    }
}

fn forget_pending(pending: &Mutex<HashMap<CacheKey, CancelToken>>, key: CacheKey) {
    lock(pending).remove(&key);
}

/// Rasterise one page synchronously. Called from worker threads only.
/// Returns raw RGBA bytes so the result can cross the worker → paint
/// boundary — Skia `Image` handles are only conditionally `Send`
/// (unique-refcount) which is too fragile for a general cache.
fn render_page(doc: &PdfDocument, page: PageId, target_w: u32, target_h: u32) -> Option<CachedTile> {
    #[allow(clippy::cast_possible_truncation)]
    let page_idx = page.0 as u16;
    let pages = doc.pdfium_doc().pages();
    let pdf_page = pages.get(page_idx).ok()?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let config = PdfRenderConfig::new().set_target_size(target_w as Pixels, target_h as Pixels);
    let bitmap = pdf_page.render_with_config(&config).ok()?;
    let bytes = bitmap.as_rgba_bytes();
    #[allow(clippy::cast_sign_loss)]
    let width = bitmap.width() as u32;
    #[allow(clippy::cast_sign_loss)]
    let height = bitmap.height() as u32;
    Some(CachedTile {
        bytes: Arc::new(bytes),
        width,
        height,
    })
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}
