//! The relay: a peer that is always on, so members of a share meet
//! even when they are never online at the same time.
//!
//! It keeps a mailbox per share. Members publish sealed records and
//! fetch what arrived since the last sequence number they saw. The
//! relay never holds a share key, so it stores and forwards bytes it
//! cannot read; the only things in the clear are the share id, the
//! document or record a payload concerns, who sent it and when.
//!
//! Session shares are forwarded but never written down.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use crate::identity::{DeviceId, DeviceKey};
use crate::net::lan::LanNode;
use crate::net::{LanConfig, NetError, NetEvent};
use crate::replica::{BlobKey, DocId};
use crate::share::ShareId;

/// Where a relay lives: `host:port/<public key hex>`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RelayAddr {
    pub host: String,
    pub public: [u8; 32],
}

impl RelayAddr {
    #[must_use]
    pub fn device(&self) -> DeviceId {
        DeviceId::of_public_key(&self.public)
    }
}

impl fmt::Display for RelayAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.host, hex::encode(self.public))
    }
}

/// The text was not `host:port/<64 hex>`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BadRelayAddr;

impl fmt::Display for BadRelayAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("relay address must be host:port/<public key>")
    }
}

impl std::error::Error for BadRelayAddr {}

impl FromStr for RelayAddr {
    type Err = BadRelayAddr;

    fn from_str(text: &str) -> Result<Self, BadRelayAddr> {
        let (host, key) = text.trim().rsplit_once('/').ok_or(BadRelayAddr)?;
        if host.is_empty() || !host.contains(':') {
            return Err(BadRelayAddr);
        }
        let bytes = hex::decode(key).map_err(|_| BadRelayAddr)?;
        let public = <[u8; 32]>::try_from(bytes).map_err(|_| BadRelayAddr)?;
        Ok(Self {
            host: host.to_owned(),
            public,
        })
    }
}

/// What a stored record concerns, in the clear.
#[derive(bincode::Encode, bincode::Decode, Clone, Copy, PartialEq, Eq, Debug)]
pub enum RecordKind {
    /// Changes to one document since the sender's previous record.
    Update { doc: DocId },
    /// The whole document; makes every earlier record for it moot.
    Snapshot { doc: DocId },
    /// One index record; replaces the earlier copy with the same key.
    Blob { key: BlobKey },
    /// The owner's view of the share; replaces the earlier one.
    Meta,
}

/// One mailbox entry.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub struct Record {
    pub seq: u64,
    pub from: DeviceId,
    pub kind: RecordKind,
    /// A [`crate::share::ShareMessage`] sealed with the share key.
    pub sealed: Vec<u8>,
}

/// What clients and the relay say to each other.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub enum RelayMessage {
    /// Owner declares (or refreshes) a share: who may use the
    /// mailbox and whether it is kept.
    Register {
        share: ShareId,
        members: Vec<[u8; 32]>,
        retain: bool,
    },
    /// Owner closes the share; the mailbox is deleted.
    Unregister { share: ShareId },
    /// Member asks for everything after `after` and for live records.
    Subscribe { share: ShareId, after: u64 },
    /// Member hands in a record.
    Publish {
        share: ShareId,
        kind: RecordKind,
        sealed: Vec<u8>,
    },
    /// Relay accepted the registration.
    Registered { share: ShareId },
    /// The sender may not use this share here.
    Refused { share: ShareId },
    /// Backlog or live records.
    Records {
        share: ShareId,
        records: Vec<Record>,
    },
    /// The share no longer exists on this relay.
    Gone { share: ShareId },
    /// Hand `bytes` to the connected device holding key `to`.
    Forward { to: [u8; 32], bytes: Vec<u8> },
    /// Bytes another device handed in for the receiver.
    Forwarded { from: [u8; 32], bytes: Vec<u8> },
}

impl RelayMessage {
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::encode_to_vec(self, bincode::config::standard()).unwrap_or_default()
    }

    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        bincode::decode_from_slice(bytes, bincode::config::standard())
            .ok()
            .map(|(message, _)| message)
    }
}

