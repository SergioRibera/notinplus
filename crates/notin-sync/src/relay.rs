//! The relay: a peer that is always on, so devices meet when they are
//! not on the same network, and members of a share meet even when they
//! are never online at the same time.
//!
//! Devices connected to the relay open end-to-end encrypted sessions
//! with each other through it, which work like any other channel: the
//! relay forwards bytes it cannot read and keeps nothing of them.
//!
//! For members never online at the same time it keeps a mailbox per
//! permanent share. Members publish sealed records and fetch what
//! arrived since the last sequence number they saw. The relay never
//! holds a share key, so it stores bytes it cannot read; the only
//! things in the clear are the share id, the document or record a
//! payload concerns, who sent it and when. Session shares never go
//! there, and a relay started without retention
//! ([`RelayLimits::retain`]) writes nothing down at all.
//!
//! Anyone may run a relay in public, so a device only reaches another
//! one through it when the target agreed: a device the target trusts,
//! a fellow member of one of its shares, or one that knocked with a
//! [`KnockToken`] derived from a secret the relay never sees. A refusal
//! looks the same as an absent target.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use blake2::{Blake2s256, Digest};

use crate::cipher::ShareKey;
use crate::identity::{DeviceId, DeviceKey, PairingSecret};
use crate::net::lan::LanNode;
use crate::net::{LanConfig, Lane, NetError, NetEvent, RelayRole};
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
    /// Whether `other` is this token, in constant time.
    #[must_use]
    pub fn matches(&self, other: Self) -> bool {
        subtle::ConstantTimeEq::ct_eq(self.0.as_slice(), other.0.as_slice()).into()
    }

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
    /// The sender walked away from the share.
    Left,
    /// The share's keys for the member with transport key `to` alone,
    /// after the owner changed them; replaces the earlier one for `to`.
    Keys { to: [u8; 32] },
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
    /// `to` expects or opens one of the shares it owns.
    Knock { to: [u8; 32], token: KnockToken },
    /// The knock on `to` is in place: a session to it may be opened
    /// now. Sent whether or not the token matched, so nobody learns
    /// whether `to` is here or what it expects.
    Knocked { to: [u8; 32] },
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

    /// Whether `public` may hand in a record of `kind`: any member, but
    /// only the owner speaks for the share as a whole or hands out keys.
    fn accepts(&self, public: [u8; 32], kind: RecordKind) -> bool {
        let owners_only = matches!(kind, RecordKind::Meta | RecordKind::Keys { .. });
        self.allows(public) && (!owners_only || self.owner == public)
    }

    /// Bytes of sealed records kept.
    fn bytes(&self) -> u64 {
        self.records.iter().map(|r| r.sealed.len() as u64).sum()
    }

    /// Append a record, dropping what it makes moot among the sender's
    /// own records, never anyone else's; kept only when `keep` (the
    /// mailbox retains and there is room).
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
                    r.from != from
                        || !matches!(
                            r.kind,
                            RecordKind::Update { doc: d } | RecordKind::Snapshot { doc: d } if d == doc
                        )
                }),
                RecordKind::Blob { key } => self.records.retain(|r| {
                    r.from != from || !matches!(r.kind, RecordKind::Blob { key: k } if k == key)
                }),
                RecordKind::Meta => self
                    .records
                    .retain(|r| r.from != from || !matches!(r.kind, RecordKind::Meta)),
                RecordKind::Keys { to } => self.records.retain(|r| {
                    r.from != from || !matches!(r.kind, RecordKind::Keys { to: t } if t == to)
                }),
                RecordKind::Left => self
                    .records
                    .retain(|r| r.from != from || !matches!(r.kind, RecordKind::Left)),
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
    /// Bytes of one sealed record.
    pub max_record: usize,
    /// Bytes kept for one share.
    pub max_mailbox_bytes: u64,
    /// Bytes kept for every share together.
    pub max_retained_bytes: u64,
    /// Knocks let through and not expired yet.
    pub max_grants: usize,
    /// How long a knock lets a device through.
    pub grant_ttl: Duration,
    /// Knocks let through toward one device at once.
    pub max_grants_per_target: usize,
    /// Devices connected at once; past it the one heard from least
    /// recently is dropped.
    pub max_devices: usize,
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
            max_grants_per_target: 64,
            max_devices: 4096,
        }
    }
}

