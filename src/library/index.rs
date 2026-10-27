//! Load / save the [`LibraryIndex`] blob to `data-store`.
//!
//! The whole index is one bincoded blob under a single key — small
//! enough (folders + items + tags without body bytes or covers) that a
//! full rewrite on every mutation stays cheap, and atomic by virtue of
//! the native backend's replace-in-place semantics
//! (`SharedPreferences.commit`, `UserDefaults.setObject`). Writes go
//! through one background thread so no caller waits on the store.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};

use bincode::config::{self, Configuration};
use istmo_data_store::DataStoreClient;

use super::error::{LibraryError, Result};
use super::links::LinkIndex;
use super::model::{INDEX_VERSION, LibraryIndex, WorkspaceIndex};

const CODEC: Configuration = config::standard();

/// data-store key the whole blob lives under. The suffix tracks the
/// on-disk schema: a blob written under an older key is never read, so
/// a schema break simply starts the library over.
pub const INDEX_KEY: &str = "library.index.v3";

/// data-store key for the [`WorkspaceIndex`] blob. Kept on its own key
/// so introducing workspaces does not force a bump of [`INDEX_KEY`].
pub const WORKSPACES_KEY: &str = "library.workspaces.v4";

/// data-store namespace used for the library. Matches the app id
/// declared in `istmo.toml` so backends can partition per-app data
/// on shared surfaces (Android multi-user, iOS App Groups).
pub const NAMESPACE: &str = "rs.sergioribera.notinplus";

/// Fetch the persisted index. `None` when the key has never been
/// written or the stored blob carries a schema version this build does
/// not read — the caller starts from a fresh index in both cases.
///
/// # Errors
/// Runtime / data-store / codec failures.
pub async fn load(client: &DataStoreClient) -> Result<Option<LibraryIndex>> {
    let raw = client.get_bytes(INDEX_KEY.to_owned()).await?;
    let Some(bytes) = raw else {
        return Ok(None);
    };
    let (index, _) = bincode::decode_from_slice::<LibraryIndex, _>(&bytes, CODEC)
        .map_err(|e| LibraryError::Codec(e.to_string()))?;
    if index.version != INDEX_VERSION {
        log::warn!(
            "library index version {} does not match {INDEX_VERSION}; starting over",
            index.version
        );
        return Ok(None);
    }
    Ok(Some(index))
}

/// Fetch the persisted workspace index or return a fresh default.
///
/// # Errors
/// Runtime / data-store / codec failures.
pub async fn load_workspaces(client: &DataStoreClient) -> Result<WorkspaceIndex> {
    let raw = client.get_bytes(WORKSPACES_KEY.to_owned()).await?;
    let Some(bytes) = raw else {
        return Ok(WorkspaceIndex::default());
    };
    let (index, _) = bincode::decode_from_slice::<WorkspaceIndex, _>(&bytes, CODEC)
        .map_err(|e| LibraryError::Codec(e.to_string()))?;
    Ok(index)
}

/// data-store key for the [`LinkIndex`] blob.
pub const LINKS_KEY: &str = "library.links.v1";

/// Fetch the persisted link index or return a fresh default.
///
/// # Errors
/// Runtime / data-store / codec failures.
pub async fn load_links(client: &DataStoreClient) -> Result<LinkIndex> {
    let raw = client.get_bytes(LINKS_KEY.to_owned()).await?;
    let Some(bytes) = raw else {
        return Ok(LinkIndex::default());
    };
    let (index, _) = bincode::decode_from_slice::<LinkIndex, _>(&bytes, CODEC)
        .map_err(|e| LibraryError::Codec(e.to_string()))?;
    Ok(index)
}

/// Encode `snapshot` for [`store`].
///
/// # Errors
/// Codec failure.
pub fn encode<T: bincode::Encode>(snapshot: &T) -> Result<Vec<u8>> {
    bincode::encode_to_vec(snapshot, CODEC).map_err(|e| LibraryError::Codec(e.to_string()))
}

/// Blobs on their way to the store: the newest bytes per key, and
/// whether a write is in flight.
#[derive(Default)]
struct Queue {
    waiting: HashMap<&'static str, Vec<u8>>,
    busy: bool,
}

struct Writer {
    queue: Mutex<Queue>,
    wake: Condvar,
}

static WRITER: OnceLock<Writer> = OnceLock::new();

fn writer() -> &'static Writer {
    WRITER.get_or_init(|| Writer {
        queue: Mutex::new(Queue::default()),
        wake: Condvar::new(),
    })
}

impl Writer {
    fn queue(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Write what is waiting, oldest state never after a newer one,
    /// for as long as the process lives.
    fn run(&self, client: &DataStoreClient) {
        loop {
            let batch: Vec<(&'static str, Vec<u8>)> = {
                let mut queue = self.queue();
                while queue.waiting.is_empty() {
                    queue = self
                        .wake
                        .wait(queue)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                queue.busy = true;
                queue.waiting.drain().collect()
            };
            for (key, bytes) in batch {
                if let Err(err) = pollster::block_on(client.set_bytes(key.to_owned(), bytes)) {
                    log::error!("library store {key}: {err}");
                }
            }
            self.queue().busy = false;
            self.wake.notify_all();
        }
    }
}

/// Hand `bytes` to the store under `key` without waiting for it. Bursts
/// collapse: only the newest bytes of a key are written.
pub fn store(client: &Arc<DataStoreClient>, key: &'static str, bytes: Vec<u8>) {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let client = Arc::clone(client);
        let spawned = std::thread::Builder::new()
            .name("notein-store".to_owned())
            .spawn(move || writer().run(&client));
        if let Err(err) = spawned {
            log::error!("library store thread: {err}");
        }
    });
    let writer = writer();
    writer.queue().waiting.insert(key, bytes);
    writer.wake.notify_all();
}

/// Block until everything handed to [`store`] is written.
pub fn flush() {
    let writer = writer();
    let mut queue = writer.queue();
    while queue.busy || !queue.waiting.is_empty() {
        queue = writer
            .wake
            .wait(queue)
            .unwrap_or_else(PoisonError::into_inner);
    }
    drop(queue);
}
