//! The exchange itself.
//!
//! Each side periodically announces what it has; the other answers
//! with whatever the announcer is missing. Nothing is assumed
//! delivered: a lost answer is simply re-sent after the next
//! announcement, and every import is idempotent.

use std::collections::HashMap;

use crate::error::SyncError;
use crate::identity::DeviceId;
use crate::replica::{Blob, BlobHead, BlobKey, DocId, Replica, Scope, Version};

/// Bumped whenever [`Message`] changes shape.
pub const PROTOCOL_VERSION: u16 = 5;

/// One unit of the exchange.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub enum Message {
    /// "This is what I have." Sent at the start of every round.
    Inventory {
        docs: Vec<(DocId, Version)>,
        blobs: Vec<BlobHead>,
    },
    /// Changes the receiver's announced version lacks.
    DocUpdates { doc: DocId, bytes: Vec<u8> },
    /// A document the receiver did not announce at all.
    DocSnapshot { doc: DocId, bytes: Vec<u8> },
    /// Blobs newer than the receiver's announced heads.
    Blobs(Vec<Blob>),
}

#[derive(bincode::Encode, bincode::Decode)]
struct Framed {
    version: u16,
    message: Message,
}

impl Message {
    /// Wire bytes, prefixed with [`PROTOCOL_VERSION`].
    pub fn to_bytes<E>(&self) -> Result<Vec<u8>, SyncError<E>> {
        bincode::encode_to_vec(
            Framed {
                version: PROTOCOL_VERSION,
                message: self.clone(),
            },
            bincode::config::standard(),
        )
        .map_err(|err| SyncError::Codec(err.to_string()))
    }

    /// Reverse of [`Self::to_bytes`].
    pub fn from_bytes<E>(bytes: &[u8]) -> Result<Self, SyncError<E>> {
        let (framed, _): (Framed, usize) =
            bincode::decode_from_slice(bytes, bincode::config::standard())
                .map_err(|err| SyncError::Codec(err.to_string()))?;
        if framed.version != PROTOCOL_VERSION {
            return Err(SyncError::Version(framed.version));
        }
        Ok(framed.message)
    }
}

/// How one document stands against the peers heard from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DocStatus {
    /// No peer has announced this document yet.
    Unknown,
    /// Every peer heard from announced the same version held here.
    Synced,
    /// At least one peer is at a different version.
    Pending,
}

/// What applying one message did.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Handled {
    /// Messages to send back to the peer.
    pub replies: Vec<Message>,
    /// Documents that received new content.
    pub imported: Vec<DocId>,
    /// Blobs that were replaced by a newer copy.
    pub merged: Vec<BlobKey>,
}

/// The last inventory a peer announced.
#[derive(Debug, Default)]
struct Heard {
    docs: HashMap<DocId, Version>,
    blobs: HashMap<BlobKey, u64>,
}

/// One side of an exchange over one [`Scope`].
#[derive(Debug)]
pub struct SyncEngine<R> {
    replica: R,
    scope: Scope,
    heard: HashMap<DeviceId, Heard>,
}

impl<R: Replica> SyncEngine<R> {
    #[must_use]
    pub fn new(replica: R, scope: Scope) -> Self {
        Self {
            replica,
            scope,
            heard: HashMap::new(),
        }
    }

    #[must_use]
    pub const fn replica(&self) -> &R {
        &self.replica
    }

    pub const fn replica_mut(&mut self) -> &mut R {
        &mut self.replica
    }

    #[must_use]
    pub const fn scope(&self) -> &Scope {
        &self.scope
    }

    pub fn set_scope(&mut self, scope: Scope) {
        self.scope = scope;
    }

    /// Forget everything heard from `peer`.
    pub fn forget_peer(&mut self, peer: DeviceId) {
        self.heard.remove(&peer);
    }

    /// The announcement that opens a round.
    #[must_use]
    pub fn inventory(&self) -> Message {
        let docs = self
            .replica
            .docs()
            .into_iter()
            .filter(|(doc, _)| self.scope.covers_doc(*doc))
            .collect();
        let blobs = self
            .replica
            .blob_heads()
            .into_iter()
            .filter(|head| self.scope.covers_blob(head.key()))
            .collect();
        Message::Inventory { docs, blobs }
    }

