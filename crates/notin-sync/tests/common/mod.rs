//! Shared by the integration tests.

use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;

use notin_sync::{Blob, BlobHead, BlobKey, DocId, Replica, Version};

/// Toy mergeable document: a set of `(author, counter)` marks.
#[derive(Default, Debug)]
pub(crate) struct MemReplica {
    pub(crate) docs: BTreeMap<DocId, BTreeSet<(u64, u64)>>,
    pub(crate) blobs: BTreeMap<BlobKey, Blob>,
}

fn encode<T: istmo_core::codec::Encode>(value: T) -> Vec<u8> {
    istmo_core::codec::encode(&value).unwrap()
}

fn decode<T: istmo_core::codec::Decode>(bytes: &[u8]) -> T {
    istmo_core::codec::decode(bytes).unwrap().0
}

impl Replica for MemReplica {
    type Error = Infallible;

    fn docs(&self) -> Vec<(DocId, Version)> {
        self.docs
            .keys()
            .map(|doc| (*doc, self.doc_version(*doc).unwrap()))
            .collect()
    }

    fn doc_version(&self, doc: DocId) -> Option<Version> {
        self.docs
            .get(&doc)
            .map(|marks| Version(encode(marks.iter().copied().collect::<Vec<_>>())))
    }

    fn doc_updates_since(&self, doc: DocId, since: &Version) -> Result<Vec<u8>, Infallible> {
        let seen: BTreeSet<(u64, u64)> = decode::<Vec<(u64, u64)>>(&since.0).into_iter().collect();
        let missing: Vec<(u64, u64)> = self.docs[&doc].difference(&seen).copied().collect();
        Ok(if missing.is_empty() {
            Vec::new()
        } else {
            encode(missing)
        })
    }

    fn doc_snapshot(&self, doc: DocId) -> Result<Vec<u8>, Infallible> {
        Ok(encode(self.docs[&doc].iter().copied().collect::<Vec<_>>()))
    }

    fn doc_import(&mut self, doc: DocId, bytes: &[u8]) -> Result<(), Infallible> {
        let marks: Vec<(u64, u64)> = decode(bytes);
        self.docs.entry(doc).or_default().extend(marks);
        Ok(())
    }

    fn blob_heads(&self) -> Vec<BlobHead> {
        self.blobs.values().map(|b| b.head).collect()
    }

    fn blobs(&self, keys: &[BlobKey]) -> Vec<Blob> {
        keys.iter()
            .filter_map(|k| self.blobs.get(k).cloned())
            .collect()
    }

    fn blob_merge(&mut self, blob: Blob) -> Result<bool, Infallible> {
        let key = blob.head.key();
        let newer = self
            .blobs
            .get(&key)
            .is_none_or(|held| held.head.updated_at < blob.head.updated_at);
        if newer {
            self.blobs.insert(key, blob);
        }
        Ok(newer)
    }
}