/// Every mailbox, optionally mirrored to disk.
#[derive(Debug, Default)]
pub struct Store {
    dir: Option<PathBuf>,
    boxes: HashMap<ShareId, Mailbox>,
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
        self.retained += mailbox.bytes();
        self.boxes.insert(id, mailbox);
    }

    fn remove(&mut self, id: ShareId) -> Option<Mailbox> {
        let mailbox = self.boxes.remove(&id)?;
        self.retained = self.retained.saturating_sub(mailbox.bytes());
        Some(mailbox)
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

/// What the relay forwards between two devices, as the test hook in
/// [`RelayServer::start_observed`] sees it: sender key, target key and
/// the opaque inner bytes.
pub type RelayObserver = Arc<dyn Fn(&[u8; 32], &[u8; 32], &[u8]) + Send + Sync>;

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
        Self::start_inner(config, store, None)
    }

    /// [`Self::start`] that also hands every forwarded frame to
    /// `observer`. For tests that check what a relay gets to see.
    ///
    /// # Errors
    /// As [`Self::start`].
    #[doc(hidden)]
    pub fn start_observed(
        config: RelayConfig,
        store: Store,
        observer: RelayObserver,
    ) -> Result<Self, NetError> {
        let tap: zudp::RelayTap = Arc::new(move |from, to, inner| {
            observer(from, to, inner);
            vec![(*from, inner.clone())]
        });
        Self::start_inner(config, store, Some(tap))
    }

    fn start_inner(
        config: RelayConfig,
        store: Store,
        tap: Option<zudp::RelayTap>,
    ) -> Result<Self, NetError> {
        let gate = Arc::new(RwLock::new(Gate::default()));
        {
            let mut gate = write(&gate);
            for (id, mailbox) in &store.boxes {
                gate.index(*id, mailbox);
            }
        }
        let policy = Arc::clone(&gate);
        let role = RelayRole {
            // zudp's rate and session budgets already suit a public relay.
            limits: zudp::RelayLimits::default(),
            max_peers: config.limits.max_devices,
            policy: Arc::new(move |from: &[u8; 32], to: &[u8; 32]| {
                read(&policy).allows(*from, *to)
            }),
            tap,
        };
        let lan = LanConfig::new(config.key, config.name)
            .port(config.port)
            .advertise(false)
            .scan(false)
            .relay(role);
        let node = LanNode::start(lan)?;
        let events = node.events().clone();
        let sender = node.sender();
        let limits = config.limits;
        std::thread::Builder::new()
            .name("notin-relay".into())
            .spawn(move || serve(&events, sender, store, gate, limits))
            .map_err(NetError::Thread)?;
        Ok(Self { node })
    }

    #[must_use]
    pub const fn local_addr(&self) -> std::net::SocketAddr {
        self.node.local_addr()
    }
}

fn read(gate: &RwLock<Gate>) -> RwLockReadGuard<'_, Gate> {
    gate.read().unwrap_or_else(PoisonError::into_inner)
}

fn write(gate: &RwLock<Gate>) -> RwLockWriteGuard<'_, Gate> {
    gate.write().unwrap_or_else(PoisonError::into_inner)
}

/// Who agreed to be reached by whom, besides sharing a mailbox.
#[derive(Default)]
struct Reach {
    /// Keys each connected device trusts.
    trusts: HashMap<[u8; 32], HashSet<[u8; 32]>>,
    /// Open pairing offer of each connected device.
    expects: HashMap<[u8; 32], KnockToken>,
    /// Pairs let through by a knock.
    grants: HashMap<Pair, Grant>,
    /// Order of the grants, oldest first.
    next_grant: u64,
}

/// Two device keys, the smaller first.
type Pair = ([u8; 32], [u8; 32]);

/// A knock let a pair through: until when, toward whom, with what.
struct Grant {
    order: u64,
    until: Instant,
    to: [u8; 32],
    token: KnockToken,
}

impl Reach {
    /// A knock lets both ways through, so the pair is stored sorted.
    fn pair(a: [u8; 32], b: [u8; 32]) -> Pair {
        if a <= b { (a, b) } else { (b, a) }
    }

    fn granted(&self, a: [u8; 32], b: [u8; 32]) -> bool {
        self.grants
            .get(&Self::pair(a, b))
            .is_some_and(|g| g.until > Instant::now())
    }

