//! Where kinds keep their content: the per-document plugin snapshot,
//! which the sync service already replicates.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use notein_i18n::Lang;
use notein_plugin::doc::{DocId, DocMeta};
use notein_plugin::face::RawStore;

use crate::library::{Item, ItemId, bodies};

/// What a card shows about a document's content.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Glance {
    /// The kind's one-line summary ("248 páginas").
    pub summary: Option<String>,
    /// First lines of text, for kinds whose cover is their text.
    pub preview: Option<String>,
}

/// Characters of a text preview on a card.
const PREVIEW_CHARS: usize = 180;

/// A glance and what it was computed for: the item revision and the
/// interface language.
type CachedGlance = (u64, Lang, Glance);

static GLANCES: std::sync::LazyLock<Mutex<HashMap<ItemId, CachedGlance>>> =
    std::sync::LazyLock::new(Mutex::default);

impl Glance {
    fn compute(item: &Item) -> Self {
        let kind = &item.kind.plugin_id;
        let (Some(bytes), Some(kind)) = (read(item.id, kind), crate::plugins::kind(kind)) else {
            return Self::default();
        };
        let preview = kind
            .excerpt(&bytes)
            .map(|text| text.chars().take(PREVIEW_CHARS).collect::<String>())
            .filter(|text| !text.trim().is_empty());
        Self {
            summary: kind.summary(&bytes),
            preview,
        }
    }

    fn forget(item: ItemId) {
        GLANCES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&item);
    }

    /// The glance of `item`, recomputed when it or the interface
    /// language changed.
    #[must_use]
    pub fn of(item: &Item) -> Self {
        let mut cache = GLANCES.lock().unwrap_or_else(PoisonError::into_inner);
        let lang = Lang::current();
        if let Some((at, at_lang, glance)) = cache.get(&item.id)
            && *at == item.updated_at
            && *at_lang == lang
        {
            return glance.clone();
        }
        let glance = Self::compute(item);
        cache.insert(item.id, (item.updated_at, lang, glance.clone()));
        glance
    }
}

/// Tell open faces of `item` that its content changed elsewhere.
pub fn notify_remote(item: ItemId) {
    Glance::forget(item);
    crate::sync::open_docs::notify_snapshot(item);
}

/// Content bytes of `item` for kind `kind`; `None` when never saved.
#[must_use]
pub fn read(item: ItemId, kind: &str) -> Option<Vec<u8>> {
    bodies::read_plugin_snapshot(item, kind).ok().flatten()
}

/// Replace the content bytes of `item` and let sync and search know.
pub fn write(item: ItemId, kind: &str, bytes: &[u8]) {
    if let Err(err) = bodies::write_plugin_snapshot(item, kind, bytes) {
        log::error!("content write {item}: {err}");
        return;
    }
    Glance::forget(item);
    crate::sync::local_changed();
}

/// Keep what the content of `item` links to, when its kind says.
pub fn record_references(item: ItemId, kind: &str, bytes: &[u8]) {
    if let Some(refs) = super::kind(kind).and_then(|k| k.references(bytes)) {
        crate::library::refs::record_content(item, refs);
    }
}

/// [`RawStore`] over one document's snapshot.
#[derive(Debug)]
pub struct SnapshotStore {
    item: ItemId,
    kind: String,
}

impl SnapshotStore {
    #[must_use]
    pub fn new(item: ItemId, kind: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            item,
            kind: kind.into(),
        })
    }
}

impl RawStore for SnapshotStore {
    fn load(&self) -> Option<Vec<u8>> {
        let unsaved = unsaved().get(&(self.item, self.kind.clone())).cloned();
        unsaved.or_else(|| read(self.item, &self.kind))
    }

    /// Kept in memory at once and written once changes pause.
    fn save(&self, bytes: Vec<u8>) {
        let (item, kind) = (self.item, self.kind.clone());
        unsaved().insert((item, kind.clone()), bytes);
        crate::deferred::schedule(store_key(item, &kind), move || store_unsaved(item, &kind));
    }

    fn changes(&self) -> flume::Receiver<()> {
        crate::sync::open_docs::watch_snapshot(self.item)
    }

    fn read_only(&self) -> bool {
        !crate::sync::can_edit(self.item)
    }

    fn access_changes(&self) -> flume::Receiver<()> {
        crate::sync::changes()
    }
}

type Unsaved = HashMap<(ItemId, String), Vec<u8>>;

/// Snapshots saved by a face and not on disk yet.
fn unsaved() -> std::sync::MutexGuard<'static, Unsaved> {
    static UNSAVED: std::sync::LazyLock<Mutex<Unsaved>> = std::sync::LazyLock::new(Mutex::default);
    UNSAVED.lock().unwrap_or_else(PoisonError::into_inner)
}

fn store_key(item: ItemId, kind: &str) -> String {
    format!("snapshot:{item}:{kind}")
}

