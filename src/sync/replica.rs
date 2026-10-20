//! The library seen as a replica: documents on disk plus the index
//! records, read fresh on every call so the service never holds a
//! stale copy of what the UI is editing.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::SystemTime;

use notein_sync::{Blob, BlobHead, BlobKey, DocId, Replica, Version};

use crate::active_boards;
use crate::canvas::lock;
use crate::doc::Doc;
use crate::library::graveyard::Graveyard;
use crate::library::sync::SyncRecord;
use crate::library::{ItemId, Library, bodies, refs};

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

/// Library-backed [`Replica`].
#[derive(Default)]
pub struct LibraryReplica {
    versions: HashMap<ItemId, (Stamp, Version)>,
    known: Vec<DocId>,
}

impl fmt::Debug for LibraryReplica {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LibraryReplica")
            .field("cached", &self.versions.len())
            .finish_non_exhaustive()
    }
}

impl LibraryReplica {
    /// Documents listed by the last [`Replica::docs`] call.
    #[must_use]
    pub fn known_docs(&self) -> &[DocId] {
        &self.known
    }

    fn open(&self) -> Result<Library, ReplicaError> {
        let runtime = istmo::Runtime::global().map_err(ReplicaError::Runtime)?;
        pollster::block_on(Library::open(&runtime)).map_err(ReplicaError::Library)
    }

    fn version_of(&mut self, item: ItemId) -> Option<Version> {
        let current = stamp(item)?;
        if let Some((cached, version)) = self.versions.get(&item)
            && *cached == current
        {
            return Some(version.clone());
        }
        let bytes = bodies::read_doc_bytes(item)?;
        let version = match live_doc(item) {
            Some(doc) => Version(doc.version()),
            None => Version(Doc::from_snapshot(&bytes).ok()?.version()),
        };
        self.versions.insert(item, (current, version.clone()));
        Some(version)
    }
}

/// The document as an open board holds it, when the item is on screen.
fn live_doc(item: ItemId) -> Option<Doc> {
    active_boards::get_item(item).map(|board| lock(&board).doc().clone())
}

fn load_doc(item: ItemId) -> Result<Option<Doc>, ReplicaError> {
    if let Some(doc) = live_doc(item) {
        return Ok(Some(doc));
    }
    bodies::read_doc_bytes(item)
        .map(|bytes| Doc::from_snapshot(&bytes).map_err(ReplicaError::Doc))
        .transpose()
}

impl Replica for LibraryReplica {
    type Error = ReplicaError;

    fn docs(&self) -> Vec<(DocId, Version)> {
        // `docs` takes `&self`; the cache refresh happens through
        // interior calls on a scratch copy so the trait stays simple.
        let Ok(lib) = self.open() else {
            return Vec::new();
        };
        let mut scratch = Self {
            versions: self.versions.clone(),
            known: Vec::new(),
        };
        let mut out = Vec::new();
        for item in &lib.index().items {
            if let Some(version) = scratch.version_of(item.id) {
                out.push((DocId(item.id.0), version));
            }
        }
        out
    }

    fn doc_version(&self, doc: DocId) -> Option<Version> {
        let mut scratch = Self {
            versions: self.versions.clone(),
            known: Vec::new(),
        };
        scratch.version_of(ItemId(doc.0))
    }

    fn doc_updates_since(&self, doc: DocId, since: &Version) -> Result<Vec<u8>, ReplicaError> {
        let Some(doc) = load_doc(ItemId(doc.0))? else {
            return Ok(Vec::new());
        };
        if doc.version() == since.0 {
            return Ok(Vec::new());
        }
        Ok(doc.updates_since(&since.0)?)
    }

    fn doc_snapshot(&self, doc: DocId) -> Result<Vec<u8>, ReplicaError> {
        match live_doc(ItemId(doc.0)) {
            Some(doc) => Ok(doc.snapshot_bytes()?),
            None => Ok(bodies::read_doc_bytes(ItemId(doc.0)).unwrap_or_default()),
        }
    }

    fn doc_import(&mut self, doc: DocId, bytes: &[u8]) -> Result<(), ReplicaError> {
        let item = ItemId(doc.0);
        let boards = active_boards::for_item(item);
        let merged = if boards.is_empty() {
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
        let snapshot = merged.snapshot_bytes()?;
        bodies::write_doc_bytes(item, &snapshot)?;
        refs::record_doc(item, &merged);
        self.versions.remove(&item);
        Ok(())
    }

    fn blob_heads(&self) -> Vec<BlobHead> {
        let Ok(lib) = self.open() else {
            return Vec::new();
        };
        lib.sync_heads(&Graveyard::load())
    }

    fn blobs(&self, keys: &[BlobKey]) -> Vec<Blob> {
        let Ok(lib) = self.open() else {
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

    fn blob_merge(&mut self, blob: Blob) -> Result<bool, ReplicaError> {
        let Some(record) = SyncRecord::decode(&blob.bytes) else {
            log::warn!("sync: undecodable record for {:?}", blob.head.key());
            return Ok(false);
        };
        let mut lib = self.open()?;
        let mut yard = Graveyard::load();
        let applied = pollster::block_on(lib.apply_sync_record(blob.head, record, &mut yard))?;
        if applied {
            yard.save()?;
        }
        Ok(applied)
    }
}

/// Keep the shared cache in step after a `docs` pass. Called by the
/// service with the list it just announced.
impl LibraryReplica {
    pub fn remember(&mut self, docs: &[(DocId, Version)]) {
        self.known = docs.iter().map(|(d, _)| *d).collect();
        for (doc, version) in docs {
            let item = ItemId(doc.0);
            if let Some(current) = stamp(item) {
                self.versions.insert(item, (current, version.clone()));
            }
        }
    }
}

/// Shared handle so the service and the UI thread can both reach it.
pub type SharedReplica = Arc<std::sync::Mutex<LibraryReplica>>;