    /// Apply a message from `peer`.
    pub fn handle(
        &mut self,
        peer: DeviceId,
        message: Message,
    ) -> Result<Handled, SyncError<R::Error>> {
        let mut handled = Handled::default();
        match message {
            Message::Inventory { docs, blobs } => {
                handled.replies = self.answer_inventory(peer, docs, &blobs)?;
            }
            Message::DocUpdates { doc, bytes } | Message::DocSnapshot { doc, bytes } => {
                if self.scope.covers_doc(doc) {
                    self.replica
                        .doc_import(doc, &bytes)
                        .map_err(SyncError::Replica)?;
                    handled.imported.push(doc);
                }
            }
            Message::Blobs(blobs) => {
                for blob in blobs {
                    let key = blob.head.key();
                    if self.scope.covers_blob(key)
                        && self.replica.blob_merge(blob).map_err(SyncError::Replica)?
                    {
                        handled.merged.push(key);
                    }
                }
            }
        }
        Ok(handled)
    }

    fn answer_inventory(
        &mut self,
        peer: DeviceId,
        remote_docs: Vec<(DocId, Version)>,
        remote_blobs: &[BlobHead],
    ) -> Result<Vec<Message>, SyncError<R::Error>> {
        self.heard.insert(
            peer,
            Heard {
                docs: remote_docs.into_iter().collect(),
                blobs: remote_blobs
                    .iter()
                    .map(|h| (h.key(), h.updated_at))
                    .collect(),
            },
        );
        self.push_for(peer)
    }

    /// Everything `peer` lacks according to its last announcement.
    /// Empty when the peer never announced. Send after local edits so
    /// they land without waiting for the peer's next round.
    pub fn push_for(&self, peer: DeviceId) -> Result<Vec<Message>, SyncError<R::Error>> {
        let Some(heard) = self.heard.get(&peer) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        let remote = &heard.docs;
        for (doc, local) in self.replica.docs() {
            if !self.scope.covers_doc(doc) {
                continue;
            }
            match remote.get(&doc) {
                Some(theirs) if *theirs == local => {}
                Some(theirs) => match self.replica.doc_updates_since(doc, theirs) {
                    Ok(bytes) if bytes.is_empty() => {}
                    Ok(bytes) => out.push(Message::DocUpdates { doc, bytes }),
                    // A version this replica can no longer diff against
                    // (history trimmed, foreign marker): the whole document
                    // still gets the peer there.
                    Err(_) => {
                        let bytes = self.replica.doc_snapshot(doc).map_err(SyncError::Replica)?;
                        out.push(Message::DocSnapshot { doc, bytes });
                    }
                },
                None => {
                    let bytes = self.replica.doc_snapshot(doc).map_err(SyncError::Replica)?;
                    out.push(Message::DocSnapshot { doc, bytes });
                }
            }
        }
        let theirs = &heard.blobs;
        let newer: Vec<BlobKey> = self
            .replica
            .blob_heads()
            .into_iter()
            .filter(|head| self.scope.covers_blob(head.key()))
            .filter(|head| theirs.get(&head.key()).is_none_or(|&t| t < head.updated_at))
            .map(|head| head.key())
            .collect();
        if !newer.is_empty() {
            let blobs = self.replica.blobs(&newer);
            if !blobs.is_empty() {
                out.push(Message::Blobs(blobs));
            }
        }
        Ok(out)
    }

    /// New changes of `doc` for `peer` since what it last announced, and
    /// a note that it now holds them, so a burst of edits does not resend
    /// the same bytes. `None` when `peer` never announced `doc` (the
    /// next round covers it) or already has everything.
    ///
    /// # Errors
    /// Replica failure while exporting.
    pub fn push_doc(
        &mut self,
        peer: DeviceId,
        doc: DocId,
    ) -> Result<Option<Message>, SyncError<R::Error>> {
        if !self.scope.covers_doc(doc) {
            return Ok(None);
        }
        let Some(theirs) = self
            .heard
            .get(&peer)
            .and_then(|h| h.docs.get(&doc))
            .cloned()
        else {
            return Ok(None);
        };
        let Some(mine) = self.replica.doc_version(doc) else {
            return Ok(None);
        };
        if mine == theirs {
            return Ok(None);
        }
        let message = match self.replica.doc_updates_since(doc, &theirs) {
            Ok(bytes) if bytes.is_empty() => return Ok(None),
            Ok(bytes) => Message::DocUpdates { doc, bytes },
            Err(_) => Message::DocSnapshot {
                doc,
                bytes: self.replica.doc_snapshot(doc).map_err(SyncError::Replica)?,
            },
        };
        if let Some(heard) = self.heard.get_mut(&peer) {
            heard.docs.insert(doc, mine);
        }
        Ok(Some(message))
    }

