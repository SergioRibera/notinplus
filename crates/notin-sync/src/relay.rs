//! The relay: a peer that is always on, so devices meet when they are
//! not on the same network, and members of a share meet even when they
//! are never online at the same time.
//!
//! It keeps a mailbox per share. Members publish sealed records and
//! fetch what arrived since the last sequence number they saw. The
//! relay never holds a share key, so it stores and forwards bytes it
//! cannot read; the only things in the clear are the share id, the
//! document or record a payload concerns, who sent it and when.
//!
//! Session shares are forwarded but never written down, and a relay
//! started without retention ([`RelayLimits::retain`]) writes nothing
//! down at all.
//!
//! Anyone may run a relay in public, so a device only reaches another
//! one through it when the target agreed: a device the target trusts,
//! a fellow member of one of its shares, or one that knocked with a
//! [`KnockToken`] derived from a secret the relay never sees.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::{Duration, Instant};

use blake2::{Blake2s256, Digest};

use crate::cipher::ShareKey;
use crate::identity::{DeviceId, DeviceKey, PairingSecret};
use crate::net::lan::LanNode;
use crate::net::{LanConfig, Lane, NetError, NetEvent};
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

/// Lets a device reach another one at the relay. Derived from a secret
/// the relay never sees, so it can compare tokens but not make one.
#[derive(bincode::Encode, bincode::Decode, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KnockToken(pub [u8; 32]);

impl KnockToken {
    /// What a device holding the invitation to `share` knocks with.
    #[must_use]
    pub fn share(key: &ShareKey, share: ShareId) -> Self {
        Self::derive(b"notin-knock-share", &[key.as_bytes(), &share.0])
    }

    /// What a device holding a pairing link knocks with.
    #[must_use]
    pub fn pairing(secret: &PairingSecret) -> Self {
        Self::derive(b"notin-knock-pair", &[secret.as_bytes()])
    }

    fn derive(domain: &[u8], parts: &[&[u8]]) -> Self {
        let mut hasher = Blake2s256::new();
        hasher.update(domain);
        for part in parts {
            hasher.update(part);
        }
        Self(hasher.finalize().into())
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
    /// mailbox, whether it is kept, and what joiners knock with.
    Register {
        share: ShareId,
        members: Vec<[u8; 32]>,
        retain: bool,
        knock: KnockToken,
    },
    /// Owner closes the share; the mailbox is deleted.
    Unregister { share: ShareId },
    /// The keys the sender trusts; they may reach it here. Replaces the
    /// earlier list and lasts while the sender stays connected.
    Trust { keys: Vec<[u8; 32]> },
    /// The sender opens (or with `None` closes) a pairing offer: whoever
    /// knocks with `token` may reach it for a while.
    Expect { token: Option<KnockToken> },
    /// Ask to reach `to`. Let through for a while when `token` is what
    /// `to` expects or opens one of the shares it owns; never answered,
    /// so nobody learns whether `to` is here.
    Knock { to: [u8; 32], token: KnockToken },
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
    /// The last record was handed on but not kept: the share's mailbox
    /// (or the relay) is full, or the record too big.
    Full { share: ShareId },
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
    #[cfg_attr(feature = "sismo", sismo::function("sync::relay::encode"))]
    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::encode_to_vec(self, bincode::config::standard()).unwrap_or_default()
    }

    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("sync::relay::decode"))]
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
    pub knock: KnockToken,
    pub next_seq: u64,
    pub records: Vec<Record>,
}

impl Mailbox {
    fn allows(&self, public: [u8; 32]) -> bool {
        self.owner == public || self.members.contains(&public)
    }