/// One share's mailbox.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub struct Mailbox {
    pub owner: [u8; 32],
    pub members: Vec<[u8; 32]>,
    pub retain: bool,
    pub next_seq: u64,
    pub records: Vec<Record>,
}

impl Mailbox {
    fn allows(&self, public: [u8; 32]) -> bool {
        self.owner == public || self.members.contains(&public)
    }

    /// Append a record, dropping whatever it makes moot.
    fn push(&mut self, from: DeviceId, kind: RecordKind, sealed: Vec<u8>) -> Record {
        let record = Record {
            seq: self.next_seq,
            from,
            kind,
            sealed,
        };
        self.next_seq += 1;
        if self.retain {
            match kind {
                RecordKind::Snapshot { doc } => self.records.retain(|r| {
                    !matches!(
                        r.kind,
                        RecordKind::Update { doc: d } | RecordKind::Snapshot { doc: d } if d == doc
                    )
                }),
                RecordKind::Blob { key } => self
                    .records
                    .retain(|r| !matches!(r.kind, RecordKind::Blob { key: k } if k == key)),
                RecordKind::Meta => self.records.retain(|r| !matches!(r.kind, RecordKind::Meta)),
                RecordKind::Update { .. } => {}
            }
            self.records.push(record.clone());
        }
        record
    }

    fn since(&self, after: u64) -> Vec<Record> {
        self.records
            .iter()
            .filter(|r| r.seq > after)
            .cloned()
            .collect()
    }
}

/// Every mailbox, optionally mirrored to disk.
#[derive(Debug, Default)]
pub struct Store {
    dir: Option<PathBuf>,
    boxes: HashMap<ShareId, Mailbox>,
}

impl Store {
    /// In memory only.
    #[must_use]
    pub fn memory() -> Self {
        Self::default()
    }

