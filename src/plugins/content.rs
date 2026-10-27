//! Where kinds keep their content: the per-document plugin snapshot,
//! which the sync service already replicates.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

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

static GLANCES: std::sync::LazyLock<Mutex<HashMap<ItemId, (u64, Glance)>>> =
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

    /// The glance of `item`, recomputed only when it changed.
    #[must_use]
    pub fn of(item: &Item) -> Self {
        let mut cache = GLANCES.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((at, glance)) = cache.get(&item.id)
            && *at == item.updated_at
        {
            return glance.clone();
        }
        let glance = Self::compute(item);
        cache.insert(item.id, (item.updated_at, glance.clone()));
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
        read(self.item, &self.kind)
    }

    fn save(&self, bytes: Vec<u8>) {
        if let Err(err) = bodies::write_plugin_snapshot(self.item, &self.kind, &bytes) {
            log::error!("content save {}: {err}", self.item);
            return;
        }
        Glance::forget(self.item);
        crate::sync::snapshot_changed(self.item);
        record_references(self.item, &self.kind, &bytes);
        reindex(self.item);
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

/// Re-index `item` in the background after its content changed.
pub fn reindex(item: ItemId) {
    std::thread::spawn(move || index_blocking(item));
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
        }).map_or_else(|err| log::error!("index thread: {err}"), drop);
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