    /// Owner and members.
    fn keys(&self) -> impl Iterator<Item = [u8; 32]> + '_ {
        std::iter::once(self.owner).chain(self.members.iter().copied())
    }

    /// Bytes of sealed records kept.
    fn bytes(&self) -> u64 {
        self.records.iter().map(|r| r.sealed.len() as u64).sum()
    }

    /// Append a record, dropping whatever it makes moot; kept only when
    /// `keep` (the mailbox retains and there is room).
    fn push(&mut self, from: DeviceId, kind: RecordKind, sealed: Vec<u8>, keep: bool) -> Record {
        let record = Record {
            seq: self.next_seq,
            from,
            kind,
            sealed,
        };
        self.next_seq += 1;
        if keep {
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

/// How much a relay takes on. The defaults suit a relay open to anyone.
#[derive(Clone, Debug)]
pub struct RelayLimits {
    /// Keep the mailboxes of permanent shares; off makes the relay a
    /// pure pass-through that writes nothing down.
    pub retain: bool,
    /// Shares registered on the relay at once.
    pub max_shares: usize,
    /// Shares one owner may register.
    pub max_shares_per_owner: usize,
    /// Members of one share.
    pub max_members: usize,
    /// Keys one device may declare it trusts.
    pub max_trusted: usize,
    /// Bytes of one sealed record or forwarded payload.
    pub max_record: usize,
    /// Bytes kept for one share.
    pub max_mailbox_bytes: u64,
    /// Bytes kept for every share together.
    pub max_retained_bytes: u64,
    /// Knocks let through and not expired yet.
    pub max_grants: usize,
    /// How long a knock lets a device through.
    pub grant_ttl: Duration,
}

impl Default for RelayLimits {
    fn default() -> Self {
        Self {
            retain: true,
            max_shares: 100_000,
            max_shares_per_owner: 256,
            max_members: 256,
            max_trusted: 64,
            max_record: 16 << 20,
            max_mailbox_bytes: 256 << 20,
            max_retained_bytes: 8 << 30,
            max_grants: 100_000,
            grant_ttl: Duration::from_secs(10 * 60),
        }
    }
}

/// Every mailbox, optionally mirrored to disk.
#[derive(Debug, Default)]
pub struct Store {
    dir: Option<PathBuf>,
    boxes: HashMap<ShareId, Mailbox>,
    /// Shares each key owns or is a member of.
    by_key: HashMap<[u8; 32], HashSet<ShareId>>,
    /// Bytes kept across every mailbox.
    retained: u64,
}

impl Store {
    /// In memory only.
    #[must_use]
    pub fn memory() -> Self {
        Self::default()
    }

    /// Mirrored under `dir`, one file per share, loaded now.
    #[cfg_attr(feature = "sismo", sismo::function("sync::relay_server::load"))]
    pub fn on_disk(dir: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let entries = std::fs::read_dir(&dir)?;
        let mut store = Self {
            dir: Some(dir),
            ..Self::default()
        };
        for entry in entries {
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
                store.insert(ShareId(id), mailbox);
            }
        }
        Ok(store)
    }

    fn insert(&mut self, id: ShareId, mailbox: Mailbox) {
        self.remove(id);
        for key in mailbox.keys() {
            self.by_key.entry(key).or_default().insert(id);
        }
        self.retained += mailbox.bytes();
        self.boxes.insert(id, mailbox);
    }

    fn remove(&mut self, id: ShareId) -> Option<Mailbox> {
        let mailbox = self.boxes.remove(&id)?;
        for key in mailbox.keys() {
            if let Some(ids) = self.by_key.get_mut(&key) {
                ids.remove(&id);
                if ids.is_empty() {
                    self.by_key.remove(&key);
                }
            }
        }
        self.retained = self.retained.saturating_sub(mailbox.bytes());
        Some(mailbox)
    }

    /// Whether `a` and `b` take part in one share.
    fn together(&self, a: [u8; 32], b: [u8; 32]) -> bool {
        self.by_key.get(&a).is_some_and(|ids| {
            ids.iter()
                .any(|id| self.boxes.get(id).is_some_and(|m| m.allows(b)))
        })
    }

    /// Whether `token` opens one of the shares `owner` owns.
    fn opens(&self, owner: [u8; 32], token: KnockToken) -> bool {
        self.by_key.get(&owner).is_some_and(|ids| {
            ids.iter().any(|id| {
                self.boxes
                    .get(id)
                    .is_some_and(|m| m.owner == owner && m.knock == token)
            })
        })
    }

    fn owned_by(&self, owner: [u8; 32]) -> usize {
        self.by_key.get(&owner).map_or(0, |ids| {
            ids.iter()
                .filter(|id| self.boxes.get(id).is_some_and(|m| m.owner == owner))
                .count()
        })
    }

    fn path(&self, share: ShareId) -> Option<PathBuf> {
        self.dir
            .as_ref()
            .map(|dir| dir.join(format!("{}.bin", hex::encode(share.0))))
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::relay_server::flush"))]
    fn flush(&self, share: ShareId) {
        let Some(path) = self.path(share) else {
            return;
        };
        match self.boxes.get(&share) {
            Some(mailbox) if mailbox.retain => {
                if let Ok(bytes) = bincode::encode_to_vec(mailbox, bincode::config::standard())
                    && let Err(err) = std::fs::write(&path, bytes)
                {
                    tracing::error!(target: "notin-relay", "write {}: {err}", path.display());
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
    pub limits: RelayLimits,
}

/// The running relay. Dropping it stops it.
#[derive(Debug)]
pub struct RelayServer {
    node: LanNode,
}

impl RelayServer {
    /// Bind and serve on a background thread.
    ///
    /// # Errors
    /// The socket could not be bound or a thread could not be spawned.
    pub fn start(config: RelayConfig, store: Store) -> Result<Self, NetError> {
        let lan = LanConfig::new(config.key, config.name)
            .port(config.port)
            .advertise(false)
            .scan(false);
        let node = LanNode::start(lan)?;
        let events = node.events().clone();
        let sender = node.sender();
        let limits = config.limits;
        std::thread::Builder::new()
            .name("notin-relay".into())
            .spawn(move || serve(&events, sender, store, limits))
            .map_err(NetError::Thread)?;
        Ok(Self { node })
    }

    #[must_use]
    pub const fn local_addr(&self) -> std::net::SocketAddr {
        self.node.local_addr()
    }
}

/// Who agreed to be reached by whom, besides sharing a mailbox.
#[derive(Default)]
struct Reach {
    /// Keys each connected device trusts.
    trusts: HashMap<[u8; 32], HashSet<[u8; 32]>>,
    /// Open pairing offer of each connected device.
    expects: HashMap<[u8; 32], KnockToken>,
    /// Pairs let through by a knock, until when.
    grants: HashMap<([u8; 32], [u8; 32]), Instant>,
}

impl Reach {
    /// A knock lets both ways through, so the pair is stored sorted.
    fn pair(a: [u8; 32], b: [u8; 32]) -> ([u8; 32], [u8; 32]) {
        if a <= b { (a, b) } else { (b, a) }
    }

    fn granted(&self, a: [u8; 32], b: [u8; 32]) -> bool {
        self.grants
            .get(&Self::pair(a, b))
            .is_some_and(|until| *until > Instant::now())
    }

    fn grant(&mut self, a: [u8; 32], b: [u8; 32], limits: &RelayLimits) {
        let now = Instant::now();
        if self.grants.len() >= limits.max_grants {
            self.grants.retain(|_, until| *until > now);
        }
        if self.grants.len() >= limits.max_grants {
            tracing::warn!(target: "notin-relay", "knock table full");
            return;
        }
        self.grants.insert(Self::pair(a, b), now + limits.grant_ttl);
    }

    /// `public` went away: what it declared goes with it.
    fn forget(&mut self, public: [u8; 32]) {
        self.trusts.remove(&public);
        self.expects.remove(&public);
    }
}

struct Serving {
    sender: crate::net::lan::LanSender,
    store: Store,
    limits: RelayLimits,
    reach: Reach,
    /// Public key of every connected device.
    keys: HashMap<DeviceId, [u8; 32]>,
    subs: HashMap<ShareId, HashSet<DeviceId>>,
}

impl Serving {
    fn reply(&self, to: DeviceId, message: &RelayMessage) {
        let bytes = crate::driver::relay_payload(message);
        if let Err(err) = self.sender.send(to, Lane::Data, bytes) {
            tracing::warn!(target: "notin-relay", "reply: {err}");
        }
    }

    /// Whether `to` agreed to be reached by `from`.
    fn allows(&self, from: [u8; 32], to: [u8; 32]) -> bool {
        self.reach.granted(from, to)
            || self
                .reach
                .trusts
                .get(&to)
                .is_some_and(|t| t.contains(&from))
            || self.store.together(from, to)
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::relay_server::on_message"))]
    fn on_message(&mut self, from: DeviceId, message: RelayMessage) {
        let Some(public) = self.keys.get(&from).copied() else {
            return;
        };
        match message {
            RelayMessage::Register {
                share,
                members,
                retain,
                knock,
            } => self.register(from, public, share, members, retain, knock),
            RelayMessage::Unregister { share } => self.unregister(from, public, share),
            RelayMessage::Trust { mut keys } => {
                keys.truncate(self.limits.max_trusted);
                self.reach.trusts.insert(public, keys.into_iter().collect());
            }
            RelayMessage::Expect { token } => match token {
                Some(token) => {
                    self.reach.expects.insert(public, token);
                }
                None => {
                    self.reach.expects.remove(&public);
                }
            },
            RelayMessage::Knock { to, token } => {
                if self.reach.expects.get(&to) == Some(&token) || self.store.opens(to, token) {
                    self.reach.grant(public, to, &self.limits);
                }
            }
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
            RelayMessage::Forward { to, bytes } => self.forward(public, to, bytes),
            RelayMessage::Registered { .. }
            | RelayMessage::Refused { .. }
            | RelayMessage::Full { .. }
            | RelayMessage::Records { .. }
            | RelayMessage::Gone { .. }
            | RelayMessage::Forwarded { .. } => {}
        }
    }

    /// Hand `bytes` to `to` when it agreed to hear from `public`.
    /// Dropped silently otherwise, the same as when `to` is away, so
    /// nobody learns who is here.
    fn forward(&self, public: [u8; 32], to: [u8; 32], bytes: Vec<u8>) {
        if bytes.len() > self.limits.max_record || !self.allows(public, to) {
            return;
        }
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

    /// Open the mailbox of `share`, or update it when `public` owns it.
    #[cfg_attr(feature = "sismo", sismo::function("sync::relay_server::register"))]
    fn register(
        &mut self,
        from: DeviceId,
        public: [u8; 32],
        share: ShareId,
        members: Vec<[u8; 32]>,
        retain: bool,
        knock: KnockToken,
    ) {
        let retain = retain && self.limits.retain;
        let accepted = members.len() <= self.limits.max_members
            && match self.store.boxes.get(&share) {
                Some(mailbox) if mailbox.owner != public => false,
                Some(_) => true,
                None => {
                    self.store.len() < self.limits.max_shares
                        && self.store.owned_by(public) < self.limits.max_shares_per_owner
                }
            };
        if !accepted {
            self.reply(from, &RelayMessage::Refused { share });
            return;
        }
        let mut mailbox = self.store.remove(share).unwrap_or(Mailbox {
            owner: public,
            members: Vec::new(),
            retain,
            knock,
            next_seq: 1,
            records: Vec::new(),
        });
        mailbox.members = members;
        mailbox.retain = retain;
        mailbox.knock = knock;
        if !retain {
            mailbox.records.clear();
        }
        self.store.insert(share, mailbox);
        self.store.flush(share);
        self.reply(from, &RelayMessage::Registered { share });
    }

    /// The owner closed `share`: drop its mailbox and tell the others.
    #[cfg_attr(feature = "sismo", sismo::function("sync::relay_server::unregister"))]
    fn unregister(&mut self, from: DeviceId, public: [u8; 32], share: ShareId) {
        if self
            .store
            .boxes
            .get(&share)
            .is_none_or(|m| m.owner != public)
        {
            return;
        }
        self.store.remove(share);
        self.store.flush(share);
        for device in self.subs.remove(&share).into_iter().flatten() {
            if device != from {
                self.reply(device, &RelayMessage::Gone { share });
            }
        }
    }

    /// Store a record when there is room and hand it to every other
    /// subscriber.
    #[cfg_attr(feature = "sismo", sismo::function("sync::relay_server::publish"))]
    fn publish(
        &mut self,
        from: DeviceId,
        public: [u8; 32],
        share: ShareId,
        kind: RecordKind,
        sealed: Vec<u8>,
    ) {
        let Some(mailbox) = self.store.boxes.get(&share) else {
            self.reply(from, &RelayMessage::Gone { share });
            return;
        };
        if !mailbox.allows(public) {
            self.reply(from, &RelayMessage::Refused { share });
            return;
        }
        if sealed.len() > self.limits.max_record {
            self.reply(from, &RelayMessage::Full { share });
            return;
        }
        let size = sealed.len() as u64;
        let room = mailbox.bytes() + size <= self.limits.max_mailbox_bytes
            && self.store.retained + size <= self.limits.max_retained_bytes;
        let keep = mailbox.retain && room;
        let Some(mut mailbox) = self.store.remove(share) else {
            return;
        };
        let record = mailbox.push(from, kind, sealed, keep);
        self.store.insert(share, mailbox);
        if keep {
            self.store.flush(share);
        } else if self.store.boxes.get(&share).is_some_and(|m| m.retain) {
            self.reply(from, &RelayMessage::Full { share });
        }
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

fn serve(
    events: &flume::Receiver<NetEvent>,
    sender: crate::net::lan::LanSender,
    store: Store,
    limits: RelayLimits,
) {
    let mut serving = Serving {
        sender,
        store,
        limits,
        reach: Reach::default(),
        keys: HashMap::new(),
        subs: HashMap::new(),
    };
    while let Ok(event) = events.recv() {
        match event {
            NetEvent::Connected(peer) => {
                serving.keys.insert(peer.device, peer.public);
            }
            NetEvent::Disconnected(device) => {
                if let Some(public) = serving.keys.remove(&device) {
                    serving.reach.forget(public);
                }
                for subs in serving.subs.values_mut() {
                    subs.remove(&device);
                }
            }
            NetEvent::Message { from, bytes } => {
                if let Some(message) = crate::driver::relay_message(&bytes) {
                    serving.on_message(from, message);
                }
            }
            NetEvent::Discovered(_) | NetEvent::Renamed(_) | NetEvent::ConnectFailed { .. } => {}
            NetEvent::Stopped(reason) => {
                tracing::error!(target: "notin-relay", "transport stopped: {reason}");
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
    const KNOCK: KnockToken = KnockToken([3; 32]);

    fn mailbox(owner: [u8; 32], members: Vec<[u8; 32]>, retain: bool) -> Mailbox {
        Mailbox {
            owner,
            members,
            retain,
            knock: KNOCK,
            next_seq: 1,
            records: Vec::new(),
        }
    }

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
        let mut mailbox = mailbox([0; 32], Vec::new(), true);
        mailbox.push(A, RecordKind::Update { doc: D }, vec![1], true);
        mailbox.push(A, RecordKind::Update { doc: D }, vec![2], true);
        mailbox.push(A, RecordKind::Meta, vec![3], true);
        mailbox.push(A, RecordKind::Snapshot { doc: D }, vec![4], true);
        mailbox.push(A, RecordKind::Meta, vec![5], true);
        let seqs: Vec<u64> = mailbox.records.iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![4, 5]);
        assert_eq!(mailbox.since(4).len(), 1);
        assert_eq!(mailbox.next_seq, 6);
    }

    #[test]
    fn records_not_kept_still_get_a_sequence() {
        let mut mailbox = mailbox([0; 32], Vec::new(), false);
        let record = mailbox.push(A, RecordKind::Update { doc: D }, vec![1], false);
        assert_eq!(record.seq, 1);
        assert!(mailbox.records.is_empty());
    }

    #[test]
    fn store_round_trips_through_disk() {
        let dir = std::env::temp_dir().join(format!("notin-relay-{}", rand::random::<u64>()));
        let mut store = Store::on_disk(dir.clone()).unwrap();
        let mut held = mailbox([1; 32], vec![[2; 32]], true);
        held.next_seq = 3;
        held.records.push(Record {
            seq: 2,
            from: A,
            kind: RecordKind::Meta,
            sealed: vec![9],
        });
        store.insert(S, held);
        store.flush(S);
        let again = Store::on_disk(dir.clone()).unwrap();
        assert_eq!(again.mailbox(S), store.mailbox(S));
        assert_eq!(again.retained, 1);
        assert!(again.together([1; 32], [2; 32]));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_store_knows_who_shares_what() {
        let mut store = Store::memory();
        store.insert(S, mailbox([1; 32], vec![[2; 32]], false));
        assert!(store.together([2; 32], [1; 32]));
        assert!(!store.together([2; 32], [3; 32]));
        assert!(store.opens([1; 32], KNOCK));
        // Only the owner's shares open to a knock on it.
        assert!(!store.opens([2; 32], KNOCK));
        assert_eq!(store.owned_by([1; 32]), 1);
        store.remove(S);
        assert!(!store.together([2; 32], [1; 32]));
        assert!(store.by_key.is_empty());
    }

    #[test]
    fn knocks_expire() {
        let mut reach = Reach::default();
        let limits = RelayLimits {
            grant_ttl: Duration::ZERO,
            ..RelayLimits::default()
        };
        reach.grant([1; 32], [2; 32], &limits);
        assert!(!reach.granted([2; 32], [1; 32]));
        reach.grant([1; 32], [2; 32], &RelayLimits::default());
        assert!(reach.granted([2; 32], [1; 32]));
    }

    #[test]
    fn tokens_depend_on_the_secret() {
        let key = ShareKey::generate();
        assert_eq!(KnockToken::share(&key, S), KnockToken::share(&key, S));
        assert_ne!(
            KnockToken::share(&key, S),
            KnockToken::share(&ShareKey::generate(), S)
        );
        let secret = PairingSecret::random();
        assert_ne!(
            KnockToken::pairing(&secret),
            KnockToken::pairing(&PairingSecret::random())
        );
    }
}
