//! What a library must expose to take part in an exchange. Documents
//! are mergeable histories; everything else (item metadata, folders,
//! tags, links, mesas) is a blob where the newest write wins.

use std::collections::HashSet;
use std::fmt;

/// A document, named by the same bytes the library uses for its item.
#[derive(bincode::Encode, bincode::Decode, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocId(pub [u8; 16]);

impl fmt::Debug for DocId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DocId({})", hex::encode(&self.0[..4]))
    }
}

/// Opaque marker of everything a replica has seen of one document.
/// Only the document store knows how to read it.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Hash, Default)]
pub struct Version(pub Vec<u8>);

impl fmt::Debug for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Version({} bytes)", self.0.len())
    }
}

/// What a blob describes.
#[derive(
    bincode::Encode, bincode::Decode, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug,
)]
pub enum BlobKind {
    Item,
    Folder,
    Tag,
    Link,
    Workspace,
    /// A plugin's own state for a document, keyed by the document id;
    /// which plugin travels inside the record.
    Snapshot,
    /// A file a document links to, like an image. Keyed by the first 8
    /// bytes of the document id and 8 bytes naming the file; files are
    /// named after their content, so they never change.
    Asset,
    /// The file a document shows (a PDF), keyed by the document id.
    Attachment,
}

/// Identity plus freshness of a blob; enough to decide who is newer.
#[derive(bincode::Encode, bincode::Decode, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct BlobHead {
    pub kind: BlobKind,
    pub id: [u8; 16],
    /// Unix milliseconds of the last write.
    pub updated_at: u64,
    /// The record is a deletion marker, not content. Carried in the
    /// head so a peer can refuse deletions by policy without opening
    /// the blob.
    pub gone: bool,
}

impl BlobHead {
    #[must_use]
    pub const fn key(&self) -> BlobKey {
        BlobKey {
            kind: self.kind,
            id: self.id,
        }
    }
}

/// A blob without its freshness.
#[derive(
    bincode::Encode, bincode::Decode, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug,
)]
pub struct BlobKey {
    pub kind: BlobKind,
    pub id: [u8; 16],
}

/// A blob with its encoded body. Tombstones are the body's business:
/// a deleted record still travels, flagged inside its bytes.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub struct Blob {
    pub head: BlobHead,
    pub bytes: Vec<u8>,
}

/// Which part of the library an exchange covers.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum Scope {
    /// Everything: documents, folders, tags, links and mesas. Used
    /// between devices of the same user.
    #[default]
    All,
    /// A share: the listed documents and the blobs the owner
    /// explicitly put in (their item records, links whose both ends
    /// are inside, ...).
    Subset {
        docs: HashSet<DocId>,
        blobs: HashSet<BlobKey>,
    },
}

impl Scope {
    #[must_use]
    pub fn covers_doc(&self, doc: DocId) -> bool {
        match self {
            Self::All => true,
            Self::Subset { docs, .. } => docs.contains(&doc),
        }
    }

    #[must_use]
    pub fn covers_blob(&self, key: BlobKey) -> bool {
        match self {
            Self::All => true,
            Self::Subset { docs, blobs } => {
                blobs.contains(&key)
                    || (matches!(key.kind, BlobKind::Snapshot | BlobKind::Attachment)
                        && docs.contains(&DocId(key.id)))
                    || (key.kind == BlobKind::Asset
                        && docs.iter().any(|doc| doc.0[..8] == key.id[..8]))
            }
        }
    }
}

/// The library side of an exchange.
pub trait Replica {
    type Error: std::error::Error + 'static;

    /// Every document this replica holds, with what it has seen.
    fn docs(&self) -> Vec<(DocId, Version)>;

    /// Current marker for one document, `None` when absent.
    fn doc_version(&self, doc: DocId) -> Option<Version>;

    /// Changes a replica at `since` is missing. Empty when none.
    fn doc_updates_since(&self, doc: DocId, since: &Version) -> Result<Vec<u8>, Self::Error>;

    /// The whole document, for a replica that lacks it.
    fn doc_snapshot(&self, doc: DocId) -> Result<Vec<u8>, Self::Error>;

    /// Merge `bytes` (updates or a snapshot) into `doc`, creating it
    /// when absent.
    fn doc_import(&mut self, doc: DocId, bytes: &[u8]) -> Result<(), Self::Error>;

    /// Freshness of every blob this replica holds.
    fn blob_heads(&self) -> Vec<BlobHead>;

    /// Bodies for the given blobs; unknown keys are skipped.
    fn blobs(&self, keys: &[BlobKey]) -> Vec<Blob>;

    /// Keep `blob` if it is newer than what is held. Returns whether
    /// it was applied.
    fn blob_merge(&mut self, blob: Blob) -> Result<bool, Self::Error>;
}

/// A replica behind a lock, so several engines can share one library.
impl<R: Replica> Replica for std::sync::Arc<std::sync::Mutex<R>> {
    type Error = R::Error;

    #[cfg_attr(feature = "sismo", sismo::function("sync::replica::locked_docs"))]
    fn docs(&self) -> Vec<(DocId, Version)> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .docs()
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::replica::locked_doc_version"))]
    fn doc_version(&self, doc: DocId) -> Option<Version> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .doc_version(doc)
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::replica::locked_updates_since"))]
    fn doc_updates_since(&self, doc: DocId, since: &Version) -> Result<Vec<u8>, Self::Error> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .doc_updates_since(doc, since)
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::replica::locked_snapshot"))]
    fn doc_snapshot(&self, doc: DocId) -> Result<Vec<u8>, Self::Error> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .doc_snapshot(doc)
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::replica::locked_import"))]
    fn doc_import(&mut self, doc: DocId, bytes: &[u8]) -> Result<(), Self::Error> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .doc_import(doc, bytes)
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::replica::locked_blob_heads"))]
    fn blob_heads(&self) -> Vec<BlobHead> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .blob_heads()
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::replica::locked_blobs"))]
    fn blobs(&self, keys: &[BlobKey]) -> Vec<Blob> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .blobs(keys)
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::replica::locked_blob_merge"))]
    fn blob_merge(&mut self, blob: Blob) -> Result<bool, Self::Error> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .blob_merge(blob)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{BlobKey, BlobKind, DocId, Scope};

    #[test]
    fn a_share_covers_the_files_of_its_documents() {
        let doc = DocId([7; 16]);
        let scope = Scope::Subset {
            docs: HashSet::from([doc]),
            blobs: HashSet::new(),
        };
        let mut mine = [7; 16];
        mine[8..].copy_from_slice(&42u64.to_le_bytes());
        let mut other = mine;
        other[0] = 1;
        assert!(scope.covers_blob(BlobKey {
            kind: BlobKind::Asset,
            id: mine
        }));
        assert!(!scope.covers_blob(BlobKey {
            kind: BlobKind::Asset,
            id: other
        }));
    }
}
