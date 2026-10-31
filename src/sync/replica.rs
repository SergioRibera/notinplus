//! The library seen as a replica: documents on disk plus the index
//! records, read fresh on every call so the service never holds a
//! stale copy of what the UI is editing.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;

use notein_i18n::tr;
use notein_plugin::UserMessage;
use notein_sync::{Blob, BlobHead, BlobKey, DocId, Replica, Version};

use crate::active_boards;
use crate::canvas::lock;
use crate::doc::Doc;
use crate::library::graveyard::Graveyard;
use crate::library::sync::SyncRecord;
use crate::library::{DocKind, ItemId, Library, bodies, refs};
use crate::sync::open_docs;

/// A step against the library failed.
#[derive(Debug)]
pub enum ReplicaError {
    Runtime(istmo::IstmoError),
    Library(crate::library::LibraryError),
    Doc(crate::doc::DocError),
    Io(std::io::Error),
}

impl fmt::Display for ReplicaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(err) => write!(f, "runtime: {err}"),
            Self::Library(err) => write!(f, "library: {err}"),
            Self::Doc(err) => write!(f, "doc: {err}"),
            Self::Io(err) => write!(f, "io: {err}"),
        }
    }
}

impl std::error::Error for ReplicaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Runtime(err) => Some(err),
            Self::Library(err) => Some(err),
            Self::Doc(err) => Some(err),
            Self::Io(err) => Some(err),
        }
    }
}

impl UserMessage for ReplicaError {
    fn user_message(&self) -> String {
        match self {
            Self::Runtime(_) => tr!("errors-library-runtime"),
            Self::Library(err) => err.user_message(),
            Self::Doc(err) => err.user_message(),
            Self::Io(err) => err.user_message(),
        }
    }
}

impl From<crate::library::LibraryError> for ReplicaError {
    fn from(err: crate::library::LibraryError) -> Self {
        Self::Library(err)
    }
}

impl From<crate::doc::DocError> for ReplicaError {
    fn from(err: crate::doc::DocError) -> Self {
        Self::Doc(err)
    }
}

impl From<std::io::Error> for ReplicaError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// File identity used to reuse a computed version.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

fn stamp(item: ItemId) -> Option<Stamp> {
    let meta = std::fs::metadata(bodies::doc_path(item)).ok()?;
    Some(Stamp {
        modified: meta.modified().ok(),
        len: meta.len(),
    })
}

/// Whether items of `kind` carry a document body of their own, as
/// opposed to a plugin snapshot only.
fn owns_doc(kind: &DocKind) -> bool {
    crate::plugins::kind_info(&kind.plugin_id).is_some_and(|info| info.body || info.records)
}

/// Library-backed [`Replica`].
#[derive(Default)]
pub struct LibraryReplica {
    /// Versions of stored documents, valid while their file is unchanged.
    versions: std::sync::Mutex<HashMap<ItemId, (Stamp, Version)>>,
    known: Vec<DocId>,
}

impl fmt::Debug for LibraryReplica {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibraryReplica")
            .field("cached", &self.versions().len())
            .finish_non_exhaustive()
    }
}

impl LibraryReplica {
    /// Documents listed by the last [`Replica::docs`] call.
    #[must_use]
    pub fn known_docs(&self) -> &[DocId] {
        &self.known
    }

    fn versions(&self) -> std::sync::MutexGuard<'_, HashMap<ItemId, (Stamp, Version)>> {
        self.versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[sismo::function("sync::replica::open_library")]
    fn open() -> Result<async_lock::MutexGuardArc<Library>, ReplicaError> {
        let handle = crate::library::shared_blocking()?;
        sismo::scope!("sync::replica::lock_wait");
        Ok(pollster::block_on(handle.lock_arc()))
    }

    fn version_of(&self, item: ItemId, lib: &Library) -> Option<Version> {
        if let Some(version) = with_live(item, Doc::version) {
            return Some(Version(version));
        }
        let Some(current) = stamp(item) else {
            // Created but never saved: the document exists and is empty.
            // Announcing it at the version every empty document shares
            // lets a peer hold it before the first stroke lands.
            let kind = &lib.index().item(item)?.kind;
            return owns_doc(kind).then(|| Version(Doc::default().version()));
        };
        if let Some((cached, version)) = self.versions().get(&item)
            && *cached == current
        {
            return Some(version.clone());
        }
        sismo::scope!("sync::replica::version_from_disk");
        let bytes = bodies::read_doc_bytes(item)?;
        let version = Version(Doc::from_snapshot(&bytes).ok()?.version());
        self.versions().insert(item, (current, version.clone()));
        Some(version)
    }
}

/// Run `f` on the document as an open board or face holds it, when
/// the item is on screen, without copying it.
#[sismo::function("sync::replica::with_live")]
fn with_live<T>(item: ItemId, f: impl FnOnce(&Doc) -> T) -> Option<T> {
    if let Some(board) = active_boards::get_item(item) {
        return Some(f(lock(&board).doc()));
    }
    open_docs::get(item).map(|doc| f(&open_docs::lock(&doc)))
}

impl Replica for LibraryReplica {
    type Error = ReplicaError;