    /// Let `from` and `to` reach each other for a while, `from` having
    /// knocked on `to` with `token`. Keys cost nothing, so one target
    /// holds only so many grants: the oldest goes first.
    fn grant(&mut self, from: [u8; 32], to: [u8; 32], token: KnockToken, limits: &RelayLimits) {
        let now = Instant::now();
        if self.grants.len() >= limits.max_grants {
            self.grants.retain(|_, g| g.until > now);
        }
        if self.grants.len() >= limits.max_grants {
            tracing::warn!(target: "notin-relay", "knock table full");
            return;
        }
        let toward: Vec<(Pair, u64)> = self
            .grants
            .iter()
            .filter(|(_, g)| g.to == to)
            .map(|(pair, g)| (*pair, g.order))
            .collect();
        if toward.len() >= limits.max_grants_per_target
            && let Some((oldest, _)) = toward.iter().min_by_key(|(_, order)| *order)
        {
            self.grants.remove(oldest);
        }
        self.next_grant += 1;
        self.grants.insert(
            Self::pair(from, to),
            Grant {
                order: self.next_grant,
                until: now + limits.grant_ttl,
                to,
                token,
            },
        );
    }

    /// `token` no longer opens `to`: whoever it let through is out.
    fn revoke_token(&mut self, to: [u8; 32], token: KnockToken) {
        self.grants.retain(|_, g| g.to != to || g.token != token);
    }

    fn revoke_pair(&mut self, a: [u8; 32], b: [u8; 32]) {
        self.grants.remove(&Self::pair(a, b));
    }

    /// `public` opened (or with `None` closed) its pairing offer; the
    /// knocks the previous one let through end with it.
    fn expect(&mut self, public: [u8; 32], token: Option<KnockToken>) {
        let previous = match token {
            Some(token) => self.expects.insert(public, token),
            None => self.expects.remove(&public),
        };
        if let Some(previous) = previous.filter(|p| Some(*p) != token) {
            self.revoke_token(public, previous);
        }
    }

    /// `public` trusts `keys` now; one it stopped trusting loses any
    /// grant toward it too.
    fn trust(&mut self, public: [u8; 32], keys: HashSet<[u8; 32]>) {
        let dropped: Vec<[u8; 32]> = self
            .trusts
            .get(&public)
            .map(|old| old.difference(&keys).copied().collect())
            .unwrap_or_default();
        for key in dropped {
            self.revoke_pair(public, key);
        }
        self.trusts.insert(public, keys);
    }

    /// `public` went away: what it declared goes with it.
    fn forget(&mut self, public: [u8; 32]) {
        self.trusts.remove(&public);
        if let Some(token) = self.expects.remove(&public) {
            self.revoke_token(public, token);
        }
    }
}

/// Who may use one share: its owner and members, and what a joiner
/// knocks with.
struct Roster {
    owner: [u8; 32],
    members: Vec<[u8; 32]>,
    knock: KnockToken,
}

impl Roster {
    fn allows(&self, public: [u8; 32]) -> bool {
        self.owner == public || self.members.contains(&public)
    }

    fn keys(&self) -> impl Iterator<Item = [u8; 32]> + '_ {
        std::iter::once(self.owner).chain(self.members.iter().copied())
    }
}

/// Who may reach whom here. The thread answering relay messages keeps
/// it current; zudp asks it from its own threads before forwarding a
/// session between two devices.
#[derive(Default)]
struct Gate {
    reach: Reach,
    rosters: HashMap<ShareId, Roster>,
    /// Shares each key owns or is a member of.
    by_key: HashMap<[u8; 32], HashSet<ShareId>>,
}

impl Gate {
    /// Whether `to` agreed to be reached by `from`: a knock let them
    /// through, `to` trusts `from`, or both take part in one share.
    fn allows(&self, from: [u8; 32], to: [u8; 32]) -> bool {
        self.reach.granted(from, to)
            || self
                .reach
                .trusts
                .get(&to)
                .is_some_and(|t| t.contains(&from))
            || self.together(from, to)
    }

    fn index(&mut self, id: ShareId, mailbox: &Mailbox) {
        self.unindex(id);
        let roster = Roster {
            owner: mailbox.owner,
            members: mailbox.members.clone(),
            knock: mailbox.knock,
        };
        for key in roster.keys() {
            self.by_key.entry(key).or_default().insert(id);
        }
        self.rosters.insert(id, roster);
    }

    fn unindex(&mut self, id: ShareId) {
        let Some(roster) = self.rosters.remove(&id) else {
            return;
        };
        for key in roster.keys() {
            if let Some(ids) = self.by_key.get_mut(&key) {
                ids.remove(&id);
                if ids.is_empty() {
                    self.by_key.remove(&key);
                }
            }
        }
    }

    /// Whether `a` and `b` take part in one share.
    fn together(&self, a: [u8; 32], b: [u8; 32]) -> bool {
        self.by_key.get(&a).is_some_and(|ids| {
            ids.iter()
                .any(|id| self.rosters.get(id).is_some_and(|r| r.allows(b)))
        })
    }