    /// Mirrored under `dir`, one file per share, loaded now.
    pub fn on_disk(dir: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let mut boxes = HashMap::new();
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(id) = hex::decode(stem) else {
                continue;
            };
            let Ok(id) = <[u8; 16]>::try_from(id) else {
                continue;
            };
            let bytes = std::fs::read(&path)?;
            if let Ok((mailbox, _)) =
                bincode::decode_from_slice::<Mailbox, _>(&bytes, bincode::config::standard())
            {
                boxes.insert(ShareId(id), mailbox);
            }
        }
        Ok(Self {
            dir: Some(dir),
            boxes,
        })
    }

    fn path(&self, share: ShareId) -> Option<PathBuf> {
        self.dir
            .as_ref()
            .map(|dir| dir.join(format!("{}.bin", hex::encode(share.0))))
    }

    fn flush(&self, share: ShareId) {
        let Some(path) = self.path(share) else {
            return;
        };
        match self.boxes.get(&share) {
            Some(mailbox) if mailbox.retain => {
                if let Ok(bytes) = bincode::encode_to_vec(mailbox, bincode::config::standard())
                    && let Err(err) = std::fs::write(&path, bytes)
                {
                    tracing::error!(target: "notein-relay", "write {}: {err}", path.display());
                }
            }
            _ => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    #[must_use]
    pub fn mailbox(&self, share: ShareId) -> Option<&Mailbox> {
        self.boxes.get(&share)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.boxes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }
}

/// How a relay binds.
#[derive(Clone, Debug)]
pub struct RelayConfig {
    pub key: DeviceKey,
    pub name: String,
    pub port: u16,
}

/// The running relay. Dropping it stops it.
#[derive(Debug)]
pub struct RelayServer {
    node: LanNode,
}

impl RelayServer {
    /// Bind and serve on a background thread.
    pub fn start(config: RelayConfig, store: Store) -> Result<Self, NetError> {
        let lan = LanConfig::new(config.key, config.name)
            .port(config.port)
            .advertise(false)
            .scan(false);
        let node = LanNode::start(lan)?;
        let events = node.events().clone();
        let sender = node.sender();
        std::thread::Builder::new()
            .name("notein-relay".into())
            .spawn(move || serve(&events, sender, store))
            .map_err(NetError::Thread)?;
        Ok(Self { node })
    }

    #[must_use]
    pub const fn local_addr(&self) -> std::net::SocketAddr {
        self.node.local_addr()
    }
}

struct Serving {
    sender: crate::net::lan::LanSender,
    store: Store,
    /// Public key of every connected device.
    keys: HashMap<DeviceId, [u8; 32]>,
    subs: HashMap<ShareId, HashSet<DeviceId>>,
}

impl Serving {
    fn reply(&self, to: DeviceId, message: &RelayMessage) {
        let bytes = crate::driver::relay_payload(message);
        if let Err(err) = self.sender.send(to, bytes) {
            tracing::warn!(target: "notein-relay", "reply: {err}");
        }
    }

    fn on_message(&mut self, from: DeviceId, message: RelayMessage) {
        let Some(public) = self.keys.get(&from).copied() else {
            return;
        };
        match message {
            RelayMessage::Register {
                share,
                members,
                retain,
            } => self.register(from, public, share, members, retain),
            RelayMessage::Unregister { share } => self.unregister(from, public, share),
            RelayMessage::Subscribe { share, after } => {
                let Some(mailbox) = self.store.boxes.get(&share) else {
                    self.reply(from, &RelayMessage::Gone { share });
                    return;
                };
                if !mailbox.allows(public) {
                    self.reply(from, &RelayMessage::Refused { share });
                    return;
                }
                let records = mailbox.since(after);
                self.subs.entry(share).or_default().insert(from);
                self.reply(from, &RelayMessage::Records { share, records });
            }
            RelayMessage::Publish {
                share,
                kind,
                sealed,
            } => self.publish(from, public, share, kind, sealed),
            RelayMessage::Forward { to, bytes } => {
                let target = self
                    .keys
                    .iter()
                    .find(|(_, key)| **key == to)
                    .map(|(device, _)| *device);
                if let Some(device) = target {
                    self.reply(
                        device,
                        &RelayMessage::Forwarded {
                            from: public,
                            bytes,
                        },
                    );
                }
            }
            RelayMessage::Registered { .. }
            | RelayMessage::Refused { .. }
            | RelayMessage::Records { .. }
            | RelayMessage::Gone { .. }
            | RelayMessage::Forwarded { .. } => {}
        }
    }

    /// Open the mailbox of `share`, or update it when `public` owns it.
    fn register(
        &mut self,
        from: DeviceId,
        public: [u8; 32],
        share: ShareId,
        members: Vec<[u8; 32]>,
        retain: bool,
    ) {
        let accepted = match self.store.boxes.get_mut(&share) {
            Some(mailbox) if mailbox.owner != public => false,
            Some(mailbox) => {
                mailbox.members = members;
                mailbox.retain = retain;
                true
            }
            None => {
                self.store.boxes.insert(
                    share,
                    Mailbox {
                        owner: public,
                        members,
                        retain,
                        next_seq: 1,
                        records: Vec::new(),
                    },
                );
                true
            }
        };
        if accepted {
            self.store.flush(share);
            self.reply(from, &RelayMessage::Registered { share });
        } else {
            self.reply(from, &RelayMessage::Refused { share });
        }
    }

    /// The owner closed `share`: drop its mailbox and tell the others.
    fn unregister(&mut self, from: DeviceId, public: [u8; 32], share: ShareId) {
        if self
            .store
            .boxes
            .get(&share)
            .is_none_or(|m| m.owner != public)
        {
            return;
        }
        self.store.boxes.remove(&share);
        self.store.flush(share);
        for device in self.subs.remove(&share).into_iter().flatten() {
            if device != from {
                self.reply(device, &RelayMessage::Gone { share });
            }
        }
    }

    /// Store a record and hand it to every other subscriber.
    fn publish(
        &mut self,
        from: DeviceId,
        public: [u8; 32],
        share: ShareId,
        kind: RecordKind,
        sealed: Vec<u8>,
    ) {
        let Some(mailbox) = self.store.boxes.get_mut(&share) else {
            self.reply(from, &RelayMessage::Gone { share });
            return;
        };
        if !mailbox.allows(public) {
            self.reply(from, &RelayMessage::Refused { share });
            return;
        }
        let record = mailbox.push(from, kind, sealed);
        self.store.flush(share);
        let listeners: Vec<DeviceId> = self
            .subs
            .get(&share)
            .map(|s| s.iter().copied().filter(|d| *d != from).collect())
            .unwrap_or_default();
        for device in listeners {
            self.reply(
                device,
                &RelayMessage::Records {
                    share,
                    records: vec![record.clone()],
                },
            );
        }
    }
}

fn serve(events: &flume::Receiver<NetEvent>, sender: crate::net::lan::LanSender, store: Store) {
    let mut serving = Serving {
        sender,
        store,
        keys: HashMap::new(),
        subs: HashMap::new(),
    };
    while let Ok(event) = events.recv() {
        match event {
            NetEvent::Connected(peer) => {
                serving.keys.insert(peer.device, peer.public);
            }
            NetEvent::Disconnected(device) => {
                serving.keys.remove(&device);
                for subs in serving.subs.values_mut() {
                    subs.remove(&device);
                }
            }
            NetEvent::Message { from, bytes } => {
                if let Some(message) = crate::driver::relay_message(&bytes) {
                    serving.on_message(from, message);
                }
            }
            NetEvent::Discovered(_) | NetEvent::ConnectFailed { .. } => {}
            NetEvent::Stopped(reason) => {
                tracing::error!(target: "notein-relay", "transport stopped: {reason}");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: ShareId = ShareId([5; 16]);
    const A: DeviceId = DeviceId([1; 32]);
    const D: DocId = DocId([2; 16]);

    #[test]
    fn relay_addr_round_trips() {
        let addr = RelayAddr {
            host: "relay.example.org:7720".into(),
            public: [7; 32],
        };
        assert_eq!(addr.to_string().parse::<RelayAddr>().unwrap(), addr);
        assert!("nohost/abcd".parse::<RelayAddr>().is_err());
        assert!("host:1/zz".parse::<RelayAddr>().is_err());
    }

    #[test]
    fn snapshots_make_earlier_updates_moot() {
        let mut mailbox = Mailbox {
            owner: [0; 32],
            members: Vec::new(),
            retain: true,
            next_seq: 1,
            records: Vec::new(),
        };
        mailbox.push(A, RecordKind::Update { doc: D }, vec![1]);
        mailbox.push(A, RecordKind::Update { doc: D }, vec![2]);
        mailbox.push(A, RecordKind::Meta, vec![3]);
        mailbox.push(A, RecordKind::Snapshot { doc: D }, vec![4]);
        mailbox.push(A, RecordKind::Meta, vec![5]);
        let seqs: Vec<u64> = mailbox.records.iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![4, 5]);
        assert_eq!(mailbox.since(4).len(), 1);
        assert_eq!(mailbox.next_seq, 6);
    }

    #[test]
    fn session_mailboxes_keep_nothing() {
        let mut mailbox = Mailbox {
            owner: [0; 32],
            members: Vec::new(),
            retain: false,
            next_seq: 1,
            records: Vec::new(),
        };
        let record = mailbox.push(A, RecordKind::Update { doc: D }, vec![1]);
        assert_eq!(record.seq, 1);
        assert!(mailbox.records.is_empty());
    }

    #[test]
    fn store_round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("notein-relay-{}", rand::random::<u64>()));
        let mut store = Store::on_disk(dir.clone()).unwrap();
        store.boxes.insert(
            S,
            Mailbox {
                owner: [1; 32],
                members: vec![[2; 32]],
                retain: true,
                next_seq: 3,
                records: vec![Record {
                    seq: 2,
                    from: A,
                    kind: RecordKind::Meta,
                    sealed: vec![9],
                }],
            },
        );
        store.flush(S);
        let again = Store::on_disk(dir.clone()).unwrap();
        assert_eq!(again.mailbox(S), store.mailbox(S));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