    #[sismo::function("sync::replica::docs")]
    fn docs(&self) -> Vec<(DocId, Version)> {
        let Ok(lib) = Self::open() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for item in &lib.index().items {
            if let Some(version) = self.version_of(item.id, &lib) {
                out.push((DocId(item.id.0), version));
            }
        }
        sismo::counter!("sync.replica.items", lib.index().items.len());
        sismo::counter!("sync.replica.docs", out.len());
        out
    }

    #[sismo::function("sync::replica::doc_version")]
    fn doc_version(&self, doc: DocId) -> Option<Version> {
        let Ok(lib) = Self::open() else {
            return None;
        };
        self.version_of(ItemId(doc.0), &lib)
    }

    #[sismo::function("sync::replica::updates_since")]
    fn doc_updates_since(&self, doc: DocId, since: &Version) -> Result<Vec<u8>, ReplicaError> {
        let item = ItemId(doc.0);
        let updates = |doc: &Doc| {
            if doc.version() == since.0 {
                Ok(Vec::new())
            } else {
                doc.updates_since(&since.0)
            }
        };
        if let Some(updates) = with_live(item, updates) {
            return Ok(updates?);
        }
        match bodies::read_doc_bytes(item) {
            Some(bytes) => Ok(updates(&Doc::from_snapshot(&bytes)?)?),
            None => Ok(Vec::new()),
        }
    }

    #[sismo::function("sync::replica::snapshot")]
    fn doc_snapshot(&self, doc: DocId) -> Result<Vec<u8>, ReplicaError> {
        match with_live(ItemId(doc.0), Doc::snapshot_bytes) {
            Some(bytes) => Ok(bytes?),
            None => match bodies::read_doc_bytes(ItemId(doc.0)) {
                Some(bytes) => Ok(bytes),
                None => Ok(Doc::default().snapshot_bytes()?),
            },
        }
    }

    #[sismo::function("sync::replica::import")]
    fn doc_import(&mut self, doc: DocId, bytes: &[u8]) -> Result<(), ReplicaError> {
        let item = ItemId(doc.0);
        let boards = active_boards::for_item(item);
        let open = open_docs::get(item);
        let merged = if let Some(open) = open.filter(|_| boards.is_empty()) {
            let mut doc = open_docs::lock(&open);
            doc.import_updates(bytes)?;
            let merged = doc.clone();
            drop(doc);
            open_docs::notify(item);
            merged
        } else if boards.is_empty() {
            let mut merged = match bodies::read_doc_bytes(item) {
                Some(existing) => Doc::from_snapshot(&existing)?,
                None => Doc::from_snapshot(bytes)?,
            };
            merged.import_updates(bytes)?;
            merged
        } else {
            for (_, board) in &boards {
                lock(board).merge_updates(bytes)?;
            }
            lock(&boards[0].1).doc().clone()
        };
        let snapshot = {
            sismo::scope!("sync::replica::import_snapshot");
            merged.snapshot_bytes()?
        };
        {
            sismo::scope!("sync::replica::import_write");
            bodies::write_doc_bytes(item, &snapshot)?;
        }
        {
            sismo::scope!("sync::replica::import_refs");
            refs::record_doc(item, &merged);
        }
        self.versions().remove(&item);
        Ok(())
    }

    #[sismo::function("sync::replica::blob_heads")]
    fn blob_heads(&self) -> Vec<BlobHead> {
        let Ok(lib) = Self::open() else {
            return Vec::new();
        };
        lib.sync_heads(&Graveyard::load())
    }

    #[sismo::function("sync::replica::blobs")]
    fn blobs(&self, keys: &[BlobKey]) -> Vec<Blob> {
        let Ok(lib) = Self::open() else {
            return Vec::new();
        };
        let yard = Graveyard::load();
        let heads: HashMap<BlobKey, BlobHead> = lib
            .sync_heads(&yard)
            .into_iter()
            .map(|h| (h.key(), h))
            .collect();
        keys.iter()
            .filter_map(|key| {
                let head = *heads.get(key)?;
                let record = lib.sync_record(*key, &yard)?;
                Some(Blob {
                    head,
                    bytes: record.encode(),
                })
            })
            .collect()
    }

    #[sismo::function("sync::replica::blob_merge")]
    fn blob_merge(&mut self, blob: Blob) -> Result<bool, ReplicaError> {
        sismo::counter!("sync.in.blob_bytes", blob.bytes.len());
        let Some(record) = SyncRecord::decode(&blob.bytes) else {
            log::warn!("sync: undecodable record for {:?}", blob.head.key());
            return Ok(false);
        };
        let mut lib = Self::open()?;
        let mut yard = Graveyard::load();
        let applied = pollster::block_on(lib.apply_sync_record(blob.head, record, &mut yard))?;
        drop(lib);
        if applied {
            sismo::scope!("sync::replica::graveyard_save");
            yard.save()?;
        }
        Ok(applied)
    }
}

/// Keep the shared cache in step after a `docs` pass. Called by the
/// service with the list it just announced.
impl LibraryReplica {
    #[sismo::function("sync::replica::remember")]
    pub fn remember(&mut self, docs: &[(DocId, Version)]) {
        self.known = docs.iter().map(|(d, _)| *d).collect();
        for (doc, version) in docs {
            let item = ItemId(doc.0);
            if let Some(current) = stamp(item) {
                self.versions().insert(item, (current, version.clone()));
            }
        }
    }
}

/// Shared handle so the service and the UI thread can both reach it.
pub type SharedReplica = Arc<std::sync::Mutex<LibraryReplica>>;
