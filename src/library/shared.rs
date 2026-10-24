//! The one library instance of the process.
//!
//! The UI, the faces, the sync service and the replica all share it,
//! so a change made anywhere is the state everyone reads next, and
//! every persisted change wakes the [`changes`] subscribers.

use std::sync::{Arc, Mutex, PoisonError};

use async_lock::{Mutex as AsyncMutex, OnceCell};

use super::{Library, LibraryError, Result};

/// Shared handle to the library.
pub type LibHandle = Arc<AsyncMutex<Library>>;

static SHARED: OnceCell<LibHandle> = OnceCell::new();
static LISTENERS: Mutex<Vec<flume::Sender<()>>> = Mutex::new(Vec::new());

/// The library, opened on first use.
///
/// # Errors
/// [`LibraryError`] when the runtime is not up or the store cannot be
/// read.
pub async fn shared() -> Result<LibHandle> {
    SHARED
        .get_or_try_init(|| async {
            let runtime = istmo::Runtime::global().map_err(LibraryError::Runtime)?;
            let lib = Library::open(&runtime).await?;
            Ok(Arc::new(AsyncMutex::new(lib)))
        })
        .await
        .cloned()
}

/// [`shared`] for threads without an executor (sync service).
///
/// # Errors
/// Same as [`shared`].
pub fn shared_blocking() -> Result<LibHandle> {
    pollster::block_on(shared())
}

/// Subscribe to library changes. The receiver gets one message per
/// persisted change (index, mesas or links).
#[must_use]
pub fn changes() -> flume::Receiver<()> {
    let (tx, rx) = flume::unbounded();
    LISTENERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(tx);
    rx
}

/// Wake every [`changes`] subscriber.
pub fn notify() {
    LISTENERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|tx| tx.send(()).is_ok());
}