    /// Whether `token` opens one of the shares `owner` owns.
    fn opens(&self, owner: [u8; 32], token: KnockToken) -> bool {
        self.by_key.get(&owner).is_some_and(|ids| {
            ids.iter().any(|id| {
                self.rosters
                    .get(id)
                    .is_some_and(|r| r.owner == owner && r.knock.matches(token))
            })
        })
    }

    fn owned_by(&self, owner: [u8; 32]) -> usize {
        self.by_key.get(&owner).map_or(0, |ids| {
            ids.iter()
                .filter(|id| self.rosters.get(id).is_some_and(|r| r.owner == owner))
                .count()
        })
    }
}

struct Serving {
    sender: crate::net::lan::LanSender,
    store: Store,
    gate: Arc<RwLock<Gate>>,
    limits: RelayLimits,
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

    /// Keep a mailbox, and who may use it.
    fn insert(&mut self, id: ShareId, mailbox: Mailbox) {
        write(&self.gate).index(id, &mailbox);
        self.store.insert(id, mailbox);
    }

    fn remove(&mut self, id: ShareId) -> Option<Mailbox> {
        write(&self.gate).unindex(id);
        self.store.remove(id)
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
                write(&self.gate)
                    .reach
                    .trust(public, keys.into_iter().collect());
            }
            RelayMessage::Expect { token } => write(&self.gate).reach.expect(public, token),
            RelayMessage::Knock { to, token } => {
                {
                    let mut gate = write(&self.gate);
                    if gate
                        .reach
                        .expects
                        .get(&to)
                        .is_some_and(|e| e.matches(token))
                        || gate.opens(to, token)
                    {
                        gate.reach.grant(public, to, token, &self.limits);
                    }
                }
                self.reply(from, &RelayMessage::Knocked { to });
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
            RelayMessage::Registered { .. }
            | RelayMessage::Refused { .. }
            | RelayMessage::Full { .. }
            | RelayMessage::Records { .. }
            | RelayMessage::Gone { .. }
            | RelayMessage::Knocked { .. } => {}
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
                        && read(&self.gate).owned_by(public) < self.limits.max_shares_per_owner
                }
            };
        if !accepted {
            self.reply(from, &RelayMessage::Refused { share });
            return;
        }
        let mut mailbox = self.remove(share).unwrap_or(Mailbox {
            owner: public,
            members: Vec::new(),
            retain,
            knock,
            next_seq: 1,
            records: Vec::new(),
        });
        {
            // Members dropped, or a new invitation, take their grants along.
            let mut gate = write(&self.gate);
            for gone in mailbox.members.iter().filter(|m| !members.contains(m)) {
                gate.reach.revoke_pair(public, *gone);
            }
            if mailbox.knock != knock {
                gate.reach.revoke_token(public, mailbox.knock);
            }
        }
        mailbox.members = members;
        mailbox.retain = retain;
        mailbox.knock = knock;
        if !retain {
            mailbox.records.clear();
        }
        self.insert(share, mailbox);
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
        if let Some(mailbox) = self.remove(share) {
            write(&self.gate).reach.revoke_token(public, mailbox.knock);
        }
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
        if !mailbox.accepts(public, kind) {
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
        // Out and back in, so the bytes kept are counted again.
        let Some(mut mailbox) = self.store.remove(share) else {
            return;
        };
        let record = mailbox.push(from, kind, sealed, keep);
        let retains = mailbox.retain;
        self.store.insert(share, mailbox);
        if keep {
            self.store.flush(share);
        } else if retains {
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
    gate: Arc<RwLock<Gate>>,
    limits: RelayLimits,
) {
    let mut serving = Serving {
        sender,
        store,
        gate,
        limits,
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
                    write(&serving.gate).reach.forget(public);
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
            NetEvent::Discovered(_)
            | NetEvent::Renamed(_)
            | NetEvent::PathChanged { .. }
            | NetEvent::ConnectFailed { .. } => {}
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
    const K: BlobKey = BlobKey {
        kind: crate::replica::BlobKind::Item,
        id: [4; 16],
    };

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
    fn a_member_cannot_wipe_what_others_left() {
        const B: DeviceId = DeviceId([9; 32]);
        let mut mailbox = mailbox([0; 32], vec![[1; 32]], true);
        mailbox.push(A, RecordKind::Update { doc: D }, vec![1], true);
        mailbox.push(A, RecordKind::Blob { key: K }, vec![2], true);
        mailbox.push(A, RecordKind::Meta, vec![3], true);
        mailbox.push(B, RecordKind::Snapshot { doc: D }, vec![4], true);
        mailbox.push(B, RecordKind::Blob { key: K }, vec![5], true);
        mailbox.push(B, RecordKind::Meta, vec![6], true);
        let seqs: Vec<u64> = mailbox.records.iter().map(|r| r.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5, 6]);
        // And only the owner may hand in a view of the share.
        assert!(mailbox.accepts([0; 32], RecordKind::Meta));
        assert!(!mailbox.accepts([1; 32], RecordKind::Meta));
        assert!(!mailbox.accepts([1; 32], RecordKind::Keys { to: [2; 32] }));
        assert!(mailbox.accepts([0; 32], RecordKind::Keys { to: [2; 32] }));
        assert!(mailbox.accepts([1; 32], RecordKind::Update { doc: D }));
        assert!(!mailbox.accepts([7; 32], RecordKind::Update { doc: D }));
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
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_gate_knows_who_shares_what() {
        let mut gate = Gate::default();
        gate.index(S, &mailbox([1; 32], vec![[2; 32]], false));
        assert!(gate.together([2; 32], [1; 32]));
        assert!(gate.allows([2; 32], [1; 32]));
        assert!(!gate.together([2; 32], [3; 32]));
        assert!(gate.opens([1; 32], KNOCK));
        // Only the owner's shares open to a knock on it.
        assert!(!gate.opens([2; 32], KNOCK));
        assert_eq!(gate.owned_by([1; 32]), 1);
        gate.unindex(S);
        assert!(!gate.allows([2; 32], [1; 32]));
        assert!(gate.by_key.is_empty());
    }

    #[test]
    fn the_gate_lets_trusted_and_knocking_devices_through() {
        let mut gate = Gate::default();
        assert!(!gate.allows([1; 32], [2; 32]));
        gate.reach.trusts.insert([2; 32], HashSet::from([[1; 32]]));
        assert!(gate.allows([1; 32], [2; 32]));
        // Trust is one way: the trusted device has to trust back.
        assert!(!gate.allows([2; 32], [1; 32]));
        gate.reach
            .grant([3; 32], [2; 32], KNOCK, &RelayLimits::default());
        assert!(gate.allows([2; 32], [3; 32]));
        gate.reach.forget([2; 32]);
        assert!(!gate.allows([1; 32], [2; 32]));
    }

    #[test]
    fn knocks_expire() {
        let mut reach = Reach::default();
        let limits = RelayLimits {
            grant_ttl: Duration::ZERO,
            ..RelayLimits::default()
        };
        reach.grant([1; 32], [2; 32], KNOCK, &limits);
        assert!(!reach.granted([2; 32], [1; 32]));
        reach.grant([1; 32], [2; 32], KNOCK, &RelayLimits::default());
        assert!(reach.granted([2; 32], [1; 32]));
    }

    #[test]
    fn grants_end_with_what_let_them_through() {
        let limits = RelayLimits::default();
        let offer = KnockToken([4; 32]);
        let mut reach = Reach::default();
        // A closed or replaced pairing offer lets nobody through any more.
        reach.expect([2; 32], Some(offer));
        reach.grant([1; 32], [2; 32], offer, &limits);
        reach.grant([5; 32], [2; 32], KNOCK, &limits);
        reach.expect([2; 32], None);
        assert!(!reach.granted([1; 32], [2; 32]));
        assert!(reach.granted([5; 32], [2; 32]), "another token stays");
        // Trust taken back takes the pair's grant with it.
        reach.trust([2; 32], HashSet::from([[5; 32]]));
        reach.trust([2; 32], HashSet::new());
        assert!(!reach.granted([5; 32], [2; 32]));
        // A share's token going away closes what it opened.
        reach.grant([6; 32], [2; 32], KNOCK, &limits);
        reach.revoke_token([2; 32], KNOCK);
        assert!(!reach.granted([6; 32], [2; 32]));
    }

    #[test]
    fn one_device_holds_only_so_many_knocks() {
        let limits = RelayLimits {
            max_grants_per_target: 3,
            ..RelayLimits::default()
        };
        let mut reach = Reach::default();
        for i in 0..10u8 {
            reach.grant([i + 10; 32], [2; 32], KNOCK, &limits);
        }
        assert_eq!(reach.grants.len(), 3);
        assert!(reach.granted([19; 32], [2; 32]), "the newest knock stays");
        assert!(!reach.granted([10; 32], [2; 32]));
    }

    #[test]
    fn tokens_depend_on_the_secret() {
        assert!(KNOCK.matches(KNOCK));
        assert!(!KNOCK.matches(KnockToken([4; 32])));
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