    /// The blob under `key` for `peer` when it announced an older one,
    /// and a note that it now holds this one. `None` when `peer` never
    /// announced or already has it.
    ///
    /// # Errors
    /// Replica failure while reading.
    pub fn push_blob(
        &mut self,
        peer: DeviceId,
        key: BlobKey,
    ) -> Result<Option<Message>, SyncError<R::Error>> {
        if !self.scope.covers_blob(key) || !self.heard.contains_key(&peer) {
            return Ok(None);
        }
        let Some(head) = self
            .replica
            .blob_heads()
            .into_iter()
            .find(|h| h.key() == key)
        else {
            return Ok(None);
        };
        let stale = self
            .heard
            .get(&peer)
            .and_then(|h| h.blobs.get(&key))
            .is_none_or(|&theirs| theirs < head.updated_at);
        if !stale {
            return Ok(None);
        }
        let blobs = self.replica.blobs(&[key]);
        if blobs.is_empty() {
            return Ok(None);
        }
        if let Some(heard) = self.heard.get_mut(&peer) {
            heard.blobs.insert(key, head.updated_at);
        }
        Ok(Some(Message::Blobs(blobs)))
    }

    /// Where `doc` stands against every peer heard from.
    #[must_use]
    pub fn status(&self, doc: DocId) -> DocStatus {
        let Some(local) = self.replica.doc_version(doc) else {
            return DocStatus::Unknown;
        };
        let mut heard_any = false;
        for heard in self.heard.values() {
            let Some(theirs) = heard.docs.get(&doc) else {
                continue;
            };
            heard_any = true;
            if *theirs != local {
                return DocStatus::Pending;
            }
        }
        if heard_any {
            DocStatus::Synced
        } else {
            DocStatus::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::convert::Infallible;

    use super::*;
    use crate::replica::BlobKind;
    use crate::testing::{MemReplica, encode};

    const A: DeviceId = DeviceId([1; 32]);
    const B: DeviceId = DeviceId([2; 32]);
    const D1: DocId = DocId([1; 16]);
    const D2: DocId = DocId([2; 16]);

    /// One full round: both announce, both answer, both apply.
    fn round(a: &mut SyncEngine<MemReplica>, b: &mut SyncEngine<MemReplica>) {
        let from_a = a.inventory();
        let from_b = b.inventory();
        let wire = |m: &Message| {
            Message::from_bytes::<Infallible>(&m.to_bytes::<Infallible>().unwrap()).unwrap()
        };
        for reply in b.handle(A, wire(&from_a)).unwrap().replies {
            assert!(a.handle(B, wire(&reply)).unwrap().replies.is_empty());
        }
        for reply in a.handle(B, wire(&from_b)).unwrap().replies {
            assert!(b.handle(A, wire(&reply)).unwrap().replies.is_empty());
        }
    }

    fn marks(engine: &SyncEngine<MemReplica>, doc: DocId) -> Vec<(u64, u64)> {
        engine.replica().docs[&doc].iter().copied().collect()
    }

    #[test]
    fn two_replicas_converge_after_one_round() {
        let mut a = SyncEngine::new(MemReplica::default(), Scope::All);
        let mut b = SyncEngine::new(MemReplica::default(), Scope::All);
        a.replica_mut().mark(D1, 1, 1);
        a.replica_mut().mark(D1, 1, 2);
        b.replica_mut().mark(D1, 2, 1);
        b.replica_mut().mark(D2, 2, 1);

        round(&mut a, &mut b);

        assert_eq!(marks(&a, D1), vec![(1, 1), (1, 2), (2, 1)]);
        assert_eq!(marks(&b, D1), marks(&a, D1));
        assert_eq!(marks(&a, D2), vec![(2, 1)]);
        assert_eq!(a.status(D1), DocStatus::Pending);
        round(&mut a, &mut b);
        assert_eq!(a.status(D1), DocStatus::Synced);
        assert_eq!(b.status(D2), DocStatus::Synced);
    }

    #[test]
    fn nothing_is_sent_when_both_sides_agree() {
        let mut a = SyncEngine::new(MemReplica::default(), Scope::All);
        let mut b = SyncEngine::new(MemReplica::default(), Scope::All);
        a.replica_mut().mark(D1, 1, 1);
        b.replica_mut().mark(D1, 1, 1);
        let handled = b.handle(A, a.inventory()).unwrap();
        assert_eq!(handled, Handled::default());
    }

    #[test]
    fn lost_reply_is_repaired_by_the_next_round() {
        let mut a = SyncEngine::new(MemReplica::default(), Scope::All);
        let mut b = SyncEngine::new(MemReplica::default(), Scope::All);
        a.replica_mut().mark(D1, 1, 1);
        // B answers A's inventory but the reply never arrives.
        let _dropped = b.handle(A, a.inventory()).unwrap();
        // A answers B's inventory; B has nothing yet so it gets a snapshot.
        for reply in a.handle(B, b.inventory()).unwrap().replies {
            assert_eq!(b.handle(A, reply).unwrap().imported, vec![D1]);
        }
        assert_eq!(marks(&b, D1), vec![(1, 1)]);
        round(&mut a, &mut b);
        assert_eq!(a.status(D1), DocStatus::Synced);
    }

    #[test]
    fn newest_blob_wins_in_both_directions() {
        let mut a = SyncEngine::new(MemReplica::default(), Scope::All);
        let mut b = SyncEngine::new(MemReplica::default(), Scope::All);
        a.replica_mut().put_blob(BlobKind::Item, 1, 10, "a-old");
        b.replica_mut().put_blob(BlobKind::Item, 1, 20, "b-new");
        a.replica_mut().put_blob(BlobKind::Link, 2, 30, "a-new");
        b.replica_mut().put_blob(BlobKind::Link, 2, 5, "b-old");
        b.replica_mut()
            .put_blob(BlobKind::Workspace, 3, 1, "only-b");

        round(&mut a, &mut b);

        for engine in [&a, &b] {
            let r = engine.replica();
            assert_eq!(r.blob_text(BlobKind::Item, 1).as_deref(), Some("b-new"));
            assert_eq!(r.blob_text(BlobKind::Link, 2).as_deref(), Some("a-new"));
            assert_eq!(
                r.blob_text(BlobKind::Workspace, 3).as_deref(),
                Some("only-b")
            );
        }
    }

    #[test]
    fn subset_scope_keeps_other_docs_and_blobs_home() {
        let scope = Scope::Subset {
            docs: HashSet::from([D1]),
            blobs: HashSet::from([BlobKey {
                kind: BlobKind::Item,
                id: [1; 16],
            }]),
        };
        let mut owner = SyncEngine::new(MemReplica::default(), scope.clone());
        let mut guest = SyncEngine::new(MemReplica::default(), scope);
        owner.replica_mut().mark(D1, 1, 1);
        owner.replica_mut().mark(D2, 1, 1);
        owner.replica_mut().put_blob(BlobKind::Item, 1, 1, "shared");
        owner
            .replica_mut()
            .put_blob(BlobKind::Item, 2, 1, "private");
        guest.replica_mut().mark(D2, 2, 7);

        round(&mut owner, &mut guest);

        assert_eq!(marks(&guest, D1), vec![(1, 1)]);
        assert!(!guest.replica().docs.contains_key(&D2) || marks(&guest, D2) == vec![(2, 7)]);
        assert_eq!(marks(&owner, D2), vec![(1, 1)]);
        assert_eq!(
            guest.replica().blob_text(BlobKind::Item, 1).as_deref(),
            Some("shared")
        );
        assert_eq!(guest.replica().blob_text(BlobKind::Item, 2), None);
    }

    #[test]
    fn out_of_scope_payloads_are_ignored_even_if_pushed() {
        let scope = Scope::Subset {
            docs: HashSet::from([D1]),
            blobs: HashSet::new(),
        };
        let mut guest = SyncEngine::new(MemReplica::default(), scope);
        let handled = guest
            .handle(
                A,
                Message::DocSnapshot {
                    doc: D2,
                    bytes: encode(vec![(1u64, 1u64)]),
                },
            )
            .unwrap();
        assert_eq!(handled, Handled::default());
        assert!(!guest.replica().docs.contains_key(&D2));
    }

    #[test]
    fn wire_rejects_other_protocol_versions() {
        let framed = Framed {
            version: PROTOCOL_VERSION + 1,
            message: Message::Blobs(Vec::new()),
        };
        let bytes = encode(framed);
        assert!(matches!(
            Message::from_bytes::<Infallible>(&bytes),
            Err(SyncError::Version(v)) if v == PROTOCOL_VERSION + 1
        ));
    }
}