fn store_unsaved(item: ItemId, kind: &str) {
    let key = (item, kind.to_owned());
    let Some(bytes) = unsaved().get(&key).cloned() else {
        return;
    };
    let written = bodies::write_plugin_snapshot(item, kind, &bytes);
    // Saved again meanwhile: those bytes stay for the next write.
    let mut waiting = unsaved();
    if waiting.get(&key) == Some(&bytes) {
        waiting.remove(&key);
    }
    drop(waiting);
    if let Err(err) = written {
        log::error!("content save {item}: {err}");
        return;
    }
    Glance::forget(item);
    crate::sync::snapshot_changed(item);
    record_references(item, kind, &bytes);
    reindex(item);
}

/// Library id → plugin doc id.
#[must_use]
pub const fn doc_id(item: ItemId) -> DocId {
    DocId(item.0)
}

/// Feed `item`'s content to its kind's search index.
pub fn index(item: &Item) {
    let Some(kind) = super::kind(&item.kind.plugin_id) else {
        return;
    };
    let content = read(item.id, &item.kind.plugin_id).unwrap_or_else(|| kind.empty_content());
    let attachment = kind
        .info()
        .attachment
        .and_then(|_| bodies::read_pdf(item.id).ok());
    kind.index(meta(item), &content, attachment.as_deref());
}

/// Re-index `item` in the background after its content changed. One
/// thread works through the requests; a burst for the same document
/// indexes it once.
pub fn reindex(item: ItemId) {
    static QUEUE: std::sync::OnceLock<Option<flume::Sender<ItemId>>> = std::sync::OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        let (tx, rx) = flume::unbounded::<ItemId>();
        std::thread::Builder::new()
            .name("notein-reindex".to_owned())
            .spawn(move || {
                while let Ok(first) = rx.recv() {
                    let mut batch = vec![first];
                    for item in rx.try_iter() {
                        if !batch.contains(&item) {
                            batch.push(item);
                        }
                    }
                    for item in batch {
                        index_blocking(item);
                    }
                }
            })
            .map_err(|err| log::error!("reindex thread: {err}"))
            .ok()
            .map(|_| tx)
    });
    if let Some(tx) = queue {
        let _ = tx.send(item);
    }
}

/// Index `item` on this thread, if it is still in the library.
pub fn index_blocking(item: ItemId) {
    let Ok(handle) = crate::library::shared_blocking() else {
        return;
    };
    let found = pollster::block_on(handle.lock()).item(item).cloned();
    if let Some(found) = found.filter(|i| i.trashed.is_none()) {
        index(&found);
    }
}

/// Index every live document, on a background thread, once at start.
pub fn index_library() {
    std::thread::Builder::new()
        .name("notein-index".to_owned())
        .spawn(|| {
            let Ok(handle) = crate::library::shared_blocking() else {
                return;
            };
            let items: Vec<Item> = pollster::block_on(handle.lock())
                .live_items()
                .cloned()
                .collect();
            for item in &items {
                index(item);
            }
        })
        .map_or_else(|err| log::error!("index thread: {err}"), drop);
}

/// Drop `item` from its kind's index (it left the library).
pub fn forget(item: &Item) {
    if let Some(kind) = super::kind(&item.kind.plugin_id) {
        kind.forget(doc_id(item.id));
    }
}

type AttachmentWatchers = Mutex<Vec<(ItemId, flume::Sender<()>)>>;

static ATTACHMENT_WATCHERS: AttachmentWatchers = Mutex::new(Vec::new());

/// Wake whenever the file `item` keeps is replaced.
#[must_use]
pub fn watch_attachment(item: ItemId) -> flume::Receiver<()> {
    let (tx, rx) = flume::unbounded();
    ATTACHMENT_WATCHERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((item, tx));
    rx
}

static ASSET_WATCHERS: Mutex<Vec<(ItemId, flume::Sender<()>)>> = Mutex::new(Vec::new());

/// Fires when files `item` links to (images) arrive from another device.
#[must_use]
pub fn watch_assets(item: ItemId) -> flume::Receiver<()> {
    let (tx, rx) = flume::unbounded();
    ASSET_WATCHERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((item, tx));
    rx
}

/// Tell whoever shows `item` that more of its files are here.
pub fn assets_arrived(item: ItemId) {
    ASSET_WATCHERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|(id, tx)| !tx.is_disconnected() && (*id != item || tx.send(()).is_ok()));
}

/// Tell the open faces of `item` that its kept file changed.
pub fn attachment_replaced(item: ItemId) {
    ATTACHMENT_WATCHERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|(id, tx)| !tx.is_disconnected() && (*id != item || tx.send(()).is_ok()));
}

/// What kinds are told about `item`.
#[must_use]
pub fn meta(item: &Item) -> DocMeta<'_> {
    DocMeta {
        id: doc_id(item.id),
        name: &item.name,
        updated_at: item.updated_at,
        tint: item.color,
    }
}
