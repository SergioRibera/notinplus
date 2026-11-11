//! Policy on top of the transport: who to connect to, who to trust,
//! how to pair, which shares to serve, and when to run an exchange
//! round.
//!
//! The driver is a state machine. The app feeds it [`NetEvent`]s and
//! clock ticks and carries out the [`Action`]s it returns, so it can
//! be tested without a network and lives on whatever thread the app
//! prefers.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::cipher::ShareKey;
use crate::engine::{Message, SyncEngine};
use crate::error::SyncError;
use crate::identity::{DeviceId, DeviceProfile, PairingCode, PairingSecret, TrustedDevice};
use crate::net::{Lane, NetEvent, PeerInfo};
use crate::relay::{KnockToken, RecordKind, Registration, RelayAddr, RelayMessage};
use crate::replica::{BlobKey, BlobKind, DocId, Replica, Scope, Version};
use crate::share::{
    EndedShare, Invite, LeftShare, Member, Role, Share, ShareBook, ShareEdit, ShareId,
    ShareMessage, ShareMode, SignedLease, Unopened, open_keys, proof_key, seal_keys,
};

/// How long a pairing offer stays valid.
pub const OFFER_TTL: Duration = Duration::from_secs(5 * 60);
/// How often [`SyncDriver::tick`] should be called.
pub const TICK: Duration = Duration::from_secs(15);
/// How often [`SyncDriver::supervise`] should be called.
pub const SUPERVISE: Duration = Duration::from_secs(1);
/// First wait before dialing an unreachable device again; it doubles
/// up to [`REDIAL_MAX`].
const REDIAL_BASE: Duration = Duration::from_secs(1);
const REDIAL_MAX: Duration = Duration::from_secs(30);
/// Rounds the larger id leaves to the smaller one before it dials too.
const LARGER_WAITS: u32 = 2;
/// Payloads held for one device until a channel to it opens.
const OUTBOX_MAX: usize = 64;
/// How long a mailbox record that may still open waits: for a view
/// listing its author, or a key the owner has yet to hand over.
const PARK_FOR_MS: u64 = 10 * 60 * 1000;
/// Such records kept per share.
const PARKED_MAX: usize = 256;
/// Bytes of such records kept per share.
const PARKED_BYTES: usize = 8 << 20;
/// How long a held message is worth delivering.
const OUTBOX_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// When to dial a related device that is known but not connected.
#[derive(Debug)]
struct Redial {
    rounds: u32,
    next: Instant,
}

impl Redial {
    const fn due(now: Instant) -> Self {
        Self {
            rounds: 0,
            next: now,
        }
    }

    /// Back off after one more round. Up to a quarter more at random,
    /// so devices that lost the network together do not dial in step.
    fn schedule(&mut self, now: Instant) {
        self.rounds = self.rounds.saturating_add(1);
        let base = REDIAL_BASE
            .saturating_mul(1 << (self.rounds - 1).min(5))
            .min(REDIAL_MAX);
        self.next = now + base + base.mul_f64(rand::random::<f64>() / 4.0);
    }
}

const LINK_SCHEME: &str = "notein://pair?";

/// What travels inside a transport payload.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub enum Payload {
    /// Exchange between trusted devices of one user.
    Sync(Vec<u8>),
    Pair(PairMessage),
    /// Sealed [`ShareMessage`] for one share.
    Share {
        share: ShareId,
        sealed: Vec<u8>,
    },
    /// Ask the owner to let us into a share.
    Join {
        share: ShareId,
        ask: JoinAsk,
    },
    /// The owner's answer, sealed with the share key.
    JoinReply {
        share: ShareId,
        sealed: Vec<u8>,
    },
    /// The owner's newest lease on a share, from the owner or passed on
    /// by a member. Signed, not sealed: one that missed a key change must
    /// still be able to read that it did.
    Lease {
        share: ShareId,
        lease: SignedLease,
    },
    /// The share's new keys from its owner, sealed for the receiver
    /// alone (see [`seal_keys`]).
    Keys {
        share: ShareId,
        sealed: Vec<u8>,
    },
    /// A [`RelayMessage`] between a device and a relay.
    Relay(Vec<u8>),
    /// Ephemeral state of one document between trusted devices: ink
    /// still being drawn, a pointer, a caret. Never stored, never
    /// retried; the next frame supersedes it.
    Live {
        doc: DocId,
        bytes: Vec<u8>,
    },
}

impl Payload {
    #[cfg_attr(feature = "sismo", sismo::function("sync::payload::encode"))]
    fn to_bytes(&self) -> Vec<u8> {
        bincode::encode_to_vec(self, bincode::config::standard()).unwrap_or_default()
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::payload::decode"))]
    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        crate::decode(bytes, crate::cap::PAYLOAD).ok()
    }
}

/// Who asks into a share, and the proof it holds the invitation.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub struct JoinAsk {
    pub name: String,
    pub device_name: String,
    pub proof: Vec<u8>,
    /// What the joiner signs its mailbox records with.
    pub sign: [u8; 32],
    /// Back after leaving: when the membership it left began. The owner
    /// ends that one now if the departure has not reached it.
    pub ended: Option<u64>,
}

/// What the owner says to a joiner.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
enum JoinAnswer {
    Welcome(Box<Share>),
    /// An organizer has to let the joiner in first.
    Pending,
    Refused,
}

/// The pairing handshake, carried over an already authenticated
/// channel so both sides know exactly which key they are trusting.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub enum PairMessage {
    /// `proof` is the link's secret bound to the requester's key
    /// ([`PairingSecret::proof`]); without it only a device on the same
    /// network pairs, with the code alone.
    Request {
        code: u32,
        name: String,
        proof: Option<[u8; 32]>,
    },
    Accept {
        name: String,
    },
    Reject,
}

/// What a device shows (as QR, link or digits) so another one can
/// pair with it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PairingOffer {
    pub device: DeviceId,
    pub public: [u8; 32],
    pub name: String,
    pub code: PairingCode,
    /// Only in a QR or link, never typed: what lets the device reach the
    /// offering one through a relay.
    pub secret: Option<PairingSecret>,
    /// The relay the offering device waits at.
    pub relay: Option<RelayAddr>,
}

impl PairingOffer {
    /// `notein://pair?pk=<hex>&name=<hex>&code=<6 digits>[&s=<hex>][&relay=<hex>]`,
    /// the relay as the hex of its `host:port/<key>` text.
    #[must_use]
    pub fn to_link(&self) -> String {
        let mut link = format!(
            "{LINK_SCHEME}pk={}&name={}&code={}",
            hex::encode(self.public),
            hex::encode(self.name.as_bytes()),
            self.code
        );
        if let Some(secret) = self.secret {
            link.push_str("&s=");
            link.push_str(&hex::encode(secret.as_bytes()));
        }
        if let Some(relay) = &self.relay {
            link.push_str("&relay=");
            link.push_str(&hex::encode(relay.to_string()));
        }
        link
    }

    /// Reverse of [`Self::to_link`].
    #[must_use]
    pub fn parse_link(link: &str) -> Option<Self> {
        let query = link.trim().strip_prefix(LINK_SCHEME)?;
        let mut public = None;
        let mut name = None;
        let mut code = None;
        let mut secret = None;
        let mut relay = None;
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=')?;
            match key {
                "pk" => {
                    let bytes = hex::decode(value).ok()?;
                    public = Some(<[u8; 32]>::try_from(bytes).ok()?);
                }
                "name" => name = Some(String::from_utf8(hex::decode(value).ok()?).ok()?),
                "code" => code = PairingCode::parse(value),
                "s" => {
                    let bytes = hex::decode(value).ok()?;
                    secret = Some(PairingSecret::from(<[u8; 16]>::try_from(bytes).ok()?));
                }
                "relay" => {
                    let text = String::from_utf8(hex::decode(value).ok()?).ok()?;
                    relay = Some(text.parse().ok()?);
                }
                _ => {}
            }
        }
        let public = public?;
        Some(Self {
            device: DeviceId::of_public_key(&public),
            public,
            name: name?,
            code: code?,
            secret,
            relay,
        })
    }
}

/// Why a pairing did not go through.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PairFailure {
    /// The other side had no offer open, or the code was wrong.
    Rejected,
    /// The other side never came online.
    Unreachable,
}

/// Why a share could not be joined.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JoinFailure {
    /// The owner does not know the share any more, or the key is wrong.
    Refused,
    /// The owner never came online.
    Unreachable,
}

/// Why a share is no longer held here.
#[derive(bincode::Encode, bincode::Decode, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Removal {
    /// The owner closed it for everyone.
    Ended,
    /// The owner threw this device out.
    Expelled,
    /// This device walked away.
    Left,
}

/// Something the app should react to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DriverEvent {
    /// A device joined the trusted set; the profile changed.
    Paired(TrustedDevice),
    PairFailed {
        device: DeviceId,
        reason: PairFailure,
    },
    /// A trusted device took a new name; the profile changed.
    Renamed {
        device: DeviceId,
        name: String,
    },
    /// A trusted device or share member came online or went away.
    PeerOnline {
        device: DeviceId,
        online: bool,
    },
    /// Documents that received remote content.
    Imported(Vec<DocId>),
    /// Blobs replaced by a newer remote copy.
    Merged(Vec<BlobKey>),
    /// A share was created, joined, updated or lost; the book changed.
    ShareChanged(ShareId),
    /// The share is gone here, and why.
    ShareRemoved {
        share: ShareId,
        reason: Removal,
    },
    JoinFailed {
        share: ShareId,
        reason: JoinFailure,
    },
    /// The owner heard us; an organizer has to let us in.
    JoinPending(ShareId),
    /// Someone asked into a share we organize and waits for an answer.
    EntryRequested {
        share: ShareId,
        member: Member,
    },
    /// The relay came within reach or went away.
    RelayOnline(bool),
    /// This device took the relay of the device it pairs with as its
    /// own, having none; the profile should keep it.
    RelayAdopted(RelayAddr),
    /// Ephemeral state of a document from another device, see
    /// [`SyncDriver::live`]. Opaque to the driver.
    Live {
        from: DeviceId,
        name: String,
        doc: DocId,
        bytes: Vec<u8>,
    },
    /// The owner's lease on the share ran out or names a newer key or
    /// other members: this device shares with nobody but the owner until
    /// it hears from it. The app says it waits for the owner.
    LeaseExpired(ShareId),
    /// The lease holds again; sharing with the other members resumes.
    LeaseRenewed(ShareId),
    /// The share changed its key: earlier invitations no longer let
    /// anyone in, so the one shown should be made again with
    /// [`SyncDriver::invite_for`].
    InviteRenewed(ShareId),
    /// The channel to a device moved onto a direct path or back to the
    /// relay; see [`SyncDriver::path_to`].
    PathChanged {
        device: DeviceId,
        path: Path,
    },
}

/// What the app must carry out.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Action {
    Send {
        device: DeviceId,
        lane: Lane,
        bytes: Vec<u8>,
    },
    Connect {
        addr: SocketAddr,
        public: [u8; 32],
    },
    /// Dial the relay at a `host:port` that still needs resolving.
    ConnectHost {
        host: String,
        public: [u8; 32],
    },
    /// Open a channel to `public` through the connected relay `relay`.
    ConnectVia {
        relay: DeviceId,
        public: [u8; 32],
    },
    /// Let the channel to `device` through a relay go direct.
    Punch {
        device: DeviceId,
    },
    /// Close the channel to `device`.
    Disconnect {
        device: DeviceId,
    },
    /// Put together (or stop) payloads `device` sends in parts; see
    /// [`crate::LanNode::allow_bulk`].
    Bulk {
        device: DeviceId,
        allowed: bool,
    },
    Event(DriverEvent),
}

#[derive(Clone, Copy, Debug)]
struct Offer {
    code: PairingCode,
    secret: PairingSecret,
    since: Instant,
}

impl Offer {
    fn expired(&self) -> bool {
        self.since.elapsed() > OFFER_TTL
    }
}

#[derive(Clone, Debug)]
struct PairIntent {
    device: DeviceId,
    public: [u8; 32],
    code: PairingCode,
    secret: Option<PairingSecret>,
    relay: Option<RelayAddr>,
    name: String,
    since: Instant,
}

#[derive(Clone, Debug)]
struct JoinIntent {
    invite: Invite,
    since: Instant,
    /// The owner heard us and an organizer has to say yes.
    awaiting_approval: bool,
    /// Back after leaving: when the membership it left began.
    ended: Option<u64>,
}

/// An invitation accepted and not answered yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingJoin<'a> {
    pub invite: &'a Invite,
    /// Whether the owner was reached and someone has to let us in,
    /// rather than the owner being nowhere yet.
    pub awaiting_approval: bool,
}

/// How a device is reached right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Path {
    /// A channel straight to the device: on the same network, or one
    /// through a relay that was punched through.
    Direct,
    /// Through the relay: a channel it carries, or its mailbox.
    Relay,
}

/// What was last handed to the relay for one share, so only new
/// content travels.
#[derive(Default, Debug)]
struct Published {
    docs: HashMap<DocId, Version>,
    blobs: HashMap<BlobKey, u64>,
    meta_at: u64,
}

impl Published {
    /// Records for the documents in `docs` that changed since the last
    /// publish: updates when the relay can build on what it has, else a
    /// snapshot.
    #[cfg_attr(feature = "sismo", sismo::function("sync::relay::doc_records"))]
    fn doc_records<R: Replica>(
        &mut self,
        replica: &R,
        docs: &[DocId],
    ) -> Result<Vec<(RecordKind, Message)>, SyncError<R::Error>> {
        let mut records = Vec::new();
        for &doc in docs {
            let Some(version) = replica.doc_version(doc) else {
                continue;
            };
            let updates = match self.docs.get(&doc) {
                Some(known) if *known == version => continue,
                Some(known) => replica.doc_updates_since(doc, known).ok(),
                None => None,
            };
            match updates {
                Some(bytes) if bytes.is_empty() => {}
                Some(bytes) => records.push((
                    RecordKind::Update { doc },
                    Message::DocUpdates { doc, bytes },
                )),
                None => {
                    let bytes = replica.doc_snapshot(doc).map_err(SyncError::Replica)?;
                    records.push((
                        RecordKind::Snapshot { doc },
                        Message::DocSnapshot { doc, bytes },
                    ));
                }
            }
            self.docs.insert(doc, version);
        }
        Ok(records)
    }

    /// Records for the blobs in `share`'s scope that `me` may publish and
    /// that changed since the last publish.
    #[cfg_attr(feature = "sismo", sismo::function("sync::relay::blob_records"))]
    fn blob_records<R: Replica>(
        &mut self,
        replica: &R,
        share: &Share,
        me: DeviceId,
    ) -> Vec<(RecordKind, Message)> {
        let scope = share.scope();
        let mut records = Vec::new();
        for head in replica.blob_heads() {
            let key = head.key();
            if !scope.covers_blob(key) {
                continue;
            }
            let allowed = match key.kind {
                BlobKind::Item | BlobKind::Snapshot | BlobKind::Asset | BlobKind::Attachment => {
                    share.can_edit(me)
                }
                BlobKind::Folder => share.folder.is_some() && share.can_edit(me),
                BlobKind::Link => share.can_link(me),
                // Personal records: never part of a share.
                BlobKind::Tag
                | BlobKind::Workspace
                | BlobKind::ItemTags
                | BlobKind::FolderTags
                | BlobKind::Pins => false,
            };
            if !allowed
                || self
                    .blobs
                    .get(&key)
                    .is_some_and(|&at| at >= head.updated_at)
            {
                continue;
            }
            let blobs = replica.blobs(&[key]);
            if blobs.is_empty() {
                continue;
            }
            records.push((RecordKind::Blob { key }, Message::Blobs(blobs)));
            self.blobs.insert(key, head.updated_at);
        }
        records
    }
}

/// The one-shot messages held for a device that is away.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Held {
    /// This device walked away from the share.
    Left,
    /// The share ended for the device, or it was thrown out.
    Removal,
    /// A change asked of the owner, as encoded: the same change asked
    /// again waits once.
    Edit(Vec<u8>),
}

/// A message held until a channel to its device opens.
#[derive(Debug)]
struct Waiting {
    share: ShareId,
    kind: Held,
    payload: Payload,
    since: Instant,
}

/// A relay this device meets others at.
#[derive(Debug)]
struct RelayLink {
    addr: RelayAddr,
    device: DeviceId,
    connected: bool,
    published: HashMap<ShareId, Published>,
    /// Key epoch the relay has each share at, learned from its answer to
    /// our subscription: until it has answered, nothing is published.
    epochs: HashMap<ShareId, u32>,
}

/// A share this device takes part in, with its own exchange.
struct ShareLink<R> {
    share: Share,
    engine: SyncEngine<Arc<Mutex<R>>>,
}

/// Owns the profile, the exchanges and the view of the network.
pub struct SyncDriver<R> {
    profile: DeviceProfile,
    replica: Arc<Mutex<R>>,
    engine: SyncEngine<Arc<Mutex<R>>>,
    shares: HashMap<ShareId, ShareLink<R>>,
    discovered: HashMap<DeviceId, PeerInfo>,
    connected: HashMap<DeviceId, PeerInfo>,
    offer: Option<Offer>,
    intent: Option<PairIntent>,
    joins: HashMap<ShareId, JoinIntent>,
    /// Every relay in use: the profile's own, and the ones shares,
    /// invitations and pairing links point at.
    relays: HashMap<DeviceId, RelayLink>,
    /// The profile's own relay, among [`Self::relays`].
    own_relay: Option<DeviceId>,
    /// Relays no longer in use: one that shows up again is let go, never
    /// taken for an ordinary device.
    retired: HashSet<DeviceId>,
    /// Public keys of every device heard of, so a channel can be opened
    /// through a relay when it is not on the network.
    known_keys: HashMap<DeviceId, [u8; 32]>,
    /// Whether the lease on each share held when last looked at, to tell
    /// the app when that changes.
    leases: HashMap<ShareId, bool>,
    /// The lease each share holds and since when on this device's
    /// monotonic clock.
    lease_seen: HashMap<ShareId, (u64, Instant)>,
    /// Whether each connected device's payloads in parts are taken, as
    /// last told to the transport.
    bulk: HashMap<DeviceId, bool>,
    /// Mailbox records by someone a share does not list yet, waiting
    /// for a newer view of it, with how often they were tried.
    parked: HashMap<ShareId, Parked>,
    /// What waits for a channel to a device that is away.
    outbox: HashMap<DeviceId, Vec<Waiting>>,
    redials: HashMap<DeviceId, Redial>,
    /// Shares this device owned and closed.
    ended: HashMap<ShareId, EndedShare>,
    /// Shares this device walked away from.
    left: HashMap<ShareId, LeftShare>,
}

impl<R> std::fmt::Debug for SyncDriver<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncDriver")
            .field("device", &self.profile.id())
            .field("shares", &self.shares.len())
            .field("connected", &self.connected.len())
            .finish_non_exhaustive()
    }
}

impl<R: Replica> SyncDriver<R> {
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::new"))]
    pub fn new(profile: DeviceProfile, replica: R, book: ShareBook) -> Self {
        let replica = Arc::new(Mutex::new(replica));
        let engine = SyncEngine::new(Arc::clone(&replica), Scope::All);
        let shares = book
            .shares
            .into_iter()
            .map(|share| {
                let engine = SyncEngine::new(Arc::clone(&replica), share.scope());
                (share.id, ShareLink { share, engine })
            })
            .collect();
        Self {
            profile,
            replica,
            engine,
            shares,
            discovered: HashMap::new(),
            connected: HashMap::new(),
            offer: None,
            intent: None,
            joins: book
                .joins
                .into_iter()
                .map(|invite| {
                    let intent = JoinIntent {
                        invite,
                        since: Instant::now(),
                        awaiting_approval: false,
                        ended: None,
                    };
                    (intent.invite.share, intent)
                })
                .collect(),
            relays: HashMap::new(),
            own_relay: None,
            retired: HashSet::new(),
            known_keys: HashMap::new(),
            outbox: HashMap::new(),
            parked: HashMap::new(),
            leases: HashMap::new(),
            lease_seen: HashMap::new(),
            bulk: HashMap::new(),
            redials: HashMap::new(),
            ended: book.ended.into_iter().map(|e| (e.id, e)).collect(),
            left: book.left.into_iter().map(|l| (l.invite.share, l)).collect(),
        }
    }

    /// What to persist about shares, for [`Self::new`] after a restart.
    #[must_use]
    pub fn book(&self) -> ShareBook {
        ShareBook {
            shares: self.shares().cloned().collect(),
            ended: self.ended.values().cloned().collect(),
            left: self.left.values().cloned().collect(),
            joins: self.joins.values().map(|j| j.invite.clone()).collect(),
        }
    }

    #[must_use]
    pub const fn profile(&self) -> &DeviceProfile {
        &self.profile
    }

    pub const fn profile_mut(&mut self) -> &mut DeviceProfile {
        &mut self.profile
    }

    /// The exchange between this user's own devices.
    #[must_use]
    pub const fn engine(&self) -> &SyncEngine<Arc<Mutex<R>>> {
        &self.engine
    }

    /// Run `f` against the library replica.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::with_replica"))]
    pub fn with_replica<T>(&self, f: impl FnOnce(&mut R) -> T) -> T {
        let mut guard = self
            .replica
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut guard)
    }

    /// Devices seen on the network, trusted or not.
    pub fn discovered(&self) -> impl Iterator<Item = &PeerInfo> {
        self.discovered.values()
    }

    /// Trusted devices with an open channel.
    pub fn online(&self) -> impl Iterator<Item = &PeerInfo> {
        self.connected
            .values()
            .filter(|p| self.profile.is_trusted(p.device))
    }

    /// Every device with an open channel, trusted or share member.
    pub fn connected(&self) -> impl Iterator<Item = &PeerInfo> {
        self.connected.values()
    }

    /// Every share this device takes part in.
    pub fn shares(&self) -> impl Iterator<Item = &Share> {
        self.shares.values().map(|link| &link.share)
    }

    #[must_use]
    pub fn share(&self, id: ShareId) -> Option<&Share> {
        self.shares.get(&id).map(|link| &link.share)
    }

    /// Where `doc` stands inside `share`.
    #[must_use]
    pub fn share_status(&self, share: ShareId, doc: DocId) -> Option<crate::engine::DocStatus> {
        self.shares.get(&share).map(|link| link.engine.status(doc))
    }

    /// Whether a file another device announced has yet to arrive, here
    /// or in any share.
    #[must_use]
    pub fn files_pending(&self) -> bool {
        self.engine.files_pending() || self.shares.values().any(|l| l.engine.files_pending())
    }

    /// This device as a share member.
    #[must_use]
    pub fn me(&self) -> Member {
        Member {
            device: self.profile.id(),
            public: *self.profile.key().public(),
            sign: self.profile.key().sign_public(),
            name: self.profile.display_name().to_owned(),
            device_name: self.profile.name().to_owned(),
            role: Role::Owner,
            joined_at: now_ms(),
        }
    }

    /// How `device` can be reached right now, if at all.
    #[must_use]
    pub fn path_to(&self, device: DeviceId) -> Option<Path> {
        let is_relay = self.relays.contains_key(&device);
        if !is_relay && let Some(peer) = self.connected.get(&device) {
            Some(if peer.relayed {
                Path::Relay
            } else {
                Path::Direct
            })
        } else if self.known_keys.contains_key(&device) && self.relay_for(device).is_some() {
            Some(Path::Relay)
        } else {
            None
        }
    }

    /// Whether `device` takes part in any share with us.
    fn shares_with(&self, device: DeviceId) -> Vec<ShareId> {
        self.shares
            .values()
            .filter(|link| link.share.is_member(device))
            .map(|link| link.share.id)
            .collect()
    }

    // ---------- Pairing -----------------------------------------------

    /// Start accepting one pairing: the returned offer is what to show,
    /// the actions tell the relays to let its holder through.
    pub fn begin_pairing(&mut self) -> (PairingOffer, Vec<Action>) {
        let offer = Offer {
            code: PairingCode::random(),
            secret: PairingSecret::random(),
            since: Instant::now(),
        };
        self.offer = Some(offer);
        let shown = PairingOffer {
            device: self.profile.id(),
            public: *self.profile.key().public(),
            name: self.profile.name().to_owned(),
            code: offer.code,
            secret: Some(offer.secret),
            relay: self.relay().cloned(),
        };
        (shown, self.relay_consent())
    }

    /// Close the offer and drop the pairing under way; the actions tell
    /// the relay the link no longer lets anyone through.
    pub fn cancel_pairing(&mut self) -> Vec<Action> {
        self.offer = None;
        self.intent = None;
        self.relay_consent()
    }

    /// Stop trusting `device`: the relay stops letting it through and
    /// nothing held for it goes out any more. `None` when it was not
    /// trusted.
    pub fn revoke(&mut self, device: DeviceId) -> Option<Vec<Action>> {
        if !self.profile.revoke(device) {
            return None;
        }
        self.outbox.remove(&device);
        Some(self.relay_consent())
    }

    /// Pair with the device described by `offer` (scanned, pasted or
    /// picked from the discovered list with its code typed in).
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::pair_with"))]
    pub fn pair_with(&mut self, offer: &PairingOffer) -> Vec<Action> {
        self.intent = Some(PairIntent {
            device: offer.device,
            public: offer.public,
            code: offer.code,
            secret: offer.secret,
            relay: offer.relay.clone(),
            name: offer.name.clone(),
            since: Instant::now(),
        });
        if self.connected.contains_key(&offer.device) {
            return vec![self.pair_request(offer.device)];
        }
        self.known_keys.insert(offer.device, offer.public);
        let mut out = match &offer.relay {
            // Without a relay of our own, the paired device's serves us both.
            Some(relay) if self.own_relay.is_none() => {
                let mut out = self.set_relay(Some(relay.clone()));
                out.push(Action::Event(DriverEvent::RelayAdopted(relay.clone())));
                out
            }
            _ => self.sync_relays(),
        };
        if let Some(peer) = self.discovered.get(&offer.device) {
            out.push(Action::Connect {
                addr: peer.addr,
                public: peer.public,
            });
        } else {
            // Through a relay the device only takes our channel once we knocked.
            out.extend(self.pair_knock());
        }
        out
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::pair_request"))]
    fn pair_request(&self, device: DeviceId) -> Action {
        let code = self.intent.as_ref().map_or(0, |i| i.code.value());
        let proof = self
            .intent
            .as_ref()
            .and_then(|i| i.secret)
            .map(|secret| secret.proof(self.profile.key().public()));
        send(
            device,
            &Payload::Pair(PairMessage::Request {
                code,
                name: self.profile.name().to_owned(),
                proof,
            }),
        )
    }

    /// Name shown to the device we are pairing with, if any.
    #[must_use]
    pub fn pairing_target(&self) -> Option<&str> {
        self.intent.as_ref().map(|i| i.name.as_str())
    }

    /// The offer currently open, if [`Self::begin_pairing`] ran and
    /// it has not expired or been consumed.
    #[must_use]
    pub fn current_offer(&self) -> Option<PairingOffer> {
        let offer = self.offer.filter(|o| !o.expired())?;
        Some(PairingOffer {
            device: self.profile.id(),
            public: *self.profile.key().public(),
            name: self.profile.name().to_owned(),
            code: offer.code,
            secret: Some(offer.secret),
            relay: self.relay().cloned(),
        })
    }

    // ---------- Shares ------------------------------------------------

    /// Start owning a share. `share.owner` must be this device.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::create_share"))]
    pub fn create_share(&mut self, mut share: Share) -> Vec<Action> {
        let id = share.id;
        if share.server.is_none() {
            share.server = self.relay().map(ToString::to_string);
        }
        share.issue_lease(self.profile.key(), now_ms());
        let engine = SyncEngine::new(Arc::clone(&self.replica), share.scope());
        self.shares.insert(id, ShareLink { share, engine });
        let mut out = self.relay_register(id);
        out.push(Action::Event(DriverEvent::ShareChanged(id)));
        out
    }

    /// Owner-side change of scope, policy or title; members learn it
    /// on their next message.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::update_share"))]
    pub fn update_share(&mut self, id: ShareId, change: impl FnOnce(&mut Share)) -> Vec<Action> {
        let Some(link) = self.shares.get_mut(&id) else {
            return Vec::new();
        };
        change(&mut link.share);
        link.share.touch(now_ms());
        link.engine.set_scope(link.share.scope());
        let mut out = self.broadcast_meta(id);
        out.extend(self.relay_register(id));
        out.push(Action::Event(DriverEvent::ShareChanged(id)));
        out
    }

    /// Owner throws a member out.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::expel"))]
    pub fn expel(&mut self, id: ShareId, device: DeviceId) -> Vec<Action> {
        let Some(link) = self.shares.get_mut(&id) else {
            return Vec::new();
        };
        let inside = link.share.is_member(device);
        if !link.share.is_owner(self.profile.id()) || !link.share.expel(device) {
            return Vec::new();
        }
        link.share.touch(now_ms());
        if !inside {
            // Gone already, with the key changed then: only its way back
            // closes.
            return vec![Action::Event(DriverEvent::ShareChanged(id))];
        }
        link.engine.forget_peer(device);
        let payload = Payload::Share {
            share: id,
            sealed: ShareMessage::Expelled.seal(&link.share.key),
        };
        self.forget_held(id, Some(device));
        let mut out: Vec<Action> = self
            .hold(device, id, Held::Removal, payload)
            .into_iter()
            .collect();
        out.extend(self.rekey(id));
        out
    }

    /// Owner side, once someone is out: a new key for everyone still in,
    /// so nothing sealed from now on opens with the one the departed
    /// device keeps. Each member gets it over its open channel and, for
    /// when it is away, in the mailbox sealed for it alone. The relay
    /// learns the new knock token, so the old invitation stops working.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::rekey"))]
    fn rekey(&mut self, id: ShareId) -> Vec<Action> {
        let me = self.profile.id();
        let Some(link) = self.shares.get_mut(&id) else {
            return Vec::new();
        };
        if !link.share.is_owner(me) {
            return Vec::new();
        }
        link.share.rotate(ShareKey::generate());
        link.share.touch(now_ms());
        let share = link.share.clone();
        let keys: Vec<ShareKey> = std::iter::once(share.key.clone())
            .chain(share.previous.iter().cloned())
            .collect();
        let relay = self.share_relay(id);
        let mut out = Vec::new();
        for member in &share.members {
            let Some(sealed) =
                seal_keys(self.profile.key(), &member.public, id, share.epoch, &keys)
            else {
                continue;
            };
            if self.connected.contains_key(&member.device) {
                out.push(send(
                    member.device,
                    &Payload::Keys {
                        share: id,
                        sealed: sealed.clone(),
                    },
                ));
            }
            out.extend(relay.and_then(|relay| {
                self.relay_send(
                    relay,
                    &RelayMessage::Publish {
                        share: id,
                        kind: RecordKind::Keys { to: member.public },
                        sealed,
                        epoch: share.epoch,
                    },
                )
            }));
        }
        out.extend(self.relay_register(id));
        out.extend(self.broadcast_meta(id));
        // Whoever is out no longer sends big payloads here either.
        out.extend(self.bulk_changes());
        out.extend(self.spread_lease(id));
        out.push(Action::Event(DriverEvent::InviteRenewed(id)));
        out.push(Action::Event(DriverEvent::ShareChanged(id)));
        out
    }

    /// Member side: keys the owner of `id` handed over. Returns whether
    /// they were new.
    fn take_keys(&mut self, id: ShareId, from: DeviceId, sealed: &[u8]) -> bool {
        let Some(link) = self.shares.get_mut(&id) else {
            return false;
        };
        if !link.share.is_owner(from) {
            return false;
        }
        let owner = link.share.owner.public;
        open_keys(self.profile.key(), &owner, id, sealed)
            .is_some_and(|(epoch, keys)| link.share.adopt_keys(epoch, keys))
    }

    /// Owner stops letting a device that walked away back in with its
    /// old invitation.
    pub fn forget_departed(&mut self, id: ShareId, device: DeviceId) -> Vec<Action> {
        let me = self.profile.id();
        let Some(link) = self.shares.get_mut(&id).filter(|l| l.share.is_owner(me)) else {
            return Vec::new();
        };
        if !link.share.forget_departed(device) {
            return Vec::new();
        }
        vec![Action::Event(DriverEvent::ShareChanged(id))]
    }

    /// Owner closes the share for everyone.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::end_share"))]
    pub fn end_share(&mut self, id: ShareId) -> Vec<Action> {
        let relay = self.share_relay(id);
        let Some(link) = self.shares.remove(&id) else {
            return Vec::new();
        };
        self.forget_held(id, None);
        if link.share.is_owner(self.profile.id()) {
            self.ended.insert(id, link.share.ended(now_ms()));
        }
        let mut out = Vec::new();
        out.extend(relay.and_then(|r| self.relay_send(r, &RelayMessage::Unregister { share: id })));
        for member in &link.share.members {
            if self.connected.contains_key(&member.device) {
                out.push(send(
                    member.device,
                    &Payload::Share {
                        share: id,
                        sealed: ShareMessage::Ended.seal(&link.share.key),
                    },
                ));
            }
        }
        out.push(Action::Event(DriverEvent::ShareRemoved {
            share: id,
            reason: Removal::Ended,
        }));
        out
    }

    /// A member walks away from a share it does not own.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::leave_share"))]
    pub fn leave_share(&mut self, id: ShareId) -> Vec<Action> {
        // Also in the mailbox, for an owner away for longer than this
        // device keeps it in mind.
        let me = self.profile.id();
        let joined_at = self
            .share(id)
            .and_then(|s| s.member(me))
            .map_or(0, |m| m.joined_at);
        let left = ShareMessage::Left { joined_at };
        let recorded = self.share(id).and_then(|share| {
            let relay = self.share_relay(id)?;
            // Not with a key the relay is past: that one may be out there.
            if !self.mailbox_ready(id, relay) {
                return None;
            }
            let sealed = left.seal_signed(share, RecordKind::Left, self.profile.key());
            let publish = RelayMessage::Publish {
                share: id,
                kind: RecordKind::Left,
                sealed,
                epoch: share.epoch,
            };
            self.relay_send(relay, &publish)
        });
        let Some(link) = self.shares.remove(&id) else {
            return Vec::new();
        };
        let server = link
            .share
            .server
            .clone()
            .or_else(|| self.relay().map(ToString::to_string));
        self.left
            .insert(id, link.share.left(server, joined_at, now_ms()));
        self.forget_held(id, None);
        let mut out: Vec<Action> = recorded.into_iter().collect();
        out.extend(self.hold(
            link.share.owner.device,
            id,
            Held::Left,
            Payload::Share {
                share: id,
                sealed: left.seal(&link.share.key),
            },
        ));
        out.push(Action::Event(DriverEvent::ShareRemoved {
            share: id,
            reason: Removal::Left,
        }));
        out
    }

    /// Accept an invitation: connect to the owner and ask in.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::join"))]
    ///
    /// The invitation's relay serves that share only; it never becomes
    /// this device's own.
    pub fn join(&mut self, invite: Invite) -> Vec<Action> {
        self.join_inner(invite, None)
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::join_inner"))]
    fn join_inner(&mut self, invite: Invite, ended: Option<u64>) -> Vec<Action> {
        let owner = invite.owner.device;
        let share = invite.share;
        self.known_keys.insert(owner, invite.owner.public);
        if self.shares.contains_key(&share) {
            return Vec::new();
        }
        self.joins.insert(
            share,
            JoinIntent {
                invite,
                since: Instant::now(),
                awaiting_approval: false,
                ended,
            },
        );
        // The invitation's relay may not be ours.
        let mut out = self.sync_relays();
        out.extend(self.join_request(share));
        out
    }

    /// Ask the owner in when a channel to it is open; otherwise open one,
    /// on the network or, after knocking, through the invitation's relay.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::join_request"))]
    fn join_request(&self, share: ShareId) -> Vec<Action> {
        let Some(intent) = self.joins.get(&share) else {
            return Vec::new();
        };
        let owner = intent.invite.owner.device;
        if self.connected.contains_key(&owner) {
            return vec![send(
                owner,
                &Payload::Join {
                    share,
                    ask: JoinAsk {
                        name: self.profile.display_name().to_owned(),
                        device_name: self.profile.name().to_owned(),
                        proof: intent.invite.proof(self.profile.key().public()),
                        sign: self.profile.key().sign_public(),
                        ended: intent.ended,
                    },
                },
            )];
        }
        if let Some(peer) = self.discovered.get(&owner) {
            return vec![Action::Connect {
                addr: peer.addr,
                public: peer.public,
            }];
        }
        // Through the relay the owner only takes our channel once we
        // knocked; the relay's `Knocked` says when to dial.
        let relay = intent
            .invite
            .server
            .as_deref()
            .and_then(relay_device)
            .or(self.own_relay);
        relay
            .and_then(|relay| {
                self.relay_send(
                    relay,
                    &RelayMessage::Knock {
                        to: intent.invite.owner.public,
                        token: KnockToken::share(&intent.invite.key, share),
                    },
                )
            })
            .into_iter()
            .collect()
    }

    /// Deliver `payload` to `device` down an open channel. Nothing is
    /// held otherwise: rounds and announcements are made again once a
    /// channel opens.
    fn route(&self, device: DeviceId, payload: &Payload) -> Option<Action> {
        self.connected
            .contains_key(&device)
            .then(|| send(device, payload))
    }

    /// Deliver a one-shot message about `share` to `device`, or hold it
    /// until a channel opens and try to open one now. A newer message of
    /// the same kind replaces the one held; different edits add up.
    fn hold(
        &mut self,
        device: DeviceId,
        share: ShareId,
        kind: Held,
        payload: Payload,
    ) -> Option<Action> {
        if self.connected.contains_key(&device) {
            return Some(send(device, &payload));
        }
        let queue = self.outbox.entry(device).or_default();
        queue.retain(|w| w.share != share || w.kind != kind);
        if queue.len() >= OUTBOX_MAX {
            queue.remove(0);
        }
        queue.push(Waiting {
            share,
            kind,
            payload,
            since: Instant::now(),
        });
        self.connect_to(device)
    }

    /// Drop what is held about `share`, for `device` or for everyone.
    fn forget_held(&mut self, share: ShareId, device: Option<DeviceId>) {
        for (to, queue) in &mut self.outbox {
            if device.is_none_or(|d| d == *to) {
                queue.retain(|w| w.share != share);
            }
        }
        self.outbox.retain(|_, queue| !queue.is_empty());
    }

    /// Owner side: a new lease on `id` when the one held no longer fits
    /// the share or is a day old. Returns whether one was issued.
    fn renew_lease(&mut self, id: ShareId) -> bool {
        let me = self.profile.id();
        let now = now_ms();
        self.shares
            .get_mut(&id)
            .filter(|link| link.share.is_owner(me))
            .is_some_and(|link| link.share.renew_lease(self.profile.key(), now))
    }

    /// Whether this device waits for the owner of `id` before it shares
    /// with anyone else: its lease ran out, or names a newer key or other
    /// members.
    #[must_use]
    pub fn waiting_for_owner(&self, id: ShareId) -> bool {
        self.share(id).is_some() && !self.lease_holds(id)
    }

    /// Whether this device may share `id` with others than its owner:
    /// always as the owner, else only under the owner's lease.
    #[must_use]
    pub fn lease_holds(&self, id: ShareId) -> bool {
        self.share(id).is_some_and(|share| {
            if share.is_owner(self.profile.id()) {
                return true;
            }
            // Neither a clock put back (the time seen before stays) nor a
            // lease kept past its days since it arrived here.
            let now = now_ms().max(share.clock_mark);
            let held_for = share.lease.as_ref().and_then(|l| {
                self.lease_seen
                    .get(&id)
                    .filter(|(seq, _)| *seq == l.lease.seq)
                    .map(|(_, since)| since.elapsed())
            });
            share.lease_holds(now)
                && held_for.is_none_or(|d| d <= Duration::from_millis(crate::share::LEASE_MS))
        })
    }

    /// Note the time this device has seen and when the lease on `id`
    /// arrived here, for [`Self::lease_holds`].
    fn mark_lease_clock(&mut self, id: ShareId) {
        let now = now_ms();
        let Some(link) = self.shares.get_mut(&id) else {
            return;
        };
        link.share.clock_mark = link.share.clock_mark.max(now);
        if let Some(seq) = link.share.lease.as_ref().map(|l| l.lease.seq)
            && self
                .lease_seen
                .get(&id)
                .is_none_or(|(held, _)| *held != seq)
        {
            self.lease_seen.insert(id, (seq, Instant::now()));
        }
    }

    /// Whether the share's content may go to `device`: its owner, or a
    /// participant while the lease holds.
    fn may_share(&self, id: ShareId, device: DeviceId) -> bool {
        self.share(id).is_some_and(|share| {
            share.is_owner(device) || (share.is_member(device) && self.lease_holds(id))
        })
    }

    /// Owner side: the newest lease on `id` to every member with an open
    /// channel and to the mailbox, renewed first when due.
    fn spread_lease(&mut self, id: ShareId) -> Vec<Action> {
        self.renew_lease(id);
        let me = self.profile.id();
        let Some(share) = self.share(id).filter(|s| s.is_owner(me)) else {
            return Vec::new();
        };
        let Some(lease) = share.lease.clone() else {
            return Vec::new();
        };
        let mut out: Vec<Action> = share
            .members
            .iter()
            .filter(|m| self.connected.contains_key(&m.device))
            .map(|m| {
                send(
                    m.device,
                    &Payload::Lease {
                        share: id,
                        lease: lease.clone(),
                    },
                )
            })
            .collect();
        out.extend(self.lease_record(id));
        out
    }

    /// The newest lease on `id` for `device`: from the owner to any
    /// member, between members only while it holds, so a device the
    /// lease leaves out gets nothing.
    fn lease_for(&mut self, id: ShareId, device: DeviceId) -> Option<Action> {
        self.renew_lease(id);
        if !self.may_share(id, device) {
            return None;
        }
        let lease = self.share(id)?.lease.clone()?;
        Some(send(device, &Payload::Lease { share: id, lease }))
    }

    /// Owner side: the newest lease on `id` for the share's mailbox.
    fn lease_record(&self, id: ShareId) -> Option<Action> {
        let share = self.share(id).filter(|s| s.is_owner(self.profile.id()))?;
        let lease = share.lease.clone()?;
        let sealed = ShareMessage::Lease(Box::new(lease)).seal_signed(
            share,
            RecordKind::Lease,
            self.profile.key(),
        );
        let publish = RelayMessage::Publish {
            share: id,
            kind: RecordKind::Lease,
            sealed,
            epoch: share.epoch,
        };
        self.relay_send(self.share_relay(id)?, &publish)
    }

    /// Member side: `signed` from `from`, the owner or a member passing
    /// it on. A lease on a newer key says this device missed a change:
    /// it shares nothing until the owner brings it up to date.
    fn take_lease(
        &mut self,
        id: ShareId,
        signed: SignedLease,
        out: &mut Vec<Action>,
    ) -> Result<(), SyncError<R::Error>> {
        let Some(link) = self.shares.get_mut(&id) else {
            return Ok(());
        };
        let now = now_ms().max(link.share.clock_mark);
        if !link.share.offer_lease(signed, now) {
            return Ok(());
        }
        self.mark_lease_clock(id);
        // Whatever it says, everyone connected inside learns it now.
        let peers: Vec<DeviceId> = self
            .share(id)
            .map(|s| s.participants().map(|m| m.device).collect())
            .unwrap_or_default();
        let peers: Vec<DeviceId> = peers
            .into_iter()
            .filter(|p| self.connected.contains_key(p))
            .collect();
        for peer in peers {
            out.extend(self.lease_for(id, peer));
        }
        // A lease that does not hold here (newer key, other members): the
        // owner brings this device up to date.
        if !self.lease_holds(id) {
            let owner = self.share(id).map(|s| s.owner.device);
            out.extend(owner.and_then(|owner| self.connect_to(owner)));
        }
        self.lease_settled(id, out)
    }

    /// After the lease or the view of `id` changed: tell the app, and hand
    /// in what waited for the lease to hold.
    fn lease_settled(
        &mut self,
        id: ShareId,
        out: &mut Vec<Action>,
    ) -> Result<(), SyncError<R::Error>> {
        self.note_lease(id, out);
        if self.lease_holds(id) {
            out.extend(self.relay_publish()?);
        }
        Ok(())
    }

    /// Tell the app when the lease on `id` starts or stops holding.
    fn note_lease(&mut self, id: ShareId, out: &mut Vec<Action>) {
        let holds = self.lease_holds(id);
        if self.share(id).is_none() {
            self.leases.remove(&id);
            return;
        }
        let before = self.leases.insert(id, holds);
        // First look: only a lease already gone is news.
        if before != Some(holds) && (before.is_some() || !holds) {
            out.push(Action::Event(if holds {
                DriverEvent::LeaseRenewed(id)
            } else {
                DriverEvent::LeaseExpired(id)
            }));
        }
    }

    /// Owner side: the share's keys for one member over its channel,
    /// once the key changed at least once. A member that missed the
    /// change, the mailbox not keeping it, catches up this way.
    fn keys_for(&self, id: ShareId, device: DeviceId) -> Option<Action> {
        let share = self
            .share(id)
            .filter(|s| s.is_owner(self.profile.id()) && s.epoch > 0)?;
        let member = share.member(device)?;
        let keys: Vec<ShareKey> = std::iter::once(share.key.clone())
            .chain(share.previous.iter().cloned())
            .collect();
        let sealed = seal_keys(self.profile.key(), &member.public, id, share.epoch, &keys)?;
        Some(send(device, &Payload::Keys { share: id, sealed }))
    }

    /// Owner's current view for one member, as [`Self::broadcast_meta`].
    fn meta_for(&mut self, id: ShareId, device: DeviceId) -> Option<Action> {
        self.renew_lease(id);
        let link = self.shares.get(&id)?;
        if !link.share.is_owner(self.profile.id()) || !link.share.is_member(device) {
            return None;
        }
        Some(send(
            device,
            &Payload::Share {
                share: id,
                sealed: ShareMessage::Meta(Box::new(link.share.for_member())).seal(&link.share.key),
            },
        ))
    }

    /// Owner's current view sent to every connected member.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::broadcast_meta"))]
    fn broadcast_meta(&mut self, id: ShareId) -> Vec<Action> {
        self.renew_lease(id);
        let Some(link) = self.shares.get(&id) else {
            return Vec::new();
        };
        if !link.share.is_owner(self.profile.id()) {
            return Vec::new();
        }
        let sealed = ShareMessage::Meta(Box::new(link.share.for_member())).seal(&link.share.key);
        link.share
            .members
            .iter()
            .filter(|m| self.connected.contains_key(&m.device))
            .map(|m| {
                send(
                    m.device,
                    &Payload::Share {
                        share: id,
                        sealed: sealed.clone(),
                    },
                )
            })
            .collect()
    }

    /// Connect to `device` if we know where it is: on the network, or
    /// through a relay it can be reached at.
    fn connect_to(&self, device: DeviceId) -> Option<Action> {
        if let Some(peer) = self.discovered.get(&device) {
            return Some(Action::Connect {
                addr: peer.addr,
                public: peer.public,
            });
        }
        let public = *self.known_keys.get(&device)?;
        let relay = self.relay_for(device)?;
        Some(Action::ConnectVia { relay, public })
    }

    /// Approve a direct path to `device` when its channel runs through a
    /// relay and it is trusted or a fellow member, never for strangers.
    fn punch(&self, device: DeviceId) -> Option<Action> {
        let relayed = self.connected.get(&device).is_some_and(|p| p.relayed);
        (relayed && self.is_related(device)).then_some(Action::Punch { device })
    }

    // ---------- Events ------------------------------------------------

    /// React to the transport.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::on_event"))]
    pub fn on_event(&mut self, event: NetEvent) -> Result<Vec<Action>, SyncError<R::Error>> {
        let mut out = Vec::new();
        match event {
            NetEvent::Discovered(peer) => self.on_discovered(peer, &mut out),
            NetEvent::Connected(peer) => self.on_connected(peer, &mut out)?,
            NetEvent::Disconnected(device) => self.on_disconnected(device, &mut out),
            NetEvent::Renamed(peer) => {
                if let Some(open) = self.connected.get_mut(&peer.device) {
                    open.name.clone_from(&peer.name);
                }
                self.refresh_name(&peer, &mut out);
            }
            NetEvent::Message { from, bytes } => self.on_message(from, &bytes, &mut out)?,
            NetEvent::PathChanged { device, relayed } => {
                if let Some(peer) = self.connected.get_mut(&device) {
                    peer.relayed = relayed;
                    let path = if relayed { Path::Relay } else { Path::Direct };
                    out.push(Action::Event(DriverEvent::PathChanged { device, path }));
                }
            }
            NetEvent::ConnectFailed { addr, .. } => self.on_connect_failed(addr, &mut out),
            NetEvent::Stopped(_) => self.on_stopped(&mut out),
        }
        out.extend(self.bulk_changes());
        Ok(out)
    }

    /// Which connected devices the transport puts payloads in parts
    /// together for, as it changed: those this device deals with — its
    /// relays, trusted devices, fellow members, the owner it asks into a
    /// share, the device it pairs with. A stranger only gets small ones
    /// through, so it can make this device hold nothing big.
    fn bulk_changes(&mut self) -> Vec<Action> {
        self.bulk
            .retain(|device, _| self.connected.contains_key(device));
        let devices: Vec<DeviceId> = self.connected.keys().copied().collect();
        let mut out = Vec::new();
        for device in devices {
            let allowed = self.relays.contains_key(&device)
                || self.is_related(device)
                || self.joins.values().any(|j| j.invite.owner.device == device)
                || self.intent.as_ref().is_some_and(|i| i.device == device);
            // The transport starts with nobody allowed.
            if self.bulk.insert(device, allowed).unwrap_or(false) != allowed {
                out.push(Action::Bulk { device, allowed });
            }
        }
        out
    }

    /// Trusted, or a member of a share with this device.
    fn is_related(&self, device: DeviceId) -> bool {
        self.profile.is_trusted(device) || !self.shares_with(device).is_empty()
    }

    /// Joins waiting on `owner`.
    fn joins_to(&self, owner: DeviceId) -> Vec<ShareId> {
        self.joins
            .iter()
            .filter(|(_, j)| j.invite.owner.device == owner)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Keep a trusted device's stored name in step with the one it
    /// announces.
    fn refresh_name(&mut self, peer: &PeerInfo, out: &mut Vec<Action>) {
        let Some(known) = self
            .profile
            .trusted()
            .iter()
            .find(|d| d.id == peer.device)
            .filter(|d| d.name != peer.name && !peer.name.is_empty())
            .cloned()
        else {
            return;
        };
        self.profile.trust(TrustedDevice {
            name: peer.name.clone(),
            ..known
        });
        out.push(Action::Event(DriverEvent::Renamed {
            device: peer.device,
            name: peer.name.clone(),
        }));
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::on_discovered"))]
    fn on_discovered(&mut self, peer: PeerInfo, out: &mut Vec<Action>) {
        self.refresh_name(&peer, out);
        let device = peer.device;
        self.known_keys.insert(device, peer.public);
        let wants_pair = self.intent.as_ref().is_some_and(|i| i.device == device);
        let wants_join = self.joins.values().any(|j| j.invite.owner.device == device);
        // Between two related devices only the smaller id dials.
        let initiates = self.profile.id() < device;
        if !self.connected.contains_key(&device)
            && (wants_pair || wants_join || (self.is_related(device) && initiates))
        {
            out.push(Action::Connect {
                addr: peer.addr,
                public: peer.public,
            });
        }
        self.discovered.insert(device, peer);
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::on_connected"))]
    fn on_connected(
        &mut self,
        peer: PeerInfo,
        out: &mut Vec<Action>,
    ) -> Result<(), SyncError<R::Error>> {
        self.refresh_name(&peer, out);
        let device = peer.device;
        if self.retired.contains(&device) {
            out.push(Action::Disconnect { device });
            return Ok(());
        }
        self.known_keys.insert(device, peer.public);
        // A channel can replace another without a disconnect in between;
        // what the old one carried may never have arrived.
        self.engine.forget_peer(device);
        for link in self.shares.values_mut() {
            link.engine.forget_peer(device);
        }
        let restarted = self
            .connected
            .get(&device)
            .is_some_and(|known| known.session != peer.session);
        self.connected.insert(device, peer);
        if restarted && self.is_related(device) {
            // Another run of the app: whatever the old one was doing
            // (ink under the pen, a pointer, a place in a document) is gone.
            out.push(Action::Event(DriverEvent::PeerOnline {
                device,
                online: false,
            }));
        }
        if let Some(relay) = self.relays.get_mut(&device) {
            relay.connected = true;
            out.extend(self.relay_hello(device)?);
            return Ok(());
        }
        if self.is_related(device) {
            out.push(Action::Event(DriverEvent::PeerOnline {
                device,
                online: true,
            }));
        }
        out.extend(self.punch(device));
        if self.profile.is_trusted(device) {
            out.extend(self.round_for(device)?);
        }
        for share in self.shares_with(device) {
            // Keys, policy, viewers or scope may have changed while it was
            // away; keys first, or the rest does not open.
            out.extend(self.keys_for(share, device));
            out.extend(self.meta_for(share, device));
            out.extend(self.lease_for(share, device));
            out.extend(self.share_round_for(share, device)?);
        }
        if self.intent.as_ref().is_some_and(|i| i.device == device) {
            out.push(self.pair_request(device));
        }
        for share in self.joins_to(device) {
            out.extend(self.join_request(share));
        }
        for held in self.outbox.remove(&device).unwrap_or_default() {
            if held.since.elapsed() < OUTBOX_TTL {
                out.push(send(device, &held.payload));
            }
        }
        Ok(())
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::on_disconnected"))]
    fn on_disconnected(&mut self, device: DeviceId, out: &mut Vec<Action>) {
        self.connected.remove(&device);
        self.engine.forget_peer(device);
        if let Some(relay) = self.relays.get_mut(&device) {
            relay.connected = false;
            relay.published.clear();
            relay.epochs.clear();
            if self.own_relay == Some(device) {
                out.push(Action::Event(DriverEvent::RelayOnline(false)));
            }
        }
        for link in self.shares.values_mut() {
            link.engine.forget_peer(device);
        }
        if self.is_related(device) {
            out.push(Action::Event(DriverEvent::PeerOnline {
                device,
                online: false,
            }));
        }
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::on_message"))]
    fn on_message(
        &mut self,
        from: DeviceId,
        bytes: &[u8],
        out: &mut Vec<Action>,
    ) -> Result<(), SyncError<R::Error>> {
        #[cfg(feature = "sismo")]
        sismo::counter!("sync.in.bytes", bytes.len());
        let Some(payload) = Payload::from_bytes(bytes) else {
            #[cfg(feature = "sismo")]
            sismo::instant!("sync::in::undecodable");
            return Ok(());
        };
        match payload {
            Payload::Sync(bytes) => {
                #[cfg(feature = "sismo")]
                sismo::counter!("sync.in.sync_bytes", bytes.len());
                if self.profile.is_trusted(from) {
                    self.on_sync(from, &bytes, out)?;
                }
            }
            Payload::Pair(message) => self.on_pair(from, message, out),
            Payload::Share { share, sealed } => {
                #[cfg(feature = "sismo")]
                sismo::counter!("sync.in.share_bytes", sealed.len());
                self.on_share(from, share, &sealed, out)?;
            }
            Payload::Join { share, ask } => self.on_join(from, share, ask, out)?,
            Payload::JoinReply { share, sealed } => {
                self.on_join_reply(from, share, &sealed, out)?;
            }
            Payload::Lease { share, lease } => {
                // Only from someone inside: a stranger has nothing to say.
                if self
                    .share(share)
                    .is_some_and(|s| s.participants().any(|m| m.device == from))
                {
                    self.take_lease(share, lease, out)?;
                }
            }
            Payload::Keys { share, sealed } => {
                if self.take_keys(share, from, &sealed) {
                    // What waited for this key opens now, and what was held
                    // back for it may go to the mailbox.
                    if self.take_records(share, Vec::new(), out)? {
                        out.push(Action::Event(DriverEvent::ShareChanged(share)));
                    }
                    self.lease_settled(share, out)?;
                }
            }
            Payload::Relay(bytes) => {
                #[cfg(feature = "sismo")]
                sismo::counter!("sync.in.relay_bytes", bytes.len());
                self.on_relay(from, &bytes, out)?;
            }
            Payload::Live { doc, bytes } => {
                #[cfg(feature = "sismo")]
                sismo::counter!("sync.in.live_bytes", bytes.len());
                if self.profile.is_trusted(from) {
                    out.push(Action::Event(DriverEvent::Live {
                        from,
                        name: self.peer_name(from),
                        doc,
                        bytes,
                    }));
                }
            }
        }
        Ok(())
    }

    /// A dial failed: a pairing or join waiting on that device fails too.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::on_connect_failed"))]
    fn on_connect_failed(&mut self, addr: SocketAddr, out: &mut Vec<Action>) {
        let Some(device) = self
            .discovered
            .values()
            .find(|p| p.addr == addr)
            .map(|p| p.device)
        else {
            return;
        };
        if self.intent.as_ref().is_some_and(|i| i.device == device) {
            self.intent = None;
            out.push(Action::Event(DriverEvent::PairFailed {
                device,
                reason: PairFailure::Unreachable,
            }));
        }
        for share in self.joins_to(device) {
            self.joins.remove(&share);
            out.push(Action::Event(DriverEvent::JoinFailed {
                share,
                reason: JoinFailure::Unreachable,
            }));
        }
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::on_stopped"))]
    fn on_stopped(&mut self, out: &mut Vec<Action>) {
        for device in self.connected.keys().copied() {
            self.engine.forget_peer(device);
            for link in self.shares.values_mut() {
                link.engine.forget_peer(device);
            }
            if self.is_related(device) {
                out.push(Action::Event(DriverEvent::PeerOnline {
                    device,
                    online: false,
                }));
            }
        }
        self.connected.clear();
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::on_sync"))]
    fn on_sync(
        &mut self,
        from: DeviceId,
        bytes: &[u8],
        out: &mut Vec<Action>,
    ) -> Result<(), SyncError<R::Error>> {
        let message = Message::from_bytes(bytes)?;
        let handled = self.engine.handle(from, message)?;
        for reply in &handled.replies {
            out.push(send(from, &Payload::Sync(reply.to_bytes()?)));
        }
        if !handled.imported.is_empty() {
            out.push(Action::Event(DriverEvent::Imported(handled.imported)));
        }
        if !handled.merged.is_empty() {
            out.push(Action::Event(DriverEvent::Merged(handled.merged)));
        }
        Ok(())
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::on_share"))]
    fn on_share(
        &mut self,
        from: DeviceId,
        id: ShareId,
        sealed: &[u8],
        out: &mut Vec<Action>,
    ) -> Result<(), SyncError<R::Error>> {
        let name = self.peer_name(from);
        if let Some(reply) = self.removal_for(from, id, sealed) {
            out.extend(reply);
            return Ok(());
        }
        // Without the owner's lease, rounds from other members are taken
        // in but not answered with content. One still writing with an
        // earlier key missed a change: it hears of the newest lease.
        let answers = self.may_share(id, from);
        let behind = self
            .share(id)
            .is_some_and(|s| s.key_opening(sealed).is_some_and(|k| *k != s.key));
        if behind {
            out.extend(self.lease_for(id, from));
        }
        let Some(link) = self.shares.get_mut(&id) else {
            return Ok(());
        };
        if !link.share.is_member(from) {
            return Ok(());
        }
        let Some(message) = ShareMessage::open_in(&link.share, sealed) else {
            return Ok(());
        };
        let me = self.profile.id();
        let mut round = false;
        let mut moved = false;
        let viewed = matches!(message, ShareMessage::Meta(_));
        match message {
            ShareMessage::Live { doc, bytes } => {
                #[cfg(feature = "sismo")]
                sismo::counter!("sync.in.share_live_bytes", bytes.len());
                if link.share.docs.contains(&doc) {
                    out.push(Action::Event(DriverEvent::Live {
                        from,
                        name,
                        doc,
                        bytes,
                    }));
                }
            }
            ShareMessage::Sync(bytes) => return self.on_share_sync(from, id, &bytes, answers, out),
            ShareMessage::Meta(share) => {
                if let Some(fresh) = fresh_meta(&link.share, from, me, *share) {
                    if fresh == link.share {
                        return Ok(());
                    }
                    moved = fresh.server != link.share.server;
                    link.share = fresh;
                    link.engine.set_scope(link.share.scope());
                    out.push(Action::Event(DriverEvent::ShareChanged(id)));
                    if link.share.is_member(me) {
                        // Documents just brought in (often by this very
                        // member) travel now, not on the next tick.
                        round = true;
                    } else {
                        self.shares.remove(&id);
                        out.push(Action::Event(DriverEvent::ShareRemoved {
                            share: id,
                            reason: Removal::Expelled,
                        }));
                    }
                }
            }
            ShareMessage::Ended => {
                self.removed_by_owner(from, id, Removal::Ended, out);
            }
            ShareMessage::Expelled => {
                self.removed_by_owner(from, id, Removal::Expelled, out);
            }
            ShareMessage::Left { joined_at } => {
                if link.share.is_owner(me) && link.share.depart(from, joined_at) {
                    link.engine.forget_peer(from);
                    out.extend(self.rekey(id));
                }
            }
            ShareMessage::Edit(edit) => {
                self.apply_edit(from, id, edit, out);
            }
            ShareMessage::Lease(signed) => self.take_lease(id, *signed, out)?,
        }
        if moved {
            out.extend(self.sync_relays());
            out.extend(self.relay_subscribe(id));
        }
        if viewed {
            self.lease_settled(id, out)?;
        }
        if round {
            out.extend(self.share_round_for(id, from)?);
        }
        Ok(())
    }

    /// An exchange message inside share `id` from `from`; answered with
    /// content only when `answers`.
    fn on_share_sync(
        &mut self,
        from: DeviceId,
        id: ShareId,
        bytes: &[u8],
        answers: bool,
        out: &mut Vec<Action>,
    ) -> Result<(), SyncError<R::Error>> {
        #[cfg(feature = "sismo")]
        sismo::counter!("sync.in.share_sync_bytes", bytes.len());
        let Some(link) = self.shares.get_mut(&id) else {
            return Ok(());
        };
        let message = Message::from_bytes(bytes)?;
        // An announcement left unanswered must not count as answered.
        if !answers && matches!(message, Message::Inventory { .. }) {
            return Ok(());
        }
        let Some(message) = permitted(&link.share, from, message) else {
            return Ok(());
        };
        let handled = link.engine.handle(from, message)?;
        for reply in handled.replies.iter().filter(|_| answers) {
            out.push(send(
                from,
                &Payload::Share {
                    share: id,
                    sealed: ShareMessage::Sync(reply.to_bytes()?).seal(&link.share.key),
                },
            ));
        }
        if !handled.imported.is_empty() {
            out.push(Action::Event(DriverEvent::Imported(handled.imported)));
        }
        if !handled.merged.is_empty() {
            out.push(Action::Event(DriverEvent::Merged(handled.merged)));
        }
        Ok(())
    }

    /// Tell someone who knocked whether they are in. The newcomer's own
    /// first round brings the content over, as after any welcome.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::answer_join"))]
    fn answer_join(&mut self, id: ShareId, device: DeviceId, admitted: bool) -> Vec<Action> {
        let Some(link) = self.shares.get_mut(&id) else {
            return Vec::new();
        };
        // Sealed with the key the joiner proved it holds, which may be one
        // the share had before. Such a key may be known to devices that are
        // out since, so the answer only goes down the joiner's own channel,
        // never to the mailbox; one away asks again on its return and, let
        // in by then, is welcomed at once.
        let key = link.share.take_knock_key(device);
        link.share.renew_lease(self.profile.key(), now_ms());
        let answer = if admitted {
            JoinAnswer::Welcome(Box::new(link.share.for_member()))
        } else {
            JoinAnswer::Refused
        };
        let sealed = seal_answer(&key, &answer);
        let mut out: Vec<Action> = self
            .route(device, &Payload::JoinReply { share: id, sealed })
            .into_iter()
            .collect();
        if admitted {
            out.push(Action::Event(DriverEvent::PeerOnline {
                device,
                online: true,
            }));
            out.extend(self.punch(device));
        }
        out
    }

    /// The explicit answer for a member that still writes into a share
    /// this device closed, or threw it out of, while it was away. `None`
    /// when `from` is still welcome or was never inside; empty when the
    /// answer has no way to it.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::removal_for"))]
    fn removal_for(&self, from: DeviceId, id: ShareId, sealed: &[u8]) -> Option<Vec<Action>> {
        let me = self.profile.id();
        let (key, message) = if let Some(link) = self.shares.get(&id) {
            let share = &link.share;
            if !share.is_owner(me) || !share.was_expelled(from) {
                return None;
            }
            // Told with the key it still holds, from before it was out.
            (share.key_opening(sealed)?, ShareMessage::Expelled)
        } else {
            let ended = self.ended.get(&id)?;
            if !ended.members.contains(&from) {
                return None;
            }
            (&ended.key, ShareMessage::Ended)
        };
        // Only someone holding the key hears about it.
        ShareMessage::open(key, sealed)?;
        Some(
            self.route(
                from,
                &Payload::Share {
                    share: id,
                    sealed: message.seal(key),
                },
            )
            .into_iter()
            .collect(),
        )
    }

    /// The owner closed the share for this device: drop it and say
    /// why. Returns whether anything was dropped.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::removed_by_owner"))]
    fn removed_by_owner(
        &mut self,
        from: DeviceId,
        id: ShareId,
        reason: Removal,
        out: &mut Vec<Action>,
    ) -> bool {
        if !self.share(id).is_some_and(|s| s.is_owner(from)) {
            return false;
        }
        self.shares.remove(&id);
        out.push(Action::Event(DriverEvent::ShareRemoved {
            share: id,
            reason,
        }));
        true
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::on_join"))]
    fn on_join(
        &mut self,
        from: DeviceId,
        id: ShareId,
        ask: JoinAsk,
        out: &mut Vec<Action>,
    ) -> Result<(), SyncError<R::Error>> {
        let Some(public) = self.known_keys.get(&from).copied() else {
            return Ok(());
        };
        let me = self.profile.id();
        // Back after leaving before its departure reached us: the
        // membership it left ends here, as if it had.
        if let Some(ended) = ask.ended
            && let Some(link) = self.shares.get_mut(&id)
            && link.share.is_owner(me)
            && link.share.depart(from, ended)
        {
            link.engine.forget_peer(from);
            out.extend(self.rekey(id));
        }
        // Answers go sealed with the key the joiner proved it holds.
        let key = self
            .shares
            .get(&id)
            .filter(|link| link.share.is_owner(me))
            .and_then(|link| proof_key(&link.share, &ask.proof, &public).cloned());
        let answer = match (self.shares.get_mut(&id), key) {
            // Already in, back after a restart: it stays as it is, role and
            // all, and only hears the share again.
            (Some(link), Some(key)) if link.share.is_member(from) => {
                link.share.renew_lease(self.profile.key(), now_ms());
                let welcome = JoinAnswer::Welcome(Box::new(link.share.for_member()));
                let sealed = seal_answer(&key, &welcome);
                out.extend(self.route(from, &Payload::JoinReply { share: id, sealed }));
                out.extend(self.share_round_for(id, from)?);
                return Ok(());
            }
            (Some(link), Some(key)) => {
                let member = Member {
                    device: from,
                    public,
                    sign: ask.sign,
                    name: ask.name,
                    device_name: ask.device_name,
                    role: link.share.entry_role,
                    joined_at: now_ms(),
                };
                // One back after leaving waits as any newcomer does now; one
                // thrown out always does, whatever the invitation says, until
                // an organizer lets it back in.
                if link.share.approve_entry || link.share.was_expelled(from) {
                    link.share.remember_knock_key(from, key.clone());
                    let fresh = link.share.knock(member.clone());
                    let sealed = seal_answer(&key, &JoinAnswer::Pending);
                    out.extend(self.route(from, &Payload::JoinReply { share: id, sealed }));
                    if fresh {
                        out.push(Action::Event(DriverEvent::EntryRequested {
                            share: id,
                            member,
                        }));
                        out.push(Action::Event(DriverEvent::ShareChanged(id)));
                    }
                    return Ok(());
                }
                link.share.admit(member);
                link.share.touch(now_ms());
                link.share.renew_lease(self.profile.key(), now_ms());
                let welcome = JoinAnswer::Welcome(Box::new(link.share.for_member()));
                let sealed = seal_answer(&key, &welcome);
                // The relay learns the new member before the welcome goes
                // out, so the member's first subscription is accepted.
                out.extend(self.relay_register(id));
                out.extend(self.route(from, &Payload::JoinReply { share: id, sealed }));
                true
            }
            _ => false,
        };
        if answer {
            out.extend(self.broadcast_meta(id));
            out.push(Action::Event(DriverEvent::ShareChanged(id)));
            out.push(Action::Event(DriverEvent::PeerOnline {
                device: from,
                online: true,
            }));
            out.extend(self.punch(from));
            out.extend(self.share_round_for(id, from)?);
            out.extend(self.relay_publish()?);
        } else {
            // Without the key we cannot seal a refusal the joiner can
            // read; an unsealed empty reply means "no".
            out.extend(self.route(
                from,
                &Payload::JoinReply {
                    share: id,
                    sealed: Vec::new(),
                },
            ));
        }
        Ok(())
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::on_join_reply"))]
    fn on_join_reply(
        &mut self,
        from: DeviceId,
        id: ShareId,
        sealed: &[u8],
        out: &mut Vec<Action>,
    ) -> Result<(), SyncError<R::Error>> {
        let Some(intent) = self.joins.get(&id) else {
            return Ok(());
        };
        if intent.invite.owner.device != from {
            return Ok(());
        }
        let answer = intent
            .invite
            .key
            .open(sealed)
            .ok()
            .and_then(|plain| crate::decode(&plain, crate::cap::ANSWER).ok());
        if matches!(answer, Some(JoinAnswer::Pending)) {
            if let Some(intent) = self.joins.get_mut(&id)
                && !intent.awaiting_approval
            {
                intent.awaiting_approval = true;
                out.push(Action::Event(DriverEvent::JoinPending(id)));
            }
            return Ok(());
        }
        let intent = self.joins.remove(&id).map(|j| j.invite);
        match (answer, intent) {
            // The owner signs mailbox records with the key the invitation named.
            (Some(JoinAnswer::Welcome(share)), Some(invite))
                if share.id == id
                    && share.owner.public == invite.owner.public
                    && share.owner.sign == invite.owner.sign =>
            {
                let server = share.server.clone().or(invite.server);
                // The key is the owner's current one, which differs from the
                // invitation's for a member coming back after it changed.
                let share = Share {
                    server,
                    relay_cursor: 0,
                    ..*share
                };
                for member in share.participants() {
                    self.known_keys.insert(member.device, member.public);
                }
                let engine = SyncEngine::new(Arc::clone(&self.replica), share.scope());
                self.shares.insert(id, ShareLink { share, engine });
                out.extend(self.sync_relays());
                out.extend(self.relay_subscribe(id));
                out.push(Action::Event(DriverEvent::ShareChanged(id)));
                out.push(Action::Event(DriverEvent::PeerOnline {
                    device: from,
                    online: true,
                }));
                out.extend(self.punch(from));
                out.extend(self.share_round_for(id, from)?);
            }
            _ => out.push(Action::Event(DriverEvent::JoinFailed {
                share: id,
                reason: JoinFailure::Refused,
            })),
        }
        Ok(())
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::on_pair"))]
    fn on_pair(&mut self, from: DeviceId, message: PairMessage, out: &mut Vec<Action>) {
        let Some(peer) = self.connected.get(&from).cloned() else {
            return;
        };
        match message {
            PairMessage::Request { code, name, proof } => {
                let offer = self.offer.filter(|o| !o.expired());
                let held =
                    offer.is_some_and(|o| proof.is_some_and(|p| o.secret.proves(&peer.public, &p)));
                // Through a relay only the link pairs; on the network the
                // typed code may, when no proof is claimed.
                let shown = held || (!peer.through_relay && proof.is_none());
                let accepted = shown && offer.is_some_and(|o| o.code.matches(code));
                // A guess from afar must not burn the offer for the device
                // in front of the user.
                if accepted || !peer.through_relay {
                    self.offer = None;
                }
                if accepted {
                    let device = TrustedDevice::new(peer.public, name, now_ms());
                    self.profile.trust(device.clone());
                    out.push(send(
                        from,
                        &Payload::Pair(PairMessage::Accept {
                            name: self.profile.name().to_owned(),
                        }),
                    ));
                    out.push(Action::Event(DriverEvent::Paired(device)));
                    out.extend(self.relay_consent());
                    out.push(Action::Event(DriverEvent::PeerOnline {
                        device: from,
                        online: true,
                    }));
                    out.extend(self.punch(from));
                    if let Ok(round) = self.round_for(from) {
                        out.extend(round);
                    }
                } else {
                    out.push(send(from, &Payload::Pair(PairMessage::Reject)));
                }
            }
            PairMessage::Accept { name } => {
                let Some(intent) = self.intent.take_if(|i| i.device == from) else {
                    return;
                };
                let device = TrustedDevice::new(intent.public, name, now_ms());
                self.profile.trust(device.clone());
                out.push(Action::Event(DriverEvent::Paired(device)));
                out.extend(self.relay_consent());
                out.push(Action::Event(DriverEvent::PeerOnline {
                    device: from,
                    online: true,
                }));
                out.extend(self.punch(from));
                if let Ok(round) = self.round_for(from) {
                    out.extend(round);
                }
            }
            PairMessage::Reject => {
                if self.intent.take_if(|i| i.device == from).is_some() {
                    out.push(Action::Event(DriverEvent::PairFailed {
                        device: from,
                        reason: PairFailure::Rejected,
                    }));
                }
            }
        }
    }

    // ---------- Rounds ------------------------------------------------

    /// One round towards `device`: whatever it lacks according to its
    /// last announcement, then our own announcement so it answers back.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::round_for"))]
    fn round_for(&self, device: DeviceId) -> Result<Vec<Action>, SyncError<R::Error>> {
        let mut out = Vec::new();
        for message in self.engine.push_for(device)? {
            out.push(send(device, &Payload::Sync(message.to_bytes()?)));
        }
        out.push(send(
            device,
            &Payload::Sync(self.engine.inventory().to_bytes()?),
        ));
        Ok(out)
    }

    /// A round with the owner of `share`, over a channel opened for it
    /// when there is none: what it answers settles what the relay said.
    fn ask_owner(
        &self,
        share: ShareId,
        owner: DeviceId,
    ) -> Result<Vec<Action>, SyncError<R::Error>> {
        if self.connected.contains_key(&owner) {
            return self.share_round_for(share, owner);
        }
        // The round goes out once the channel is there.
        Ok(self.connect_to(owner).into_iter().collect())
    }

    /// Same as [`Self::round_for`] inside one share.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::share_round_for"))]
    fn share_round_for(
        &self,
        id: ShareId,
        device: DeviceId,
    ) -> Result<Vec<Action>, SyncError<R::Error>> {
        // Without the owner's lease, only the owner hears from us.
        if !self.may_share(id, device) {
            return Ok(Vec::new());
        }
        let Some(link) = self.shares.get(&id) else {
            return Ok(Vec::new());
        };
        let mut messages = link.engine.push_for(device)?;
        if !link.share.can_edit(self.profile.id()) {
            // Only read here: announce so the others send, never push.
            messages.clear();
        }
        messages.push(link.engine.inventory());
        let mut sealed = Vec::with_capacity(messages.len());
        for message in messages {
            sealed.push(ShareMessage::Sync(message.to_bytes()?).seal(&link.share.key));
        }
        Ok(sealed
            .into_iter()
            .filter_map(|sealed| self.route(device, &Payload::Share { share: id, sealed }))
            .collect())
    }

    /// A round towards every online trusted device and share member.
    /// Call after local edits land and on every [`TICK`].
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::announce"))]
    pub fn announce(&mut self) -> Result<Vec<Action>, SyncError<R::Error>> {
        let mut out = Vec::new();
        let devices: Vec<DeviceId> = self.online().map(|p| p.device).collect();
        for device in devices {
            out.extend(self.round_for(device)?);
        }
        let pairs: Vec<(ShareId, DeviceId)> = self
            .shares
            .values()
            .flat_map(|link| {
                link.share
                    .participants()
                    .filter(|m| m.device != self.profile.id())
                    .filter(|m| self.connected.contains_key(&m.device))
                    .map(move |m| (link.share.id, m.device))
            })
            .collect();
        for (share, device) in pairs {
            out.extend(self.share_round_for(share, device)?);
        }
        out.extend(self.relay_publish()?);
        Ok(out)
    }

    /// Push one document's new changes to everyone who may hold it,
    /// without waiting for a round: trusted devices in the clear, share
    /// members sealed, offline members through the relay mailbox.
    ///
    /// # Errors
    /// Replica failure while exporting.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::push_doc"))]
    pub fn push_doc(&mut self, doc: DocId) -> Result<Vec<Action>, SyncError<R::Error>> {
        let mut out = Vec::new();
        let devices: Vec<DeviceId> = self.online().map(|p| p.device).collect();
        for device in devices {
            if let Some(message) = self.engine.push_doc(device, doc)? {
                out.push(send(device, &Payload::Sync(message.to_bytes()?)));
            }
        }
        let me = self.profile.id();
        let ids: Vec<ShareId> = self.shares.keys().copied().collect();
        for id in ids {
            let Some((members, key)) = self.shares.get(&id).and_then(|link| {
                let share = &link.share;
                if !share.docs.contains(&doc) || !share.can_edit(me) {
                    return None;
                }
                let members: Vec<DeviceId> = share
                    .participants()
                    .filter(|m| m.device != me)
                    .filter(|m| self.connected.contains_key(&m.device))
                    .filter(|m| self.may_share(id, m.device))
                    .map(|m| m.device)
                    .collect();
                Some((members, share.key.clone()))
            }) else {
                continue;
            };
            let mut sealed = Vec::with_capacity(members.len());
            if let Some(link) = self.shares.get_mut(&id) {
                for device in members {
                    if let Some(message) = link.engine.push_doc(device, doc)? {
                        sealed.push((device, ShareMessage::Sync(message.to_bytes()?).seal(&key)));
                    }
                }
            }
            for (device, sealed) in sealed {
                out.extend(self.route(device, &Payload::Share { share: id, sealed }));
            }
        }
        out.extend(self.relay_publish()?);
        Ok(out)
    }

    /// Push one blob to everyone who may hold it, like [`Self::push_doc`].
    ///
    /// # Errors
    /// Replica failure while reading.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::push_blob"))]
    pub fn push_blob(&mut self, key: BlobKey) -> Result<Vec<Action>, SyncError<R::Error>> {
        let mut out = Vec::new();
        let devices: Vec<DeviceId> = self.online().map(|p| p.device).collect();
        for device in devices {
            if let Some(message) = self.engine.push_blob(device, key)? {
                out.push(send(device, &Payload::Sync(message.to_bytes()?)));
            }
        }
        let me = self.profile.id();
        let ids: Vec<ShareId> = self.shares.keys().copied().collect();
        for id in ids {
            let Some((members, share_key)) = self.shares.get(&id).and_then(|link| {
                let share = &link.share;
                if !share.can_edit(me) {
                    return None;
                }
                let members: Vec<DeviceId> = share
                    .participants()
                    .filter(|m| m.device != me)
                    .filter(|m| self.connected.contains_key(&m.device))
                    .filter(|m| self.may_share(id, m.device))
                    .map(|m| m.device)
                    .collect();
                Some((members, share.key.clone()))
            }) else {
                continue;
            };
            let mut sealed = Vec::with_capacity(members.len());
            if let Some(link) = self.shares.get_mut(&id) {
                for device in members {
                    if let Some(message) = link.engine.push_blob(device, key)? {
                        sealed.push((
                            device,
                            ShareMessage::Sync(message.to_bytes()?).seal(&share_key),
                        ));
                    }
                }
            }
            for (device, sealed) in sealed {
                out.extend(self.route(device, &Payload::Share { share: id, sealed }));
            }
        }
        out.extend(self.relay_publish()?);
        Ok(out)
    }

    /// Hand ephemeral state of `doc` to everyone looking at it right
    /// now: trusted devices in the clear, share members sealed. Nothing
    /// is queued for whoever is away.
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::live"))]
    pub fn live(&self, doc: DocId, bytes: &[u8]) -> Vec<Action> {
        let mut out: Vec<Action> = self
            .online()
            .map(|p| {
                send_live(
                    p.device,
                    &Payload::Live {
                        doc,
                        bytes: bytes.to_vec(),
                    },
                )
            })
            .collect();
        let me = self.profile.id();
        for link in self.shares.values() {
            if !link.share.docs.contains(&doc) {
                continue;
            }
            let sealed = ShareMessage::Live {
                doc,
                bytes: bytes.to_vec(),
            }
            .seal(&link.share.key);
            for member in link.share.participants() {
                if member.device == me
                    || !self.connected.contains_key(&member.device)
                    || !self.may_share(link.share.id, member.device)
                {
                    continue;
                }
                out.push(send_live(
                    member.device,
                    &Payload::Share {
                        share: link.share.id,
                        sealed: sealed.clone(),
                    },
                ));
            }
        }
        #[cfg(feature = "sismo")]
        {
            sismo::counter!("sync.live.bytes", bytes.len());
            sismo::counter!("sync.live.targets", out.len());
        }
        out
    }

    /// What to call `device` in the UI: its hello name, its trusted
    /// name, or the short id.
    fn peer_name(&self, device: DeviceId) -> String {
        self.connected
            .get(&device)
            .map(|p| p.name.clone())
            .or_else(|| {
                self.profile
                    .trusted()
                    .iter()
                    .find(|t| t.id == device)
                    .map(|t| t.name.clone())
            })
            .unwrap_or_else(|| device.short())
    }

    /// Periodic housekeeping plus an announcement round.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::tick"))]
    pub fn tick(&mut self) -> Result<Vec<Action>, SyncError<R::Error>> {
        #[cfg(feature = "sismo")]
        {
            sismo::counter!("sync.peers.discovered", self.discovered.len());
            sismo::counter!("sync.peers.connected", self.connected.len());
            sismo::counter!("sync.shares", self.shares.len());
            sismo::counter!("sync.joins_pending", self.joins.len());
            sismo::counter!("sync.redials", self.redials.len());
        }
        let mut out = Vec::new();
        // The owner keeps its leases fresh; members learn when theirs ran out.
        let ids: Vec<ShareId> = self.shares.keys().copied().collect();
        for id in ids {
            self.mark_lease_clock(id);
            if self.renew_lease(id) {
                out.extend(self.spread_lease(id));
            }
            self.note_lease(id, &mut out);
        }
        if self.offer.is_some_and(|o| o.expired()) {
            self.offer = None;
        }
        if let Some(intent) = self.intent.take_if(|i| i.since.elapsed() > OFFER_TTL) {
            out.push(Action::Event(DriverEvent::PairFailed {
                device: intent.device,
                reason: PairFailure::Unreachable,
            }));
        }
        let stale: Vec<ShareId> = self
            .joins
            .iter()
            .filter(|(_, j)| j.since.elapsed() > OFFER_TTL)
            .map(|(id, _)| *id)
            .collect();
        for share in stale {
            self.joins.remove(&share);
            out.push(Action::Event(DriverEvent::JoinFailed {
                share,
                reason: JoinFailure::Unreachable,
            }));
        }
        out.extend(self.sync_relays());
        let pending: Vec<ShareId> = self.joins.keys().copied().collect();
        for share in pending {
            out.extend(self.join_request(share));
        }
        // Consent follows pairings, revocations and offers opened since.
        out.extend(self.relay_consent());
        out.extend(self.pair_knock());
        // Subscriptions are cheap to repeat and heal a refused one once
        // the owner's registration has landed.
        for share in self.shares.keys() {
            out.extend(self.relay_subscribe(*share));
        }
        out.extend(self.announce()?);
        Ok(out)
    }

    /// Dial every related device that is known but not connected,
    /// backing off while it stays unreachable. Call every [`SUPERVISE`].
    ///
    /// Devices on the network are dialed there. The others are dialed
    /// through a relay: the end with the smaller key does it for trusted
    /// devices and fellow members, so both never dial at once; a joiner,
    /// a pairer and a device holding something to deliver always do.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::supervise"))]
    pub fn supervise(&mut self, now: Instant) -> Vec<Action> {
        let me = self.profile.id();
        for queue in self.outbox.values_mut() {
            queue.retain(|w| now.saturating_duration_since(w.since) < OUTBOX_TTL);
        }
        self.outbox.retain(|_, queue| !queue.is_empty());
        let mut wanted: Vec<(DeviceId, Action, bool)> = self
            .discovered
            .values()
            .filter(|p| !self.connected.contains_key(&p.device) && self.is_related(p.device))
            .map(|p| {
                let dial = Action::Connect {
                    addr: p.addr,
                    public: p.public,
                };
                // The smaller id dials; the larger joins in after a few
                // rounds in case the smaller one cannot see it.
                (p.device, dial, me < p.device)
            })
            .collect();
        wanted.extend(self.relay_dials());
        // Connected or no longer wanted: start from scratch next time.
        self.redials
            .retain(|device, _| wanted.iter().any(|(d, ..)| d == device));
        let mut out = Vec::new();
        for (device, dial, eager) in wanted {
            let redial = self
                .redials
                .entry(device)
                .or_insert_with(|| Redial::due(now));
            if now < redial.next {
                continue;
            }
            let dials = eager || redial.rounds >= LARGER_WAITS;
            redial.schedule(now);
            if dials {
                out.push(dial);
            }
        }
        out
    }

    /// The dials through a relay [`Self::supervise`] keeps trying, for
    /// devices not connected and not on the network.
    fn relay_dials(&self) -> Vec<(DeviceId, Action, bool)> {
        let mine = self.profile.key().public();
        let related = self
            .profile
            .trusted()
            .iter()
            .map(|t| (t.id, t.public))
            .chain(
                self.shares
                    .values()
                    .flat_map(|l| l.share.participants().map(|m| (m.device, m.public))),
            )
            .filter(|(_, public)| mine < public);
        let joins = self
            .joins
            .values()
            .map(|j| (j.invite.owner.device, j.invite.owner.public));
        let pairing = self.intent.as_ref().map(|i| (i.device, i.public));
        let waiting = self
            .outbox
            .keys()
            .filter_map(|device| Some((*device, *self.known_keys.get(device)?)));
        let mut dials: Vec<(DeviceId, Action, bool)> = Vec::new();
        for (device, public) in related.chain(joins).chain(pairing).chain(waiting) {
            if device == self.profile.id()
                || self.connected.contains_key(&device)
                || self.discovered.contains_key(&device)
                || dials.iter().any(|(d, ..)| *d == device)
            {
                continue;
            }
            if let Some(relay) = self.relay_for(device) {
                dials.push((device, Action::ConnectVia { relay, public }, true));
            }
        }
        dials
    }

    /// Dial every disconnected device at the next [`Self::supervise`],
    /// for a device coming back to the network.
    pub fn redial_now(&mut self, now: Instant) {
        for redial in self.redials.values_mut() {
            redial.next = now;
        }
    }

    /// Shares we are still waiting to be let into.
    pub fn pending_joins(&self) -> impl Iterator<Item = PendingJoin<'_>> {
        self.joins.values().map(|j| PendingJoin {
            invite: &j.invite,
            awaiting_approval: j.awaiting_approval,
        })
    }

    /// Shares this device owned and closed, newest first.
    pub fn ended(&self) -> impl Iterator<Item = &EndedShare> {
        let mut all: Vec<&EndedShare> = self.ended.values().collect();
        all.sort_by_key(|e| std::cmp::Reverse(e.ended_at));
        all.into_iter()
    }

    /// Shares this device walked away from, newest first.
    pub fn left(&self) -> impl Iterator<Item = &LeftShare> {
        let mut all: Vec<&LeftShare> = self.left.values().collect();
        all.sort_by_key(|l| std::cmp::Reverse(l.left_at));
        all.into_iter()
    }

    /// Ask back into a share this device walked away from, with the
    /// invitation it kept.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::rejoin"))]
    pub fn rejoin(&mut self, id: ShareId) -> Vec<Action> {
        let Some(left) = self.left.remove(&id) else {
            return Vec::new();
        };
        self.join_inner(left.invite, Some(left.joined_at))
    }

    /// Stop waiting on an invitation. Returns whether one was pending.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::cancel_join"))]
    pub fn cancel_join(&mut self, id: ShareId) -> bool {
        self.joins.remove(&id).is_some()
    }

    /// Open a share this device ended again: same title and documents,
    /// a new id and key, so the old invitation stays dead.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::reopen"))]
    pub fn reopen(&mut self, id: ShareId) -> Option<(ShareId, Vec<Action>)> {
        let ended = self.ended.remove(&id)?;
        let mut share = Share::new(self.me(), ended.title, ended.mode, now_ms());
        share.docs = ended.docs;
        share.blobs = ended.blobs;
        share.folder = ended.folder;
        share.desk = ended.desk;
        let fresh = share.id;
        Some((fresh, self.create_share(share)))
    }

    /// Drop a share from the recent lists.
    pub fn forget_recent(&mut self, id: ShareId) -> bool {
        self.ended.remove(&id).is_some() | self.left.remove(&id).is_some()
    }

    /// Drop what left or ended before `before` (Unix milliseconds).
    /// Returns whether anything went.
    pub fn prune_recent(&mut self, before: u64) -> bool {
        let ended = self.ended.len();
        let left = self.left.len();
        self.ended.retain(|_, e| e.ended_at >= before);
        self.left.retain(|_, l| l.left_at >= before);
        ended != self.ended.len() || left != self.left.len()
    }

    /// Change a share. The owner applies it on the spot; a member asks
    /// the owner, who applies it if the member's role allows and tells
    /// everyone. Nothing happens when this device may not.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::edit_share"))]
    pub fn edit_share(&mut self, id: ShareId, edit: ShareEdit) -> Vec<Action> {
        let me = self.profile.id();
        let Some(link) = self.shares.get(&id) else {
            return Vec::new();
        };
        if link.share.is_owner(me) {
            let mut out = Vec::new();
            self.apply_edit(me, id, edit, &mut out);
            return out;
        }
        if !edit_allowed(&link.share, me, &edit) {
            return Vec::new();
        }
        let owner = link.share.owner.device;
        let asked = bincode::encode_to_vec(&edit, bincode::config::standard()).unwrap_or_default();
        let sealed = ShareMessage::Edit(edit).seal(&link.share.key);
        self.hold(
            owner,
            id,
            Held::Edit(asked),
            Payload::Share { share: id, sealed },
        )
        .into_iter()
        .collect()
    }

    /// Owner side: apply `edit` asked by `from` when its role allows,
    /// and tell everyone. Returns whether the share changed.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::apply_edit"))]
    fn apply_edit(
        &mut self,
        from: DeviceId,
        id: ShareId,
        edit: ShareEdit,
        out: &mut Vec<Action>,
    ) -> bool {
        let me = self.profile.id();
        let Some(link) = self.shares.get_mut(&id) else {
            return false;
        };
        if !link.share.is_owner(me) || !edit_allowed(&link.share, from, &edit) {
            return false;
        }
        let share = &mut link.share;
        let mut told = Vec::new();
        let mut learned = None;
        let answered = match &edit {
            ShareEdit::Approve { device, .. } => Some((*device, true)),
            ShareEdit::Refuse(device) => Some((*device, false)),
            _ => None,
        };
        let changed = match edit {
            ShareEdit::Rename(title) => {
                let title = title.trim().to_owned();
                if title.is_empty() || title == share.title {
                    false
                } else {
                    share.title = title;
                    true
                }
            }
            ShareEdit::AddDocs { docs, blobs } => share.add_docs(&docs, &blobs),
            ShareEdit::RemoveDocs(docs) => share.remove_docs(&docs),
            ShareEdit::RemoveFolders(folders) => share.remove_folders(&folders),
            ShareEdit::SetRole { device, role } => share.set_role(device, role),
            ShareEdit::Expel(device) => {
                if share.expel(device) {
                    link.engine.forget_peer(device);
                    told.push((device, ShareMessage::Expelled));
                    true
                } else {
                    false
                }
            }
            ShareEdit::Approve { device, role } => match share.answer(device) {
                Some(waiting) => {
                    learned = Some((device, waiting.public));
                    share.admit(Member { role, ..waiting });
                    true
                }
                None => false,
            },
            ShareEdit::Refuse(device) => share.answer(device).is_some(),
            ShareEdit::SetEntryRole(role) => {
                let role = role.for_member();
                let changed = share.entry_role != role;
                share.entry_role = role;
                changed
            }
            ShareEdit::SetApproveEntry(on) => {
                let changed = share.approve_entry != on;
                share.approve_entry = on;
                changed
            }
            ShareEdit::SetEditorsAddDocs(on) => {
                let changed = share.editors_add_docs != on;
                share.editors_add_docs = on;
                changed
            }
        };
        if !changed {
            return false;
        }
        share.touch(now_ms());
        link.engine.set_scope(share.scope());
        let key = share.key.clone();
        if let Some((device, public)) = learned {
            self.known_keys.insert(device, public);
        }
        let someone_out = !told.is_empty();
        for (device, message) in told {
            self.forget_held(id, Some(device));
            let payload = Payload::Share {
                share: id,
                sealed: message.seal(&key),
            };
            out.extend(self.hold(device, id, Held::Removal, payload));
        }
        if someone_out {
            out.extend(self.rekey(id));
        }
        // The relay learns the new member before the welcome goes out,
        // so the member's first subscription is accepted.
        out.extend(self.relay_register(id));
        if let Some((device, admitted)) = answered {
            out.extend(self.answer_join(id, device, admitted));
        }
        out.extend(self.broadcast_meta(id));
        out.push(Action::Event(DriverEvent::ShareChanged(id)));
        true
    }

    /// Ask again into every share [`Self::new`] restored as pending:
    /// reach each invitation's relay, then the owner.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::resume_joins"))]
    pub fn resume_joins(&mut self) -> Vec<Action> {
        let invites: Vec<Invite> = self.joins.drain().map(|(_, j)| j.invite).collect();
        let mut out = Vec::new();
        for invite in invites {
            out.extend(self.join(invite));
        }
        out
    }
}

// ---------- Relay ---------------------------------------------------

/// Wire bytes of a relay message, as the transport carries them.
#[must_use]
pub fn relay_payload(message: &RelayMessage) -> Vec<u8> {
    Payload::Relay(message.to_bytes()).to_bytes()
}

/// The device behind a relay address in text form.
fn relay_device(server: &str) -> Option<DeviceId> {
    server.parse::<RelayAddr>().ok().map(|addr| addr.device())
}

/// Reverse of [`relay_payload`]; `None` for any other payload.
#[must_use]
pub fn relay_message(bytes: &[u8]) -> Option<RelayMessage> {
    match Payload::from_bytes(bytes)? {
        Payload::Relay(inner) => RelayMessage::from_bytes(&inner),
        Payload::Sync(_)
        | Payload::Pair(_)
        | Payload::Share { .. }
        | Payload::Join { .. }
        | Payload::JoinReply { .. }
        | Payload::Keys { .. }
        | Payload::Lease { .. }
        | Payload::Live { .. } => None,
    }
}

impl<R: Replica> SyncDriver<R> {
    /// Use (or drop) a relay. Connecting happens on the next tick or
    /// right away when the relay is already known. Shares this device
    /// owns move there with it.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::set_relay"))]
    pub fn set_relay(&mut self, addr: Option<RelayAddr>) -> Vec<Action> {
        self.own_relay = addr.as_ref().map(RelayAddr::device);
        let server = addr.as_ref().map(ToString::to_string);
        if let Some(addr) = addr {
            self.add_relay(addr);
        }
        let me = self.profile.id();
        let now = now_ms();
        for link in self.shares.values_mut() {
            if link.share.is_owner(me) && link.share.server != server {
                link.share.server.clone_from(&server);
                // Another relay numbers its mailbox from scratch.
                link.share.relay_cursor = 0;
                link.share.touch(now);
            }
        }
        self.sync_relays()
    }

    #[must_use]
    pub fn relay(&self) -> Option<&RelayAddr> {
        self.relays.get(&self.own_relay?).map(|r| &r.addr)
    }

    #[must_use]
    pub fn relay_online(&self) -> bool {
        self.own_relay
            .and_then(|device| self.relays.get(&device))
            .is_some_and(|r| r.connected)
    }

    /// The invitation for a share we own, pointing at its relay.
    #[must_use]
    pub fn invite_for(&self, id: ShareId) -> Option<Invite> {
        let share = self.share(id)?;
        let mut invite = share.invite();
        if invite.server.is_none() {
            invite.server = self.relay().map(ToString::to_string);
        }
        Some(invite)
    }

    fn add_relay(&mut self, addr: RelayAddr) {
        self.relays
            .entry(addr.device())
            .or_insert_with(|| RelayLink {
                device: addr.device(),
                addr,
                connected: false,
                published: HashMap::new(),
                epochs: HashMap::new(),
            });
    }

    /// Every relay something here meets others at: the profile's own,
    /// those of the shares, of pending invitations and of the device we
    /// are pairing with, and those of shares left with something still
    /// to say there.
    fn wanted_relays(&self) -> Vec<RelayAddr> {
        let shares = self
            .shares
            .values()
            .filter_map(|l| l.share.server.as_deref());
        let joins = self
            .joins
            .values()
            .filter_map(|j| j.invite.server.as_deref());
        let left = self
            .left
            .values()
            .filter(|l| self.outbox.contains_key(&l.invite.owner.device))
            .filter_map(|l| l.invite.server.as_deref());
        let mut wanted: Vec<RelayAddr> = shares
            .chain(joins)
            .chain(left)
            .filter_map(|s| s.parse().ok())
            .chain(self.intent.as_ref().and_then(|i| i.relay.clone()))
            // Others name these: never one of the devices we talk to.
            .filter(|addr: &RelayAddr| !self.is_peer(addr.device()))
            .chain(self.relay().cloned())
            .collect();
        wanted.dedup();
        wanted
    }

    /// Whether `device` is one we know as a device rather than a relay:
    /// this one, a trusted one, someone in a share, an owner we wait on,
    /// the one we pair with, or one with a channel open as a device.
    fn is_peer(&self, device: DeviceId) -> bool {
        device == self.profile.id()
            || self.profile.is_trusted(device)
            || self
                .shares
                .values()
                .any(|l| l.share.participants().any(|m| m.device == device))
            || self.joins.values().any(|j| j.invite.owner.device == device)
            || self.intent.as_ref().is_some_and(|i| i.device == device)
            || (self.connected.contains_key(&device) && !self.relays.contains_key(&device))
    }

    /// Keep a link to every wanted relay and close the others, unless a
    /// channel to some device still runs through one.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::sync_relays"))]
    fn sync_relays(&mut self) -> Vec<Action> {
        let wanted = self.wanted_relays();
        for addr in &wanted {
            self.retired.remove(&addr.device());
            self.add_relay(addr.clone());
        }
        let dropped: Vec<DeviceId> = self
            .relays
            .keys()
            .copied()
            .filter(|device| !wanted.iter().any(|a| a.device() == *device))
            .filter(|device| !self.carries(*device))
            .collect();
        let mut out = Vec::new();
        for device in dropped {
            let reached = self.relays.remove(&device).is_some_and(|r| r.connected);
            // Only what we reached as a relay is turned away later.
            if reached && self.connected.remove(&device).is_some() {
                self.retired.insert(device);
                out.push(Action::Disconnect { device });
            }
        }
        out.extend(self.relay_reconnect());
        out
    }

    /// Whether a channel to some device runs through `relay` right now.
    fn carries(&self, relay: DeviceId) -> bool {
        let Some(at) = self.connected.get(&relay).map(|p| p.addr) else {
            return false;
        };
        self.connected
            .values()
            .any(|p| p.relayed && p.addr == at && p.device != relay)
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::relay_reconnect"))]
    fn relay_reconnect(&self) -> Vec<Action> {
        self.relays
            .values()
            .filter(|r| !r.connected && !self.connected.contains_key(&r.device))
            .map(|r| Action::ConnectHost {
                host: r.addr.host.clone(),
                public: r.addr.public,
            })
            .collect()
    }

    /// The relay a share meets at: the one it names, once linked, else
    /// ours. Never ours in place of one it names.
    fn share_relay(&self, id: ShareId) -> Option<DeviceId> {
        self.share(id)
            .and_then(|s| s.server.as_deref())
            .map_or(self.own_relay, |server| {
                relay_device(server).filter(|device| self.relays.contains_key(device))
            })
    }

    /// A connected relay `device` can be reached at: ours for a trusted
    /// device, a share's for its members, an invitation's for its owner,
    /// the pairing link's for the device we pair with.
    fn relay_for(&self, device: DeviceId) -> Option<DeviceId> {
        let shares = self
            .shares
            .values()
            .filter(|l| l.share.participants().any(|m| m.device == device))
            .filter_map(|l| self.share_relay(l.share.id));
        let joins = self
            .joins
            .values()
            .filter(|j| j.invite.owner.device == device)
            .filter_map(|j| j.invite.server.as_deref().and_then(relay_device));
        let left = self
            .left
            .values()
            .filter(|l| l.invite.owner.device == device)
            .filter_map(|l| l.invite.server.as_deref().and_then(relay_device));
        let pairing = self
            .intent
            .as_ref()
            .filter(|i| i.device == device)
            .and_then(|i| i.relay.as_ref().map(RelayAddr::device));
        let own = self.own_relay;
        shares
            .chain(joins)
            .chain(left)
            .chain(pairing)
            .chain(own)
            .find(|relay| self.relays.get(relay).is_some_and(|r| r.connected))
    }

    fn relay_send(&self, relay: DeviceId, message: &RelayMessage) -> Option<Action> {
        let link = self.relays.get(&relay)?;
        link.connected.then(|| Action::Send {
            device: relay,
            lane: Lane::Data,
            bytes: relay_payload(message),
        })
    }

    /// Tell `relay` which devices may reach us there: the trusted ones,
    /// and whoever holds the open pairing link.
    fn consent_at(&self, relay: DeviceId) -> Vec<Action> {
        let keys = self.profile.trusted().iter().map(|t| t.public).collect();
        let token = self
            .offer
            .filter(|o| !o.expired())
            .map(|o| KnockToken::pairing(&o.secret));
        self.relay_send(relay, &RelayMessage::Trust { keys })
            .into_iter()
            .chain(self.relay_send(relay, &RelayMessage::Expect { token }))
            .collect()
    }

    /// Relays that hear whom we trust and what we expect: ours only.
    /// One a share, an invitation or a pairing link chose could map our
    /// devices; pairing through it needs no more than our knock.
    fn consent_relays(&self) -> Vec<DeviceId> {
        self.own_relay.into_iter().collect()
    }

    /// [`Self::consent_at`] every relay of [`Self::consent_relays`].
    fn relay_consent(&self) -> Vec<Action> {
        self.consent_relays()
            .into_iter()
            .flat_map(|relay| self.consent_at(relay))
            .collect()
    }

    /// Knock on the device we are pairing with, when its link carried
    /// the secret for it.
    fn pair_knock(&self) -> Option<Action> {
        let intent = self.intent.as_ref()?;
        let secret = intent.secret?;
        let relay = intent
            .relay
            .as_ref()
            .map(RelayAddr::device)
            .or(self.own_relay)?;
        self.relay_send(
            relay,
            &RelayMessage::Knock {
                to: intent.public,
                token: KnockToken::pairing(&secret),
            },
        )
    }

    /// Owner tells the share's relay who may use its mailbox and reach
    /// its members.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::relay_register"))]
    fn relay_register(&self, id: ShareId) -> Vec<Action> {
        let Some(share) = self.share(id) else {
            return Vec::new();
        };
        if !share.is_owner(self.profile.id()) {
            return Vec::new();
        }
        let Some(relay) = self.share_relay(id) else {
            return Vec::new();
        };
        self.relay_send(
            relay,
            &RelayMessage::Register {
                share: id,
                registration: Registration {
                    members: share.members.iter().map(|m| m.public).collect(),
                    retain: share.mode == ShareMode::Permanent,
                    knock: KnockToken::share(&share.key, id),
                    epoch: share.epoch,
                },
            },
        )
        .into_iter()
        .collect()
    }

    fn relay_subscribe(&self, id: ShareId) -> Option<Action> {
        let share = self.share(id)?;
        self.relay_send(
            self.share_relay(id)?,
            &RelayMessage::Subscribe {
                share: id,
                after: share.relay_cursor,
            },
        )
    }

    /// Whether this device may leave records for `id` at `relay`: the
    /// owner always, a member under the owner's lease once the relay
    /// answered its subscription, while it holds the key the relay has
    /// the share at.
    fn mailbox_ready(&self, id: ShareId, relay: DeviceId) -> bool {
        let me = self.profile.id();
        let Some(share) = self.share(id) else {
            return false;
        };
        // The owner's lease is what says the key is still the current one;
        // the relay's epoch is only a hint, a relay may lie about it.
        share.is_owner(me)
            || (self.lease_holds(id)
                && self
                    .relays
                    .get(&relay)
                    .and_then(|r| r.epochs.get(&id))
                    .is_some_and(|epoch| *epoch <= share.epoch))
    }

    /// Just connected to `relay`: register what we own there, subscribe
    /// to what meets there, knock for whoever we wait on there, hand in
    /// what it has not seen.
    #[cfg_attr(feature = "sismo", sismo::function("sync::driver::relay_hello"))]
    fn relay_hello(&mut self, relay: DeviceId) -> Result<Vec<Action>, SyncError<R::Error>> {
        let mut out = Vec::new();
        if self.own_relay == Some(relay) {
            out.push(Action::Event(DriverEvent::RelayOnline(true)));
        }
        if self.consent_relays().contains(&relay) {
            out.extend(self.consent_at(relay));
        }
        let ids: Vec<ShareId> = self
            .shares
            .keys()
            .copied()
            .filter(|id| self.share_relay(*id) == Some(relay))
            .collect();
        for id in ids {
            out.extend(self.relay_register(id));
            out.extend(self.relay_subscribe(id));
            if self.renew_lease(id) {
                out.extend(self.spread_lease(id));
            } else {
                out.extend(self.lease_record(id));
            }
        }
        let pending: Vec<ShareId> = self.joins.keys().copied().collect();
        for share in pending {
            out.extend(self.join_request(share));
        }
        out.extend(self.pair_knock());
        out.extend(self.relay_publish()?);
        Ok(out)
    }

    /// Hand each share's relay whatever changed since the last time.
    /// Sessions ride live channels only: a mailbox would only mark their
    /// content delivered when it was not.
    #[cfg_attr(feature = "sismo", sismo::function("sync::relay::publish"))]
    fn relay_publish(&mut self) -> Result<Vec<Action>, SyncError<R::Error>> {
        let mut out = Vec::new();
        let me = self.profile.id();
        let ids: Vec<ShareId> = self.shares.keys().copied().collect();
        for id in ids {
            let Some(link) = self.shares.get(&id) else {
                continue;
            };
            if link.share.mode == ShareMode::Session {
                continue;
            }
            let share = link.share.clone();
            // Never ahead of what the relay told us, or with a key it is
            // past: what goes there must not open with a key someone thrown
            // out still holds.
            if !self
                .share_relay(id)
                .is_some_and(|relay| self.mailbox_ready(id, relay))
            {
                continue;
            }
            let Some(relay) = self
                .share_relay(id)
                .and_then(|device| self.relays.get_mut(&device))
                .filter(|r| r.connected)
            else {
                continue;
            };
            let published = relay.published.entry(id).or_default();
            let mut records = if share.can_edit(me) {
                published.doc_records(&self.replica, &share.docs)?
            } else {
                Vec::new()
            };
            records.extend(published.blob_records(&self.replica, &share, me));
            #[cfg(feature = "sismo")]
            sismo::counter!("sync.relay.records_out", records.len());
            for (kind, message) in records {
                let sealed = ShareMessage::Sync(message.to_bytes()?).seal_signed(
                    &share,
                    kind,
                    self.profile.key(),
                );
                out.push(Action::Send {
                    device: relay.device,
                    lane: Lane::Data,
                    bytes: relay_payload(&RelayMessage::Publish {
                        share: id,
                        kind,
                        sealed,
                        epoch: share.epoch,
                    }),
                });
            }
            if share.is_owner(me) && published.meta_at < share.updated_at {
                published.meta_at = share.updated_at;
                out.push(Action::Send {
                    device: relay.device,
                    lane: Lane::Data,
                    bytes: relay_payload(&RelayMessage::Publish {
                        share: id,
                        kind: RecordKind::Meta,
                        sealed: ShareMessage::Meta(Box::new(share.for_member())).seal_signed(
                            &share,
                            RecordKind::Meta,
                            self.profile.key(),
                        ),
                        epoch: share.epoch,
                    }),
                });
            }
        }
        Ok(out)
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::relay::on_message"))]
    fn on_relay(
        &mut self,
        from: DeviceId,
        bytes: &[u8],
        out: &mut Vec<Action>,
    ) -> Result<(), SyncError<R::Error>> {
        if !self.relays.contains_key(&from) {
            return Ok(());
        }
        let Some(message) = RelayMessage::from_bytes(bytes) else {
            return Ok(());
        };
        match message {
            RelayMessage::Records { share, .. }
            | RelayMessage::Gone { share }
            | RelayMessage::Refused { share }
            | RelayMessage::Registered { share }
            | RelayMessage::Full { share }
            | RelayMessage::Behind { share, .. }
                if self.share_relay(share) != Some(from) =>
            {
                // What a relay says about a share counts only from the relay
                // the share meets at.
            }
            RelayMessage::Records {
                share,
                records,
                epoch,
            } => {
                #[cfg(feature = "sismo")]
                sismo::counter!("sync.relay.records_in", records.len());
                // Keys among these come first, so what this device holds is
                // compared with the relay's once they are in.
                if self.take_records(share, records, out)? {
                    out.push(Action::Event(DriverEvent::ShareChanged(share)));
                }
                if let Some(relay) = self.relays.get_mut(&from) {
                    relay.epochs.insert(share, epoch);
                }
                out.extend(self.relay_publish()?);
            }
            RelayMessage::Behind { share, epoch } => {
                // Nothing of what went out was taken: all of it goes again
                // once the key is in.
                if let Some(relay) = self.relays.get_mut(&from) {
                    relay.epochs.insert(share, epoch);
                    relay.published.remove(&share);
                }
                // The owner hands the new key over once we meet.
                if let Some(owner) = self.share(share).map(|s| s.owner.device) {
                    out.extend(self.ask_owner(share, owner)?);
                }
            }
            RelayMessage::Gone { share } => {
                let me = self.profile.id();
                match self.share(share).map(|s| (s.is_owner(me), s.owner.device)) {
                    Some((true, _)) => out.extend(self.relay_register(share)),
                    // A relay that lost its mailboxes says the same as one
                    // whose owner closed the share: only the owner can tell
                    // which, so ask it.
                    Some((false, owner)) => out.extend(self.ask_owner(share, owner)?),
                    None => {}
                }
            }
            RelayMessage::Refused { share } => {
                // Turned away from a mailbox: maybe thrown out while away,
                // maybe the owner has not registered us yet. Its answer says,
                // and what was handed in goes again once let in.
                if let Some(relay) = self.relays.get_mut(&from) {
                    relay.published.remove(&share);
                }
                let me = self.profile.id();
                if let Some(owner) = self
                    .share(share)
                    .filter(|s| !s.is_owner(me))
                    .map(|s| s.owner.device)
                {
                    out.extend(self.ask_owner(share, owner)?);
                }
            }
            RelayMessage::Registered { share } => {
                out.extend(self.relay_subscribe(share));
            }
            RelayMessage::Knocked { to } => {
                // The knock is in place: dial whoever we knocked on.
                let device = DeviceId::of_public_key(&to);
                let waited = self.joins.values().any(|j| j.invite.owner.public == to)
                    || self.intent.as_ref().is_some_and(|i| i.public == to);
                if waited && !self.connected.contains_key(&device) {
                    out.push(Action::ConnectVia {
                        relay: from,
                        public: to,
                    });
                }
            }
            RelayMessage::Full { share } => {
                tracing::warn!(target: "notin-sync", "relay keeps no more for share {share:?}");
            }
            RelayMessage::Register { .. }
            | RelayMessage::Unregister { .. }
            | RelayMessage::Trust { .. }
            | RelayMessage::Expect { .. }
            | RelayMessage::Knock { .. }
            | RelayMessage::Subscribe { .. }
            | RelayMessage::Publish { .. } => {}
        }
        Ok(())
    }

    /// Take in mailbox records for `id`, with those parked before.
    /// Returns whether the share changed.
    ///
    /// A view of the share says who may write, so views go first. A
    /// record by someone not listed yet waits for the next view, and the
    /// cursor stays before it, so a restart fetches it again.
    #[cfg_attr(feature = "sismo", sismo::function("sync::relay::take_records"))]
    fn take_records(
        &mut self,
        id: ShareId,
        records: Vec<crate::relay::Record>,
        out: &mut Vec<Action>,
    ) -> Result<bool, SyncError<R::Error>> {
        let Some((start, server)) = self.share(id).map(|s| (s.relay_cursor, s.server.clone()))
        else {
            return Ok(false);
        };
        let now = now_ms();
        // Since when each record has waited, kept across restarts with the
        // cursor, so a backlog fetched again is given up on in time.
        let waited: HashMap<u64, u64> = self
            .share(id)
            .map(|s| s.held_back.iter().copied().collect())
            .unwrap_or_default();
        let held = self.parked.remove(&id).unwrap_or_default();
        let mut batch = held.records;
        for record in records {
            if record.seq > start && !batch.iter().any(|(r, _)| r.seq == record.seq) {
                let since = waited.get(&record.seq).copied().unwrap_or(now);
                batch.push((record, since));
            }
        }
        // Keys open the rest; a view says who may write.
        batch.sort_by_key(|(r, _)| {
            let rank = match r.kind {
                RecordKind::Keys { .. } => 0,
                RecordKind::Meta => 1,
                _ => 2,
            };
            (rank, r.seq)
        });
        let mut touched = false;
        // What counted while the cursor waited on parked records.
        let mut top = start.max(held.top);
        let mut parked = Vec::new();
        for (record, since) in batch {
            match self.apply_record(id, &record, out)? {
                Taken::Applied(changed) => {
                    touched |= changed;
                    top = top.max(record.seq);
                }
                Taken::Parked if now.saturating_sub(since) < PARK_FOR_MS => {
                    parked.push((record, since));
                }
                Taken::Parked | Taken::Dropped => {}
            }
        }
        let Some(link) = self.shares.get_mut(&id) else {
            return Ok(false);
        };
        // A share that moved is read from the start there. The cursor never
        // passes a waiting record, kept or not: one let go for room is
        // fetched again.
        if link.share.server == server {
            let before = parked.iter().map(|(r, _)| r.seq - 1).min();
            link.share.relay_cursor = before.map_or(top, |b| top.min(b)).max(start);
            touched |= link.share.relay_cursor != start;
        }
        let mut held_back: Vec<(u64, u64)> =
            parked.iter().map(|(r, since)| (r.seq, *since)).collect();
        held_back.sort_unstable();
        held_back.truncate(PARKED_MAX);
        touched |= held_back != link.share.held_back;
        link.share.held_back = held_back;
        if !parked.is_empty() {
            // The oldest are the ones holding the cursor back: keep them.
            parked.sort_by_key(|(r, _)| r.seq);
            let mut bytes = 0;
            parked.retain(|(r, _)| {
                bytes += r.sealed.len();
                bytes <= PARKED_BYTES
            });
            parked.truncate(PARKED_MAX);
            self.parked.insert(
                id,
                Parked {
                    records: parked,
                    top,
                },
            );
        }
        Ok(touched)
    }

    /// Apply one mailbox record.
    #[cfg_attr(feature = "sismo", sismo::function("sync::relay::apply_record"))]
    fn apply_record(
        &mut self,
        id: ShareId,
        record: &crate::relay::Record,
        out: &mut Vec<Action>,
    ) -> Result<Taken, SyncError<R::Error>> {
        let me = self.profile.id();
        let Some(link) = self.shares.get_mut(&id) else {
            return Ok(Taken::Dropped);
        };
        if let RecordKind::Keys { to } = record.kind {
            if to != *self.profile.key().public() {
                return Ok(Taken::Applied(false));
            }
            let owner = link.share.owner.device;
            return Ok(
                if record.from == owner
                    && open_keys(
                        self.profile.key(),
                        &link.share.owner.public,
                        id,
                        &record.sealed,
                    )
                    .is_some()
                {
                    Taken::Applied(self.take_keys(id, record.from, &record.sealed))
                } else {
                    Taken::Dropped
                },
            );
        }
        // Only what a member signed counts, and only as that member: the
        // relay names the sender but anyone holding the key can seal.
        // The owner's roster and keys are the share's own: what does not
        // open there never will. A member waits only for a newer view, or
        // for a key newer than its own that the owner has yet to hand over.
        let owner = link.share.is_owner(me);
        let (author, message) =
            match ShareMessage::open_signed(&link.share, &record.sealed, record.kind) {
                Ok(opened) => opened,
                Err(Unopened::Stranger) if !owner => return Ok(Taken::Parked),
                Err(Unopened::NoKey)
                    if !owner
                        && record.epoch > link.share.epoch
                        && record.epoch <= link.share.epoch.saturating_add(1) =>
                {
                    return Ok(Taken::Parked);
                }
                Err(Unopened::Stranger | Unopened::NoKey | Unopened::Invalid) => {
                    return Ok(Taken::Dropped);
                }
            };
        if author != record.from {
            return Ok(Taken::Dropped);
        }
        if author == me {
            return Ok(Taken::Applied(false));
        }
        match (message, record.kind) {
            (
                ShareMessage::Sync(bytes),
                RecordKind::Update { .. } | RecordKind::Snapshot { .. } | RecordKind::Blob { .. },
            ) => {
                let message = Message::from_bytes(&bytes)?;
                if let Some(message) = permitted(&link.share, author, message) {
                    let handled = link.engine.handle(author, message)?;
                    if !handled.imported.is_empty() {
                        out.push(Action::Event(DriverEvent::Imported(handled.imported)));
                    }
                    if !handled.merged.is_empty() {
                        out.push(Action::Event(DriverEvent::Merged(handled.merged)));
                    }
                }
            }
            (ShareMessage::Meta(share), RecordKind::Meta) => {
                let Some(fresh) = fresh_meta(&link.share, author, me, *share) else {
                    return Ok(Taken::Applied(false));
                };
                let moved = fresh.server != link.share.server;
                link.share = fresh;
                link.engine.set_scope(link.share.scope());
                if !link.share.is_member(me) {
                    self.shares.remove(&id);
                    self.parked.remove(&id);
                    out.push(Action::Event(DriverEvent::ShareRemoved {
                        share: id,
                        reason: Removal::Expelled,
                    }));
                    return Ok(Taken::Dropped);
                }
                if moved {
                    out.extend(self.sync_relays());
                    out.extend(self.relay_subscribe(id));
                }
                return Ok(Taken::Applied(true));
            }
            (ShareMessage::Lease(signed), RecordKind::Lease) if link.share.is_owner(author) => {
                self.take_lease(id, *signed, out)?;
                return Ok(Taken::Applied(false));
            }
            // Only for itself: the signature says who walked away.
            (ShareMessage::Left { joined_at }, RecordKind::Left)
                if link.share.is_owner(me) && link.share.depart(author, joined_at) =>
            {
                link.engine.forget_peer(author);
                out.extend(self.rekey(id));
                return Ok(Taken::Applied(true));
            }
            // Members only ever leave exchange records, their departure and
            // the owner's view there; anything else came from somewhere it
            // should not.
            _ => {}
        }
        Ok(Taken::Applied(false))
    }
}

/// Mailbox records of one share waiting for a newer view of it.
#[derive(Debug, Default)]
struct Parked {
    /// Each with since when it waits, in Unix milliseconds.
    records: Vec<(crate::relay::Record, u64)>,
    /// The last record that counted, past the cursor held back for these.
    top: u64,
}

/// What became of one mailbox record.
enum Taken {
    /// It counted; `true` when the share itself changed.
    Applied(bool),
    /// Its author is not listed yet; it waits for a newer view.
    Parked,
    /// It does not count and never will.
    Dropped,
}

/// The owner's view `incoming` as this device adopts it, or `None` when
/// it does not come from the owner or is not newer than what is held. The
/// keys and the owner stay as held here (keys come only sealed for this
/// device), and a share that
/// moved to another relay is read there from the start.
fn fresh_meta(current: &Share, from: DeviceId, me: DeviceId, incoming: Share) -> Option<Share> {
    if !current.is_owner(from)
        || current.is_owner(me)
        || incoming.id != current.id
        || incoming.updated_at <= current.updated_at
    {
        return None;
    }
    let (relay_cursor, held_back) = if incoming.server == current.server {
        (current.relay_cursor, current.held_back.clone())
    } else {
        (0, Vec::new())
    };
    // A newer lease of the owner's comes along; any other is ignored.
    let mut held = current.clone();
    if let Some(signed) = incoming.lease.clone() {
        held.offer_lease(signed, now_ms().max(current.clock_mark));
    }
    let lease = held.lease;
    Some(Share {
        key: current.key.clone(),
        epoch: current.epoch,
        previous: current.previous.clone(),
        lease,
        clock_mark: current.clock_mark,
        owner: current.owner.clone(),
        relay_cursor,
        held_back,
        ..incoming
    })
}

/// Whether `from`'s role lets it ask for `edit`.
#[cfg_attr(feature = "sismo", sismo::function("sync::driver::edit_allowed"))]
fn edit_allowed(share: &Share, from: DeviceId, edit: &ShareEdit) -> bool {
    if share.is_owner(from) {
        return true;
    }
    let Some(role) = share.role_of(from) else {
        return false;
    };
    match edit {
        ShareEdit::Rename(_) => share.can_rename(from),
        ShareEdit::AddDocs { .. } | ShareEdit::RemoveDocs(_) | ShareEdit::RemoveFolders(_) => {
            share.can_add_docs(from)
        }
        ShareEdit::SetRole { device, role: to } => share.outranks(from, *device) && *to < role,
        ShareEdit::Expel(device) => share.outranks(from, *device),
        ShareEdit::Approve { role: to, .. } => role.can_manage() && *to < role,
        ShareEdit::Refuse(_) => role.can_manage(),
        ShareEdit::SetEntryRole(_)
        | ShareEdit::SetApproveEntry(_)
        | ShareEdit::SetEditorsAddDocs(_) => false,
    }
}

/// Drop what `from` may not do inside `share`.
#[cfg_attr(feature = "sismo", sismo::function("sync::driver::permitted"))]
fn permitted(share: &Share, from: DeviceId, message: Message) -> Option<Message> {
    match message {
        Message::Inventory { .. } => Some(message),
        Message::DocUpdates { .. } | Message::DocSnapshot { .. } => {
            share.can_edit(from).then_some(message)
        }
        Message::Blobs(blobs) => {
            let kept: Vec<_> = blobs
                .into_iter()
                .filter(|blob| match blob.head.kind {
                    BlobKind::Item | BlobKind::Folder if blob.head.gone => share.can_delete(from),
                    BlobKind::Item
                    | BlobKind::Snapshot
                    | BlobKind::Asset
                    | BlobKind::Attachment => share.can_edit(from),
                    // Folders only travel inside a shared folder.
                    BlobKind::Folder => share.folder.is_some() && share.can_edit(from),
                    BlobKind::Link => share.can_link(from),
                    BlobKind::Tag
                    | BlobKind::Workspace
                    | BlobKind::ItemTags
                    | BlobKind::FolderTags
                    | BlobKind::Pins => false,
                })
                .collect();
            (!kept.is_empty()).then_some(Message::Blobs(kept))
        }
    }
}

#[cfg_attr(feature = "sismo", sismo::function("sync::driver::seal_answer"))]
fn seal_answer(key: &ShareKey, answer: &JoinAnswer) -> Vec<u8> {
    let plain = bincode::encode_to_vec(answer, bincode::config::standard()).unwrap_or_default();
    key.seal(&plain).unwrap_or_default()
}

fn send(device: DeviceId, payload: &Payload) -> Action {
    let bytes = payload.to_bytes();
    #[cfg(feature = "sismo")]
    sismo::counter!("sync.out.data_bytes", bytes.len());
    Action::Send {
        device,
        lane: Lane::Data,
        bytes,
    }
}

/// [`send`] on the live lane: small, urgent, soon stale.
fn send_live(device: DeviceId, payload: &Payload) -> Action {
    let bytes = payload.to_bytes();
    #[cfg(feature = "sismo")]
    sismo::counter!("sync.out.live_bytes", bytes.len());
    Action::Send {
        device,
        lane: Lane::Live,
        bytes,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};

    use super::*;
    use crate::DocStatus;
    use crate::replica::BlobKind;
    use crate::share::ShareMode;
    use crate::testing::MemReplica;

    const D1: DocId = DocId([1; 16]);
    const D2: DocId = DocId([2; 16]);

    fn driver(name: &str) -> SyncDriver<MemReplica> {
        SyncDriver::new(
            DeviceProfile::generate(name),
            MemReplica::default(),
            ShareBook::default(),
        )
    }

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, port))
    }

    fn info(driver: &SyncDriver<MemReplica>, port: u16) -> PeerInfo {
        PeerInfo {
            device: driver.profile.id(),
            public: *driver.profile.key().public(),
            name: driver.profile.name().to_owned(),
            addr: addr(port),
            relayed: false,
            through_relay: false,
            session: 7,
        }
    }

    fn marks(driver: &SyncDriver<MemReplica>, doc: DocId) -> Vec<(u64, u64)> {
        driver.with_replica(|r| r.marks(doc))
    }

    fn mark(driver: &SyncDriver<MemReplica>, doc: DocId, author: u64, counter: u64) {
        driver.with_replica(|r| r.mark(doc, author, counter));
    }

    /// Deliver every `Send` in `actions` from `from` to `to`, looping
    /// until nothing is left to carry. Returns the events seen.
    fn deliver(
        actions: Vec<Action>,
        from: &mut SyncDriver<MemReplica>,
        to: &mut SyncDriver<MemReplica>,
    ) -> Vec<DriverEvent> {
        let mut events = Vec::new();
        let mut queue: std::collections::VecDeque<(bool, Action)> =
            actions.into_iter().map(|a| (true, a)).collect();
        while let Some((forward, action)) = queue.pop_front() {
            match action {
                Action::Send { bytes, .. } => {
                    let (sender, receiver) = if forward {
                        (&*from, &mut *to)
                    } else {
                        (&*to, &mut *from)
                    };
                    let event = NetEvent::Message {
                        from: sender.profile.id(),
                        bytes,
                    };
                    let replies = receiver.on_event(event).unwrap();
                    queue.extend(replies.into_iter().map(|a| (!forward, a)));
                }
                Action::Connect { .. }
                | Action::ConnectHost { .. }
                | Action::ConnectVia { .. }
                | Action::Punch { .. }
                | Action::Disconnect { .. }
                | Action::Bulk { .. } => {}
                Action::Event(event) => events.push(event),
            }
        }
        events
    }

    fn connect_both(a: &mut SyncDriver<MemReplica>, b: &mut SyncDriver<MemReplica>) -> Vec<Action> {
        let mut out = a.on_event(NetEvent::Connected(info(b, 2))).unwrap();
        let from_b = b.on_event(NetEvent::Connected(info(a, 1))).unwrap();
        out.extend(deliver(from_b, b, a).into_iter().map(Action::Event));
        out
    }

    #[test]
    fn pairing_over_a_connected_channel_trusts_both_sides() {
        let mut a = driver("Tablet");
        let mut b = driver("PC");
        let (offer, _) = a.begin_pairing();
        let parsed = PairingOffer::parse_link(&offer.to_link()).unwrap();
        assert_eq!(parsed, offer);

        assert!(b.pair_with(&parsed).is_empty());
        let actions = b.on_event(NetEvent::Discovered(info(&a, 1))).unwrap();
        assert!(matches!(actions[..], [Action::Connect { .. }]));

        assert!(
            a.on_event(NetEvent::Connected(info(&b, 2)))
                .unwrap()
                .is_empty()
        );
        let request = b.on_event(NetEvent::Connected(info(&a, 1))).unwrap();
        // The request, and the device it pairs with may now send it big ones.
        assert!(matches!(
            request[..],
            [Action::Send { .. }, Action::Bulk { allowed: true, .. }]
        ));

        let events = deliver(request, &mut b, &mut a);
        assert!(a.profile.is_trusted(b.profile.id()));
        assert!(b.profile.is_trusted(a.profile.id()));
        assert_eq!(a.profile.trusted()[0].name, "PC");
        assert_eq!(b.profile.trusted()[0].name, "Tablet");
        assert!(events.iter().any(|e| matches!(e, DriverEvent::Paired(_))));
        assert!(b.pairing_target().is_none());
    }

    fn pair_request_from(
        b: &SyncDriver<MemReplica>,
        code: u32,
        proof: Option<[u8; 32]>,
    ) -> NetEvent {
        NetEvent::Message {
            from: b.profile.id(),
            bytes: Payload::Pair(PairMessage::Request {
                code,
                name: "PC".into(),
                proof,
            })
            .to_bytes(),
        }
    }

    #[test]
    fn through_a_relay_only_the_link_pairs_and_guesses_keep_the_offer() {
        let mut a = driver("Tablet");
        let b = driver("PC");
        let (offer, _) = a.begin_pairing();
        let secret = offer.secret.unwrap();
        let code = offer.code.value();
        a.on_event(NetEvent::Connected(PeerInfo {
            relayed: true,
            through_relay: true,
            ..info(&b, 2)
        }))
        .unwrap();
        // Punched through since: still a channel the relay opened.
        a.on_event(NetEvent::PathChanged {
            device: b.profile.id(),
            relayed: false,
        })
        .unwrap();
        let b_public = *b.profile.key().public();
        // The right code alone, a proof made for another key, a proof
        // with the wrong code: none of them pairs, and the offer stays.
        for (code, proof) in [
            (code, None),
            (code, Some(secret.proof(&[7; 32]))),
            ((code + 1) % 1_000_000, Some(secret.proof(&b_public))),
        ] {
            let out = a.on_event(pair_request_from(&b, code, proof)).unwrap();
            assert!(!a.profile.is_trusted(b.profile.id()));
            assert!(
                out.iter()
                    .all(|x| !matches!(x, Action::Event(DriverEvent::Paired(_))))
            );
            assert!(
                a.current_offer().is_some(),
                "a guess from afar burnt the offer"
            );
        }
        a.on_event(pair_request_from(&b, code, Some(secret.proof(&b_public))))
            .unwrap();
        assert!(a.profile.is_trusted(b.profile.id()));
        assert!(a.current_offer().is_none());
    }

    #[test]
    fn on_the_network_a_wrong_proof_still_fails() {
        let mut a = driver("Tablet");
        let b = driver("PC");
        let (offer, _) = a.begin_pairing();
        a.on_event(NetEvent::Connected(info(&b, 2))).unwrap();
        let wrong = offer.secret.unwrap().proof(&[7; 32]);
        a.on_event(pair_request_from(&b, offer.code.value(), Some(wrong)))
            .unwrap();
        assert!(!a.profile.is_trusted(b.profile.id()));
    }

    #[test]
    fn wrong_code_is_rejected() {
        let mut a = driver("Tablet");
        let mut b = driver("PC");
        let (mut offer, _) = a.begin_pairing();
        offer.code =
            PairingCode::parse(&format!("{:06}", (offer.code.value() + 1) % 1_000_000)).unwrap();
        a.on_event(NetEvent::Connected(info(&b, 2))).unwrap();
        b.on_event(NetEvent::Connected(info(&a, 1))).unwrap();
        let request = b.pair_with(&offer);
        let events = deliver(request, &mut b, &mut a);
        assert!(!a.profile.is_trusted(b.profile.id()));
        assert!(!b.profile.is_trusted(a.profile.id()));
        assert!(events.contains(&DriverEvent::PairFailed {
            device: a.profile.id(),
            reason: PairFailure::Rejected
        }));
    }

    #[test]
    fn untrusted_sync_payloads_are_ignored() {
        let a = driver("Tablet");
        let mut b = driver("PC");
        mark(&a, D1, 1, 1);
        b.on_event(NetEvent::Connected(info(&a, 1))).unwrap();
        let inventory = a
            .engine()
            .inventory()
            .to_bytes::<std::convert::Infallible>()
            .unwrap();
        let out = b
            .on_event(NetEvent::Message {
                from: a.profile.id(),
                bytes: Payload::Sync(inventory).to_bytes(),
            })
            .unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn trusted_devices_converge_after_connecting() {
        let mut a = driver("Tablet");
        let mut b = driver("PC");
        let a_dev = TrustedDevice::new(*a.profile.key().public(), "Tablet", 1);
        let b_dev = TrustedDevice::new(*b.profile.key().public(), "PC", 1);
        a.profile_mut().trust(b_dev);
        b.profile_mut().trust(a_dev);
        mark(&a, D1, 1, 1);
        mark(&b, D1, 2, 1);

        let a_sees_b = a.on_event(NetEvent::Discovered(info(&b, 2))).unwrap();
        let b_sees_a = b.on_event(NetEvent::Discovered(info(&a, 1))).unwrap();
        let connects = a_sees_b
            .iter()
            .chain(&b_sees_a)
            .filter(|x| matches!(x, Action::Connect { .. }))
            .count();
        assert_eq!(connects, 1);

        let from_a = a.on_event(NetEvent::Connected(info(&b, 2))).unwrap();
        let from_b = b.on_event(NetEvent::Connected(info(&a, 1))).unwrap();
        let mut events = deliver(from_a, &mut a, &mut b);
        events.extend(deliver(from_b, &mut b, &mut a));

        assert_eq!(marks(&a, D1), vec![(1, 1), (2, 1)]);
        assert_eq!(marks(&b, D1), vec![(1, 1), (2, 1)]);
        assert!(events.contains(&DriverEvent::Imported(vec![D1])));

        mark(&a, D1, 1, 2);
        let round = a.announce().unwrap();
        deliver(round, &mut a, &mut b);
        assert_eq!(marks(&b, D1), vec![(1, 1), (1, 2), (2, 1)]);

        let offline = b.on_event(NetEvent::Disconnected(a.profile.id())).unwrap();
        assert_eq!(
            offline,
            vec![Action::Event(DriverEvent::PeerOnline {
                device: a.profile.id(),
                online: false
            })]
        );
    }

    fn trusted_pair() -> (SyncDriver<MemReplica>, SyncDriver<MemReplica>) {
        let mut a = driver("Tablet");
        let mut b = driver("PC");
        let a_dev = TrustedDevice::new(*a.profile.key().public(), "Tablet", 1);
        let b_dev = TrustedDevice::new(*b.profile.key().public(), "PC", 1);
        a.profile_mut().trust(b_dev);
        b.profile_mut().trust(a_dev);
        // `small` dials first.
        if a.profile.id() < b.profile.id() {
            (a, b)
        } else {
            (b, a)
        }
    }

    fn dials(actions: &[Action]) -> usize {
        actions
            .iter()
            .filter(|a| matches!(a, Action::Connect { .. }))
            .count()
    }

    #[test]
    fn a_lost_device_is_dialed_again_with_backoff() {
        let (mut small, large) = trusted_pair();
        small
            .on_event(NetEvent::Discovered(info(&large, 2)))
            .unwrap();
        small
            .on_event(NetEvent::Connected(info(&large, 2)))
            .unwrap();
        let t0 = Instant::now();
        assert_eq!(dials(&small.supervise(t0)), 0, "connected: nothing to do");

        small
            .on_event(NetEvent::Disconnected(large.profile.id()))
            .unwrap();
        assert_eq!(dials(&small.supervise(t0)), 1, "dials right away");
        assert_eq!(dials(&small.supervise(t0)), 0, "then waits");
        let mut at = t0;
        let mut gaps = Vec::new();
        let mut last = t0;
        while gaps.len() < 3 {
            at += Duration::from_millis(100);
            if dials(&small.supervise(at)) == 1 {
                gaps.push(at - last);
                last = at;
            }
        }
        assert!(gaps[0] >= Duration::from_secs(1), "{gaps:?}");
        assert!(gaps[1] >= Duration::from_secs(2), "{gaps:?}");
        assert!(gaps[2] >= Duration::from_secs(4), "{gaps:?}");
        assert!(gaps[2] <= Duration::from_secs(5) + Duration::from_millis(100));

        // Back for good: the backoff starts over after the next loss.
        small
            .on_event(NetEvent::Connected(info(&large, 2)))
            .unwrap();
        small.supervise(at);
        small
            .on_event(NetEvent::Disconnected(large.profile.id()))
            .unwrap();
        assert_eq!(dials(&small.supervise(at)), 1);
    }

    #[test]
    fn the_larger_id_dials_once_the_smaller_stays_silent() {
        let (small, mut large) = trusted_pair();
        large
            .on_event(NetEvent::Discovered(info(&small, 1)))
            .unwrap();
        let t0 = Instant::now();
        let mut first = None;
        let mut at = t0;
        while at < t0 + Duration::from_secs(10) {
            if dials(&large.supervise(at)) == 1 {
                first = Some(at - t0);
                break;
            }
            at += Duration::from_millis(100);
        }
        let first = first.expect("the larger id dials eventually");
        assert!(first >= Duration::from_secs(3), "{first:?}");
    }

    #[test]
    fn unrelated_devices_are_never_dialed() {
        let mut a = driver("Tablet");
        let b = driver("PC");
        a.on_event(NetEvent::Discovered(info(&b, 2))).unwrap();
        let t0 = Instant::now();
        assert_eq!(dials(&a.supervise(t0)), 0);
        assert_eq!(dials(&a.supervise(t0 + Duration::from_secs(60))), 0);
    }

    #[test]
    fn a_replaced_channel_forgets_what_the_old_one_claimed() {
        let (mut small, mut large) = trusted_pair();
        mark(&large, D1, 2, 1);
        mark(&small, D1, 1, 1);
        let from_small = small
            .on_event(NetEvent::Connected(info(&large, 2)))
            .unwrap();
        let from_large = large
            .on_event(NetEvent::Connected(info(&small, 1)))
            .unwrap();
        deliver(from_small, &mut small, &mut large);
        deliver(from_large, &mut large, &mut small);
        assert_eq!(marks(&small, D1), vec![(1, 1), (2, 1)]);
        // An edit pushed into a channel that dies on the way: the push
        // notes the peer as holding it.
        mark(&large, D1, 2, 2);
        large.push_doc(D1).unwrap();
        assert_eq!(large.engine().status(D1), DocStatus::Synced);
        // A new channel replaces the old one without a disconnect.
        large
            .on_event(NetEvent::Connected(info(&small, 1)))
            .unwrap();
        assert_eq!(
            large.engine().status(D1),
            DocStatus::Unknown,
            "nothing is known until the peer announces again"
        );
    }

    #[test]
    fn a_peer_back_with_another_session_leaves_and_rejoins() {
        let (mut small, large) = trusted_pair();
        let online = |device| {
            move |out: &[Action], online: bool| {
                out.contains(&Action::Event(DriverEvent::PeerOnline { device, online }))
            }
        };
        let seen = online(large.profile.id());
        let out = small
            .on_event(NetEvent::Connected(info(&large, 2)))
            .unwrap();
        assert!(seen(&out, true) && !seen(&out, false));
        // The same run on a new channel is no departure.
        let out = small
            .on_event(NetEvent::Connected(info(&large, 2)))
            .unwrap();
        assert!(!seen(&out, false));
        let restarted = PeerInfo {
            session: 8,
            ..info(&large, 2)
        };
        let out = small.on_event(NetEvent::Connected(restarted)).unwrap();
        let left = out
            .iter()
            .position(|a| seen(std::slice::from_ref(a), false))
            .expect("the old run leaves");
        let back = out
            .iter()
            .position(|a| seen(std::slice::from_ref(a), true))
            .expect("the new run arrives");
        assert!(left < back);
    }

    #[test]
    fn a_trusted_device_renaming_itself_updates_its_stored_name() {
        let mut a = driver("Tablet");
        let b = driver("PC");
        a.profile_mut()
            .trust(TrustedDevice::new(*b.profile.key().public(), "PC", 1));
        a.on_event(NetEvent::Connected(info(&b, 2))).unwrap();
        let renamed = PeerInfo {
            name: "PC del estudio".into(),
            ..info(&b, 2)
        };
        let out = a.on_event(NetEvent::Renamed(renamed.clone())).unwrap();
        assert_eq!(a.profile.trusted()[0].name, "PC del estudio");
        assert!(out.contains(&Action::Event(DriverEvent::Renamed {
            device: b.profile.id(),
            name: "PC del estudio".into(),
        })));
        // The same name again changes nothing.
        assert!(a.on_event(NetEvent::Renamed(renamed)).unwrap().is_empty());
    }

    #[test]
    fn link_parsing_rejects_garbage() {
        assert_eq!(PairingOffer::parse_link("notein://join?x=1"), None);
        assert_eq!(
            PairingOffer::parse_link("notein://pair?pk=zz&name=&code=123456"),
            None
        );
    }

    fn shared_doc(owner: &SyncDriver<MemReplica>) -> Share {
        mark(owner, D1, 1, 1);
        mark(owner, D2, 1, 1);
        owner.with_replica(|r| r.put_blob(BlobKind::Item, 1, 5, "doc-1"));
        let mut share = Share::new(owner.me(), "Física", ShareMode::Permanent, 1);
        // Permanent shares ask first by default; these tests let people
        // straight in and cover approval on their own.
        share.approve_entry = false;
        share.docs.push(D1);
        share.blobs.push(BlobKey {
            kind: BlobKind::Item,
            id: [1; 16],
        });
        share
    }

    #[test]
    fn guest_joins_with_the_invite_and_receives_only_the_share() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let share = shared_doc(&owner);
        let id = share.id;
        let invite = Invite::parse_link(&share.invite().to_link()).unwrap();
        owner.create_share(share);

        // The guest knows the owner from the LAN and asks in.
        guest
            .on_event(NetEvent::Discovered(info(&owner, 1)))
            .unwrap();
        let actions = guest.join(invite);
        assert!(matches!(actions[..], [Action::Connect { .. }]));
        assert!(
            owner
                .on_event(NetEvent::Connected(info(&guest, 2)))
                .unwrap()
                .is_empty()
        );
        let request = guest
            .on_event(NetEvent::Connected(info(&owner, 1)))
            .unwrap();
        let events = deliver(request, &mut guest, &mut owner);

        assert!(events.contains(&DriverEvent::ShareChanged(id)));
        assert!(guest.share(id).is_some());
        assert_eq!(owner.share(id).unwrap().members.len(), 1);
        assert_eq!(owner.share(id).unwrap().members[0].name, "Ana");
        assert_eq!(marks(&guest, D1), vec![(1, 1)]);
        assert!(marks(&guest, D2).is_empty(), "D2 is outside the share");
        assert_eq!(
            guest.with_replica(|r| r.blob_text(BlobKind::Item, 1)),
            Some("doc-1".to_owned())
        );
        assert!(!guest.profile.is_trusted(owner.profile.id()));

        // Guest edits flow back; a guest edit outside the share stays home.
        mark(&guest, D1, 2, 1);
        mark(&guest, D2, 2, 9);
        let round = guest.announce().unwrap();
        deliver(round, &mut guest, &mut owner);
        assert_eq!(marks(&owner, D1), vec![(1, 1), (2, 1)]);
        assert_eq!(marks(&owner, D2), vec![(1, 1)]);
    }

    #[test]
    fn wrong_key_is_refused() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let share = shared_doc(&owner);
        let id = share.id;
        let mut invite = share.invite();
        invite.key = crate::cipher::ShareKey::generate();
        owner.create_share(share);
        connect_both(&mut owner, &mut guest);
        let request = guest.join(invite);
        let events = deliver(request, &mut guest, &mut owner);
        assert!(events.contains(&DriverEvent::JoinFailed {
            share: id,
            reason: JoinFailure::Refused
        }));
        assert!(guest.share(id).is_none());
        assert!(owner.share(id).unwrap().members.is_empty());
    }

    #[test]
    fn read_only_members_cannot_push_edits() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let mut share = shared_doc(&owner);
        share.entry_role = Role::Viewer;
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        connect_both(&mut owner, &mut guest);
        deliver(guest.join(invite), &mut guest, &mut owner);
        assert_eq!(marks(&guest, D1), vec![(1, 1)]);
        assert!(
            guest
                .share(id)
                .unwrap()
                .is_read_only_for(guest.profile.id())
        );

        mark(&guest, D1, 2, 1);
        deliver(guest.announce().unwrap(), &mut guest, &mut owner);
        assert_eq!(marks(&owner, D1), vec![(1, 1)]);

        // Owner edits still arrive.
        mark(&owner, D1, 1, 2);
        deliver(owner.announce().unwrap(), &mut owner, &mut guest);
        assert_eq!(marks(&guest, D1), vec![(1, 1), (1, 2), (2, 1)]);
    }

    #[test]
    fn owner_ending_or_expelling_removes_the_share_on_the_guest() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let share = shared_doc(&owner);
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        connect_both(&mut owner, &mut guest);
        deliver(guest.join(invite), &mut guest, &mut owner);

        let events = deliver(owner.expel(id, guest.profile.id()), &mut owner, &mut guest);
        assert!(events.contains(&DriverEvent::ShareRemoved {
            share: id,
            reason: Removal::Expelled,
        }));
        assert!(guest.share(id).is_none());
        assert!(owner.share(id).unwrap().members.is_empty());

        let ended = DriverEvent::ShareRemoved {
            share: id,
            reason: Removal::Ended,
        };
        assert!(
            owner
                .end_share(id)
                .into_iter()
                .any(|a| matches!(a, Action::Event(e) if e == ended))
        );
        assert!(owner.share(id).is_none());
    }

    /// A relay this driver uses, already connected.
    fn on_relay(driver: &mut SyncDriver<MemReplica>) -> DeviceId {
        on_relay_at(driver, &crate::identity::DeviceKey::generate())
    }

    /// The relay with key `key` this driver uses, already connected.
    fn on_relay_at(
        driver: &mut SyncDriver<MemReplica>,
        key: &crate::identity::DeviceKey,
    ) -> DeviceId {
        driver.set_relay(Some(RelayAddr {
            host: "relay:7710".into(),
            public: *key.public(),
        }));
        driver
            .on_event(NetEvent::Connected(PeerInfo {
                device: key.id(),
                public: *key.public(),
                name: "relay".into(),
                addr: addr(9),
                relayed: false,
                through_relay: false,
                session: 1,
            }))
            .unwrap();
        key.id()
    }

    /// The relay answered `driver`'s subscription to `id`, at epoch 0.
    fn caught_up(driver: &mut SyncDriver<MemReplica>, relay: DeviceId, id: ShareId) {
        let answer = RelayMessage::Records {
            share: id,
            records: Vec::new(),
            epoch: 0,
        };
        driver.on_event(from_relay(relay, &answer)).unwrap();
    }

    fn from_relay(relay: DeviceId, message: &RelayMessage) -> NetEvent {
        NetEvent::Message {
            from: relay,
            bytes: relay_payload(message),
        }
    }

    /// Owner and guest inside one share, then apart.
    fn joined_then_apart() -> (SyncDriver<MemReplica>, SyncDriver<MemReplica>, ShareId) {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let share = shared_doc(&owner);
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        connect_both(&mut owner, &mut guest);
        deliver(guest.join(invite), &mut guest, &mut owner);
        assert!(guest.share(id).is_some());
        owner
            .on_event(NetEvent::Disconnected(guest.profile.id()))
            .unwrap();
        guest
            .on_event(NetEvent::Disconnected(owner.profile.id()))
            .unwrap();
        (owner, guest, id)
    }

    fn removed(events: &[DriverEvent], share: ShareId, reason: Removal) -> bool {
        events.contains(&DriverEvent::ShareRemoved { share, reason })
    }

    #[test]
    fn a_guest_away_while_the_share_ended_is_told_when_it_returns() {
        let (mut owner, mut guest, id) = joined_then_apart();
        assert!(
            owner
                .end_share(id)
                .iter()
                .all(|a| !matches!(a, Action::Send { .. }))
        );
        // The owner restarts in between: the ending is remembered.
        let book = ShareBook::decode(&owner.book().encode()).unwrap();
        let mut owner = SyncDriver::new(owner.profile.clone(), MemReplica::default(), book);

        let events = connect_both(&mut owner, &mut guest)
            .into_iter()
            .filter_map(|a| match a {
                Action::Event(e) => Some(e),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(removed(&events, id, Removal::Ended), "{events:?}");
        assert!(guest.share(id).is_none());
    }

    #[test]
    fn a_guest_away_while_expelled_is_told_when_it_returns() {
        let (mut owner, mut guest, id) = joined_then_apart();
        owner.expel(id, guest.profile.id());
        let events: Vec<DriverEvent> = connect_both(&mut owner, &mut guest)
            .into_iter()
            .filter_map(|a| match a {
                Action::Event(e) => Some(e),
                _ => None,
            })
            .collect();
        assert!(removed(&events, id, Removal::Expelled), "{events:?}");
        assert!(guest.share(id).is_none());
        assert!(owner.share(id).unwrap().members.is_empty());
    }

    #[test]
    fn a_pending_join_survives_a_restart() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let share = shared_doc(&owner);
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        assert!(guest.join(invite).is_empty(), "the owner is nowhere yet");

        let book = ShareBook::decode(&guest.book().encode()).unwrap();
        let mut guest = SyncDriver::new(guest.profile.clone(), MemReplica::default(), book);
        assert_eq!(guest.pending_joins().count(), 1);
        guest.resume_joins();
        assert_eq!(guest.pending_joins().count(), 1);
        connect_both(&mut owner, &mut guest);
        assert!(guest.share(id).is_some());
        assert_eq!(guest.pending_joins().count(), 0);
    }

    #[test]
    fn permissions_changed_while_away_arrive_on_return() {
        let (mut owner, mut guest, id) = joined_then_apart();
        let device = guest.profile.id();
        owner.edit_share(
            id,
            ShareEdit::SetRole {
                device,
                role: Role::Viewer,
            },
        );
        assert!(guest.share(id).unwrap().can_edit(guest.profile.id()));
        connect_both(&mut guest, &mut owner);
        assert!(!guest.share(id).unwrap().can_edit(guest.profile.id()));
        // Nothing new the next time: no change is reported.
        let events = connect_both(&mut guest, &mut owner);
        assert!(
            !events
                .iter()
                .any(|a| matches!(a, Action::Event(DriverEvent::ShareChanged(_))))
        );
    }

    #[test]
    fn a_share_ended_while_away_is_told_through_the_relay() {
        let (mut owner, mut guest, id) = joined_then_apart();
        let owner_public = *owner.profile.key().public();
        owner.end_share(id);
        let at_guest = on_relay(&mut guest);
        on_relay(&mut owner);
        // The guest asks the owner, dialing it through the relay.
        let asked = guest
            .on_event(from_relay(at_guest, &RelayMessage::Gone { share: id }))
            .unwrap();
        assert!(
            asked.contains(&Action::ConnectVia {
                relay: at_guest,
                public: owner_public,
            }),
            "{asked:?}"
        );
        // The relayed channel opens; what waited for it goes out and the
        // owner answers.
        let relayed = |d: &SyncDriver<MemReplica>| PeerInfo {
            relayed: true,
            ..info(d, 9)
        };
        owner
            .on_event(NetEvent::Connected(relayed(&guest)))
            .unwrap();
        let out = guest
            .on_event(NetEvent::Connected(relayed(&owner)))
            .unwrap();
        let events = deliver(out, &mut guest, &mut owner);
        assert!(
            events.contains(&DriverEvent::ShareRemoved {
                share: id,
                reason: Removal::Ended,
            }),
            "{events:?}"
        );
        assert_eq!(guest.path_to(owner.profile.id()), Some(Path::Relay));
    }

    fn record(seq: u64, from: DeviceId, kind: RecordKind, sealed: Vec<u8>) -> crate::relay::Record {
        crate::relay::Record {
            seq,
            from,
            kind,
            sealed,
            epoch: 0,
        }
    }

    /// Owner, `guest` and `eve` in one share, everyone knowing everyone.
    fn three_members() -> (
        SyncDriver<MemReplica>,
        SyncDriver<MemReplica>,
        SyncDriver<MemReplica>,
        ShareId,
    ) {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let mut eve = driver("Eve");
        let share = shared_doc(&owner);
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        connect_both(&mut owner, &mut guest);
        deliver(guest.join(invite.clone()), &mut guest, &mut owner);
        connect_both(&mut owner, &mut eve);
        deliver(eve.join(invite), &mut eve, &mut owner);
        connect_both(&mut guest, &mut owner);
        assert!(guest.share(id).unwrap().is_member(eve.profile.id()));
        (owner, guest, eve, id)
    }

    fn held_for(driver: &SyncDriver<MemReplica>, device: DeviceId) -> Vec<Held> {
        driver
            .outbox
            .get(&device)
            .map(|q| q.iter().map(|w| w.kind.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn only_one_shot_messages_wait_for_a_device_away() {
        let (mut owner, mut guest, id) = joined_then_apart();
        let owner_id = owner.profile.id();
        let guest_id = guest.profile.id();
        // Rounds are made again on the next channel; nothing waits.
        guest.share_round_for(id, owner_id).unwrap();
        assert!(held_for(&guest, owner_id).is_empty());
        // The same edit asked twice waits once, another edit adds up.
        let rename = |t: &str| ShareEdit::Rename(t.into());
        guest
            .shares
            .get_mut(&id)
            .unwrap()
            .share
            .set_role(guest_id, Role::CoOrganizer);
        guest.edit_share(id, rename("a"));
        guest.edit_share(id, rename("a"));
        guest.edit_share(id, rename("b"));
        let held = held_for(&guest, owner_id);
        assert_eq!(held.len(), 2);
        assert!(held.iter().all(|k| matches!(k, Held::Edit(_))));
        // Leaving makes them moot: only the departure waits.
        guest.leave_share(id);
        assert_eq!(held_for(&guest, owner_id), vec![Held::Left]);

        // On the owner's side the share ending drops what waited for it.
        owner.expel(id, guest_id);
        assert_eq!(held_for(&owner, guest_id), vec![Held::Removal]);
        owner.end_share(id);
        assert!(held_for(&owner, guest_id).is_empty());
    }

    #[test]
    fn what_waits_too_long_is_dropped() {
        let (_owner, mut guest, id) = joined_then_apart();
        let owner_id = guest.share(id).unwrap().owner.device;
        guest.leave_share(id);
        assert_eq!(held_for(&guest, owner_id), vec![Held::Left]);
        guest.supervise(Instant::now() + OUTBOX_TTL + Duration::from_secs(1));
        assert!(held_for(&guest, owner_id).is_empty());
    }

    #[test]
    fn a_device_we_talk_to_is_never_taken_for_a_relay() {
        let (_owner, mut guest, id) = joined_then_apart();
        let phone = driver("Phone");
        guest.profile_mut().trust(TrustedDevice::new(
            *phone.profile.key().public(),
            "Phone",
            1,
        ));
        guest
            .on_event(NetEvent::Connected(info(&phone, 12)))
            .unwrap();
        // The owner names the guest's own phone as the share's relay.
        guest.shares.get_mut(&id).unwrap().share.server = Some(
            RelayAddr {
                host: "phone:7710".into(),
                public: *phone.profile.key().public(),
            }
            .to_string(),
        );
        let out = guest.sync_relays();
        assert!(out.is_empty(), "{out:?}");
        assert!(!guest.relays.contains_key(&phone.profile.id()));
        assert!(guest.connected().any(|p| p.device == phone.profile.id()));
        let out = guest.tick().unwrap();
        assert!(!out.contains(&Action::Disconnect {
            device: phone.profile.id()
        }));
    }

    #[test]
    fn a_relay_no_longer_used_is_let_go_and_stays_gone() {
        let offering = driver("Tablet");
        let mut a = driver("PC");
        on_relay(&mut a);
        let key = crate::identity::DeviceKey::generate();
        let theirs = RelayAddr {
            host: "theirs:7710".into(),
            public: *key.public(),
        };
        let offer = PairingOffer {
            device: offering.profile.id(),
            public: *offering.profile.key().public(),
            name: "Tablet".into(),
            code: PairingCode::random(),
            secret: Some(PairingSecret::random()),
            relay: Some(theirs),
        };
        a.pair_with(&offer);
        let at_theirs = PeerInfo {
            device: key.id(),
            public: *key.public(),
            name: "theirs".into(),
            addr: addr(10),
            relayed: false,
            through_relay: false,
            session: 1,
        };
        a.on_event(NetEvent::Connected(at_theirs.clone())).unwrap();
        a.cancel_pairing();
        let out = a.tick().unwrap();
        assert!(
            out.contains(&Action::Disconnect { device: key.id() }),
            "{out:?}"
        );
        assert!(!a.relays.contains_key(&key.id()));
        // Back again on its own: let go, not taken for a device.
        let out = a.on_event(NetEvent::Connected(at_theirs)).unwrap();
        assert_eq!(out, vec![Action::Disconnect { device: key.id() }]);
        assert!(a.connected().all(|p| p.device != key.id()));
    }

    #[test]
    fn revoking_and_cancelling_tell_the_relay_at_once() {
        let mut a = driver("Tablet");
        let relay = on_relay(&mut a);
        let b = driver("PC");
        a.profile_mut()
            .trust(TrustedDevice::new(*b.profile.key().public(), "PC", 1));
        a.begin_pairing();
        let out = a.cancel_pairing();
        assert!(out.iter().any(|x| matches!(x, Action::Send { device, bytes, .. }
            if *device == relay && relay_message(bytes) == Some(RelayMessage::Expect { token: None }))));
        let out = a.revoke(b.profile.id()).expect("it was trusted");
        assert!(out.iter().any(|x| matches!(x, Action::Send { device, bytes, .. }
            if *device == relay && relay_message(bytes) == Some(RelayMessage::Trust { keys: Vec::new() }))));
        assert!(a.revoke(b.profile.id()).is_none());
    }

    fn consent_sent(out: &[Action], relay: DeviceId) -> bool {
        out.iter().any(|a| match a {
            Action::Send { device, bytes, .. } if *device == relay => matches!(
                relay_message(bytes),
                Some(RelayMessage::Trust { .. } | RelayMessage::Expect { .. })
            ),
            _ => false,
        })
    }

    #[test]
    fn an_invitations_relay_serves_the_share_and_never_hears_whom_we_trust() {
        let owner = driver("Sergio");
        let mut guest = driver("Ana");
        let own = on_relay(&mut guest);
        guest
            .profile_mut()
            .trust(TrustedDevice::new([3; 32], "PC", 1));
        let key = crate::identity::DeviceKey::generate();
        let theirs = RelayAddr {
            host: "theirs:7710".into(),
            public: *key.public(),
        };
        let mut invite = shared_doc(&owner).invite();
        invite.server = Some(theirs.to_string());
        let out = guest.join(invite);
        assert_eq!(
            guest.relay(),
            Some(&RelayAddr {
                host: "relay:7710".into(),
                public: guest.relays[&own].addr.public,
            })
        );
        assert!(
            out.iter().any(
                |a| matches!(a, Action::ConnectHost { public, .. } if *public == theirs.public)
            )
        );
        let out = guest
            .on_event(NetEvent::Connected(PeerInfo {
                device: key.id(),
                public: *key.public(),
                name: "theirs".into(),
                addr: addr(10),
                relayed: false,
                through_relay: false,
                session: 1,
            }))
            .unwrap();
        assert!(!consent_sent(&out, key.id()), "{out:?}");
        // The owner is knocked on there all the same.
        assert!(out.iter().any(|a| matches!(a, Action::Send { device, bytes, .. }
            if *device == key.id() && matches!(relay_message(bytes), Some(RelayMessage::Knock { .. })))));
        let out = guest.tick().unwrap();
        assert!(!consent_sent(&out, key.id()));
        assert!(consent_sent(&out, own));
    }

    #[test]
    fn a_pairing_links_relay_hears_our_knock_and_nothing_else() {
        let mut offering = driver("Tablet");
        on_relay(&mut offering);
        let (offer, _) = offering.begin_pairing();
        let theirs = offer.relay.clone().unwrap();
        let mut b = driver("PC");
        let own = on_relay(&mut b);
        b.profile_mut()
            .trust(TrustedDevice::new([3; 32], "Phone", 1));
        b.pair_with(&offer);
        let out = b
            .on_event(NetEvent::Connected(PeerInfo {
                device: theirs.device(),
                public: theirs.public,
                name: "theirs".into(),
                addr: addr(11),
                relayed: false,
                through_relay: false,
                session: 1,
            }))
            .unwrap();
        assert!(!consent_sent(&out, theirs.device()), "{out:?}");
        assert!(out.iter().any(|a| matches!(a, Action::Send { device, bytes, .. }
            if *device == theirs.device() && matches!(relay_message(bytes), Some(RelayMessage::Knock { .. })))));
        let out = b.tick().unwrap();
        assert!(!consent_sent(&out, theirs.device()));
        assert!(consent_sent(&out, own));
    }

    #[test]
    fn a_device_without_a_relay_takes_the_paired_ones_and_says_so() {
        let mut a = driver("Tablet");
        let mut b = driver("PC");
        on_relay(&mut a);
        let (offer, _) = a.begin_pairing();
        let relay = offer.relay.clone().unwrap();
        let out = b.pair_with(&offer);
        assert!(out.contains(&Action::Event(DriverEvent::RelayAdopted(relay.clone()))));
        assert_eq!(b.relay(), Some(&relay));
        // A device with a relay keeps its own.
        let mut c = driver("Phone");
        let mine = on_relay(&mut c);
        let out = c.pair_with(&offer);
        assert!(
            !out.iter()
                .any(|a| matches!(a, Action::Event(DriverEvent::RelayAdopted(_))))
        );
        assert_eq!(c.own_relay, Some(mine));
    }

    /// What `out` hands the relay `relay` to keep.
    fn published(out: &[Action], relay: DeviceId) -> Vec<(RecordKind, Vec<u8>)> {
        out.iter()
            .filter_map(|a| match a {
                Action::Send { device, bytes, .. } if *device == relay => {
                    match relay_message(bytes) {
                        Some(RelayMessage::Publish { kind, sealed, .. }) => Some((kind, sealed)),
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_knock_let_in_after_the_key_changed_still_opens_its_welcome() {
        let (mut owner, _guest, eve, id) = three_members();
        owner.shares.get_mut(&id).unwrap().share.approve_entry = true;
        let invite = owner.share(id).unwrap().invite();
        let mut carl = driver("Carl");
        connect_both(&mut owner, &mut carl);
        let events = deliver(carl.join(invite), &mut carl, &mut owner);
        assert!(events.contains(&DriverEvent::JoinPending(id)));
        owner.expel(id, eve.profile.id());
        assert_eq!(owner.share(id).unwrap().epoch, 1);
        let carl_id = carl.profile.id();
        let approve = ShareEdit::Approve {
            device: carl_id,
            role: Role::Editor,
        };
        // Away when let in: nothing waits for it in the outbox.
        owner.on_event(NetEvent::Disconnected(carl_id)).unwrap();
        carl.on_event(NetEvent::Disconnected(owner.profile.id()))
            .unwrap();
        owner.edit_share(id, approve);
        assert!(owner.share(id).unwrap().is_member(carl_id));
        assert!(!owner.outbox.contains_key(&carl_id));
        // Back, it asks again and is welcomed with what it can open.
        connect_both(&mut owner, &mut carl);
        let share = carl.share(id).expect("welcomed");
        assert_eq!(share.epoch, 1);
        assert_eq!(share.key, owner.share(id).unwrap().key);
    }

    #[test]
    fn an_answer_for_a_knock_goes_sealed_with_the_key_it_knocked_with() {
        let (mut owner, _guest, eve, id) = three_members();
        owner.shares.get_mut(&id).unwrap().share.approve_entry = true;
        let invite = owner.share(id).unwrap().invite();
        let mut carl = driver("Carl");
        connect_both(&mut owner, &mut carl);
        deliver(carl.join(invite), &mut carl, &mut owner);
        owner.expel(id, eve.profile.id());
        let out = owner.edit_share(
            id,
            ShareEdit::Approve {
                device: carl.profile.id(),
                role: Role::Editor,
            },
        );
        deliver(out, &mut owner, &mut carl);
        assert_eq!(carl.share(id).map(|s| s.epoch), Some(1));
    }

    #[test]
    fn an_expelled_member_reads_nothing_sealed_after_it_is_out() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let mut carl = driver("Carl");
        let mut eve = driver("Eve");
        let share = shared_doc(&owner);
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        for member in [&mut guest, &mut carl, &mut eve] {
            connect_both(&mut owner, member);
            deliver(member.join(invite.clone()), member, &mut owner);
            assert!(member.share(id).is_some());
        }
        // Carl and Eve go away; Ana stays connected.
        for member in [&mut carl, &mut eve] {
            owner
                .on_event(NetEvent::Disconnected(member.profile.id()))
                .unwrap();
            member
                .on_event(NetEvent::Disconnected(owner.profile.id()))
                .unwrap();
        }
        let at_owner = on_relay(&mut owner);
        let at_carl = on_relay(&mut carl);
        let at_eve = on_relay(&mut eve);
        let o = owner.profile.id();

        let out = owner.expel(id, eve.profile.id());
        assert!(out.contains(&Action::Event(DriverEvent::InviteRenewed(id))));
        assert_eq!(owner.share(id).unwrap().epoch, 1);
        let mut kept = published(&out, at_owner);
        assert_eq!(
            kept.iter()
                .filter(|(k, _)| matches!(k, RecordKind::Keys { .. }))
                .count(),
            2,
            "one for Ana, one for Carl, none for Eve"
        );
        deliver(out, &mut owner, &mut guest);
        assert_eq!(guest.share(id).unwrap().epoch, 1, "Ana got it live");

        // The owner writes after Eve is out.
        mark(&owner, D1, 1, 50);
        let out = owner.push_doc(D1).unwrap();
        kept.extend(published(&out, at_owner));
        deliver(out, &mut owner, &mut guest);
        assert!(marks(&guest, D1).contains(&(1, 50)));

        // A relay on Eve's side hands her everything it got.
        let records: Vec<crate::relay::Record> = kept
            .into_iter()
            .zip(1..)
            .map(|((kind, sealed), seq)| record(seq, o, kind, sealed))
            .collect();
        for (member, relay) in [(&mut carl, at_carl), (&mut eve, at_eve)] {
            member
                .on_event(from_relay(
                    relay,
                    &RelayMessage::Records {
                        share: id,
                        records: records.clone(),
                        epoch: 0,
                    },
                ))
                .unwrap();
        }
        assert!(
            marks(&carl, D1).contains(&(1, 50)),
            "Carl, away, still syncs"
        );
        assert_eq!(carl.share(id).unwrap().epoch, 1);
        assert!(
            !marks(&eve, D1).contains(&(1, 50)),
            "Eve read past her expulsion"
        );
        assert_eq!(eve.share(id).unwrap().epoch, 0);
        let new_key = owner.share(id).unwrap().key.clone();
        assert_ne!(eve.share(id).unwrap().key, new_key);
        assert!(!eve.share(id).unwrap().previous.contains(&new_key));
    }

    #[test]
    fn what_the_relay_turned_away_goes_again_once_the_key_is_in() {
        let (mut owner, mut guest, eve, id) = three_members();
        for d in [guest.profile.id(), eve.profile.id()] {
            owner.on_event(NetEvent::Disconnected(d)).unwrap();
        }
        guest
            .on_event(NetEvent::Disconnected(owner.profile.id()))
            .unwrap();
        let at_guest = on_relay(&mut guest);
        caught_up(&mut guest, at_guest, id);
        mark(&guest, D1, 2, 9);
        assert!(!published(&guest.push_doc(D1).unwrap(), at_guest).is_empty());
        // Meanwhile the key changed; the relay turns those away.
        owner.expel(id, eve.profile.id());
        let behind = RelayMessage::Behind {
            share: id,
            epoch: 1,
        };
        let out = guest.on_event(from_relay(at_guest, &behind)).unwrap();
        assert!(published(&out, at_guest).is_empty());
        // The owner hands over the key, its view and its lease.
        let gid = guest.profile.id();
        let handed = [
            owner.keys_for(id, gid),
            owner.meta_for(id, gid),
            owner.lease_for(id, gid),
        ];
        let mut again = Vec::new();
        for action in handed {
            let Some(Action::Send { bytes, .. }) = action else {
                panic!("the owner hands it over");
            };
            let from = owner.profile.id();
            let out = guest.on_event(NetEvent::Message { from, bytes }).unwrap();
            again.extend(published(&out, at_guest));
        }
        assert!(!again.is_empty(), "what was turned away never went again");
        for (kind, sealed) in again {
            assert!(ShareMessage::open_signed(owner.share(id).unwrap(), &sealed, kind).is_ok());
            assert!(ShareMessage::open_signed(eve.share(id).unwrap(), &sealed, kind).is_err());
        }
    }

    #[test]
    fn a_member_that_missed_a_new_key_gets_it_when_it_meets_the_owner() {
        let (mut owner, mut guest, eve, id) = three_members();
        for d in [guest.profile.id(), eve.profile.id()] {
            owner.on_event(NetEvent::Disconnected(d)).unwrap();
        }
        guest
            .on_event(NetEvent::Disconnected(owner.profile.id()))
            .unwrap();
        owner.expel(id, eve.profile.id());
        assert_eq!(guest.share(id).unwrap().epoch, 0);
        connect_both(&mut guest, &mut owner);
        assert_eq!(guest.share(id).unwrap().epoch, 1);
        assert_eq!(guest.share(id).unwrap().key, owner.share(id).unwrap().key);
    }

    #[test]
    fn only_records_that_may_still_open_wait() {
        let (mut owner, mut guest, id) = joined_then_apart();
        let at_owner = on_relay(&mut owner);
        let at_guest = on_relay(&mut guest);
        let stranger = driver("Eve");
        let view = owner.share(id).unwrap().clone();
        let owner_id = owner.profile.id();
        let by_stranger = |seq| {
            let sealed =
                ShareMessage::Ended.seal_signed(&view, RecordKind::Meta, stranger.profile.key());
            record(seq, stranger.profile.id(), RecordKind::Meta, sealed)
        };
        let unknown_key = |seq, epoch| crate::relay::Record {
            epoch,
            ..record(
                seq,
                owner_id,
                RecordKind::Meta,
                ShareMessage::Ended.seal(&ShareKey::generate()),
            )
        };
        let feed = |d: &mut SyncDriver<MemReplica>, relay, records| {
            let answer = RelayMessage::Records {
                share: id,
                records,
                epoch: 0,
            };
            d.on_event(from_relay(relay, &answer)).unwrap();
        };
        // The owner knows its members and keys: nothing waits there.
        feed(
            &mut owner,
            at_owner,
            vec![by_stranger(1), unknown_key(2, 5)],
        );
        assert!(!owner.parked.contains_key(&id));
        // A member waits only for what a newer view or key may open.
        feed(
            &mut guest,
            at_guest,
            vec![
                by_stranger(1),
                unknown_key(2, 0),
                unknown_key(3, 1),
                // Further ahead than one key change: not the owner's.
                unknown_key(4, 2),
            ],
        );
        let waiting: Vec<u64> = guest.parked[&id]
            .records
            .iter()
            .map(|(r, _)| r.seq)
            .collect();
        assert_eq!(waiting, vec![1, 3]);

        // Past the room kept, the oldest stay and the cursor never passes
        // any of them, kept or let go.
        let (owner, mut guest, id) = joined_then_apart();
        let at_guest = on_relay(&mut guest);
        let view = owner.share(id).unwrap().clone();
        let records: Vec<crate::relay::Record> = (10..10 + PARKED_MAX as u64 + 20)
            .map(|seq| {
                let sealed = ShareMessage::Ended.seal_signed(
                    &view,
                    RecordKind::Meta,
                    stranger.profile.key(),
                );
                record(seq, stranger.profile.id(), RecordKind::Meta, sealed)
            })
            .collect();
        let answer = RelayMessage::Records {
            share: id,
            records,
            epoch: 0,
        };
        guest.on_event(from_relay(at_guest, &answer)).unwrap();
        let kept = &guest.parked[&id].records;
        assert_eq!(kept.len(), PARKED_MAX);
        assert_eq!(kept[0].0.seq, 10);
        assert!(guest.share(id).unwrap().relay_cursor < 10);
    }

    #[test]
    fn a_wait_outlives_a_restart_and_then_ends() {
        let (owner, mut guest, id) = joined_then_apart();
        let at_guest = on_relay(&mut guest);
        let stranger = driver("Eve");
        let sealed = ShareMessage::Ended.seal_signed(
            owner.share(id).unwrap(),
            RecordKind::Meta,
            stranger.profile.key(),
        );
        let records = RelayMessage::Records {
            share: id,
            records: vec![
                record(4, stranger.profile.id(), RecordKind::Meta, sealed),
                record(
                    5,
                    owner.profile.id(),
                    RecordKind::Meta,
                    ShareMessage::Meta(Box::new(owner.share(id).unwrap().for_member()))
                        .seal_signed(
                            owner.share(id).unwrap(),
                            RecordKind::Meta,
                            owner.profile.key(),
                        ),
                ),
            ],
            epoch: 0,
        };
        guest.on_event(from_relay(at_guest, &records)).unwrap();
        let share = guest.share(id).unwrap();
        assert_eq!(share.relay_cursor, 3);
        assert_eq!(share.held_back.len(), 1);
        // Restarted, it fetches the record again, still within its wait.
        let book = ShareBook::decode(&guest.book().encode()).unwrap();
        assert_eq!(book.get(id).unwrap().held_back, share.held_back);
        let mut again = SyncDriver::new(guest.profile.clone(), MemReplica::default(), book);
        let at_guest = on_relay(&mut again);
        again.on_event(from_relay(at_guest, &records)).unwrap();
        assert_eq!(again.share(id).unwrap().relay_cursor, 3);
        // Once the wait is over the cursor moves past it, restart or not.
        again.shares.get_mut(&id).unwrap().share.held_back[0].1 = 0;
        again.parked.clear();
        again.on_event(from_relay(at_guest, &records)).unwrap();
        assert_eq!(again.share(id).unwrap().relay_cursor, 5);
        assert!(again.share(id).unwrap().held_back.is_empty());
    }

    /// Owner, Ana, Carl and Eve in one share; then Eve is thrown out while
    /// Ana is away, so Ana still holds the old key and lists Eve. Carl
    /// heard of it from the owner.
    fn stale_member() -> (
        SyncDriver<MemReplica>,
        SyncDriver<MemReplica>,
        SyncDriver<MemReplica>,
        SyncDriver<MemReplica>,
        ShareId,
    ) {
        let (mut owner, mut guest, mut eve, id) = three_members();
        let mut carl = driver("Carl");
        let invite = owner.share(id).unwrap().invite();
        connect_both(&mut owner, &mut carl);
        deliver(carl.join(invite), &mut carl, &mut owner);
        connect_both(&mut guest, &mut owner);
        for member in [&mut guest, &mut eve, &mut carl] {
            owner
                .on_event(NetEvent::Disconnected(member.profile.id()))
                .unwrap();
            member
                .on_event(NetEvent::Disconnected(owner.profile.id()))
                .unwrap();
        }
        assert!(guest.lease_holds(id), "a member in step holds the lease");
        owner.expel(id, eve.profile.id());
        connect_both(&mut carl, &mut owner);
        assert_eq!(carl.share(id).unwrap().epoch, 1);
        assert!(carl.lease_holds(id));
        (owner, guest, carl, eve, id)
    }

    /// The owner's lease as of long ago, on what `member` holds.
    fn old_lease(
        owner: &SyncDriver<MemReplica>,
        member: &SyncDriver<MemReplica>,
        id: ShareId,
    ) -> SignedLease {
        let mut view = member.share(id).unwrap().clone();
        view.lease = None;
        view.issue_lease(owner.profile.key(), now_ms() - crate::share::LEASE_MS - 1);
        view.lease.unwrap()
    }

    #[test]
    fn the_owner_keeps_its_lease_fresh_and_a_member_back_with_it_says_so() {
        let (mut owner, mut guest, id) = joined_then_apart();
        let at_owner = on_relay(&mut owner);
        guest.shares.get_mut(&id).unwrap().share.lease = Some(old_lease(&owner, &guest, id));
        let out = guest.tick().unwrap();
        assert!(out.contains(&Action::Event(DriverEvent::LeaseExpired(id))));
        assert!(guest.waiting_for_owner(id));
        // A lease two days old is renewed and handed out.
        let share = &mut owner.shares.get_mut(&id).unwrap().share;
        share.lease = None;
        share.issue_lease(owner.profile.key(), now_ms() - 2 * 24 * 60 * 60 * 1000);
        let out = owner.tick().unwrap();
        assert!(
            published(&out, at_owner)
                .iter()
                .any(|(kind, _)| *kind == RecordKind::Lease)
        );
        // Meeting the owner brings the guest's lease back.
        let events = connect_both(&mut guest, &mut owner);
        assert!(
            events.contains(&Action::Event(DriverEvent::LeaseRenewed(id))),
            "{events:?}"
        );
        assert!(!guest.waiting_for_owner(id));
    }

    fn leases_to(out: &[Action], device: DeviceId) -> usize {
        out.iter()
            .filter(|a| match a {
                Action::Send {
                    device: d, bytes, ..
                } if *d == device => {
                    matches!(Payload::from_bytes(bytes), Some(Payload::Lease { .. }))
                }
                _ => false,
            })
            .count()
    }

    #[test]
    fn a_newer_lease_goes_on_at_once_to_everyone_inside() {
        let (owner, mut guest, carl, eve, id) = stale_member();
        // Ana is with Eve when Carl's newer lease reaches her.
        guest.on_event(NetEvent::Connected(info(&eve, 3))).unwrap();
        guest.on_event(NetEvent::Connected(info(&carl, 4))).unwrap();
        let lease = carl.share(id).unwrap().lease.clone().unwrap();
        let out = guest
            .on_event(NetEvent::Message {
                from: carl.profile.id(),
                bytes: Payload::Lease { share: id, lease }.to_bytes(),
            })
            .unwrap();
        // It does not hold for her: nothing for Eve, and off to the owner.
        assert!(!guest.lease_holds(id));
        assert_eq!(leases_to(&out, eve.profile.id()), 0);
        drop(owner);

        // Carl, up to date, hears from someone still on the old key: it
        // passes the newest lease straight back.
        let (_owner, guest, mut carl, _eve, id) = stale_member();
        carl.on_event(NetEvent::Connected(info(&guest, 4))).unwrap();
        let old = ShareMessage::Live {
            doc: D1,
            bytes: b"ink".to_vec(),
        }
        .seal(&guest.share(id).unwrap().key);
        let out = carl
            .on_event(NetEvent::Message {
                from: guest.profile.id(),
                bytes: Payload::Share {
                    share: id,
                    sealed: old,
                }
                .to_bytes(),
            })
            .unwrap();
        assert_eq!(leases_to(&out, guest.profile.id()), 1, "{out:?}");
    }

    #[test]
    fn a_holding_lease_adopted_goes_to_everyone_connected() {
        let (mut owner, mut guest, mut eve, id) = three_members();
        for d in [guest.profile.id(), eve.profile.id()] {
            owner.on_event(NetEvent::Disconnected(d)).unwrap();
        }
        connect_both(&mut guest, &mut eve);
        // The owner's lease is renewed while Ana is away from it.
        let share = &mut owner.shares.get_mut(&id).unwrap().share;
        share.issue_lease(owner.profile.key(), now_ms());
        let lease = share.lease.clone().unwrap();
        let out = guest
            .on_event(NetEvent::Message {
                from: owner.profile.id(),
                bytes: Payload::Lease { share: id, lease }.to_bytes(),
            })
            .unwrap();
        assert!(guest.lease_holds(id));
        assert_eq!(leases_to(&out, eve.profile.id()), 1, "{out:?}");
    }

    #[test]
    fn a_lease_for_other_members_sends_for_the_owner() {
        // The owner changed a role while Ana was away: same key, new roster.
        let (mut owner, mut guest, eve, id) = three_members();
        let mut carl = driver("Carl");
        let invite = owner.share(id).unwrap().invite();
        connect_both(&mut owner, &mut carl);
        deliver(carl.join(invite), &mut carl, &mut owner);
        connect_both(&mut guest, &mut owner);
        let relay = on_relay(&mut guest);
        for d in [guest.profile.id(), carl.profile.id(), eve.profile.id()] {
            owner.on_event(NetEvent::Disconnected(d)).unwrap();
        }
        guest
            .on_event(NetEvent::Disconnected(owner.profile.id()))
            .unwrap();
        owner.edit_share(
            id,
            ShareEdit::SetRole {
                device: eve.profile.id(),
                role: Role::Viewer,
            },
        );
        connect_both(&mut carl, &mut owner);
        let lease = carl.share(id).unwrap().lease.clone().unwrap();
        assert_eq!(lease.lease.epoch, guest.share(id).unwrap().epoch);
        let out = guest
            .on_event(NetEvent::Message {
                from: carl.profile.id(),
                bytes: Payload::Lease { share: id, lease }.to_bytes(),
            })
            .unwrap();
        assert!(!guest.lease_holds(id));
        let owner_public = *owner.profile.key().public();
        assert!(
            out.contains(&Action::ConnectVia {
                relay,
                public: owner_public
            }),
            "{out:?}"
        );
    }

    #[test]
    fn only_devices_dealt_with_may_send_payloads_in_parts() {
        let (mut owner, guest, id) = joined_then_apart();
        let stranger = driver("Eve");
        let bulk = |out: &[Action], device: DeviceId| {
            out.iter().find_map(|a| match a {
                Action::Bulk { device: d, allowed } if *d == device => Some(*allowed),
                _ => None,
            })
        };
        let out = owner
            .on_event(NetEvent::Connected(info(&stranger, 5)))
            .unwrap();
        assert_eq!(
            bulk(&out, stranger.profile.id()),
            None,
            "a stranger stays out"
        );
        let out = owner
            .on_event(NetEvent::Connected(info(&guest, 6)))
            .unwrap();
        assert_eq!(bulk(&out, guest.profile.id()), Some(true));
        // Thrown out, it is out of this too.
        let out = owner.expel(id, guest.profile.id());
        assert_eq!(bulk(&out, guest.profile.id()), Some(false));
    }

    #[test]
    fn a_clock_put_back_does_not_revive_a_lease() {
        let (_owner, mut guest, id) = joined_then_apart();
        assert!(guest.lease_holds(id));
        // This device once saw a time past the lease's days.
        guest.shares.get_mut(&id).unwrap().share.clock_mark = now_ms() + 2 * crate::share::LEASE_MS;
        assert!(!guest.lease_holds(id));
        // Nor one kept here longer than its days, whatever the clock says.
        let (_owner, mut guest, id) = joined_then_apart();
        let seq = guest.share(id).unwrap().lease.as_ref().unwrap().lease.seq;
        if let Some(long_ago) =
            Instant::now().checked_sub(Duration::from_millis(crate::share::LEASE_MS + 1000))
        {
            guest.lease_seen.insert(id, (seq, long_ago));
            assert!(!guest.lease_holds(id));
        }
    }

    #[test]
    fn an_announcement_left_unanswered_is_not_counted_as_answered() {
        let (owner, mut guest, carl, eve, id) = stale_member();
        // Ana's lease ran out: she answers Carl with nothing.
        guest.shares.get_mut(&id).unwrap().share.lease = Some(old_lease(&owner, &guest, id));
        drop((owner, eve));
        mark(&guest, D1, 2, 98);
        guest.on_event(NetEvent::Connected(info(&carl, 4))).unwrap();
        let inventory = carl.shares[&id].engine.inventory();
        let sealed = ShareMessage::Sync(inventory.to_bytes::<std::convert::Infallible>().unwrap())
            .seal(&guest.share(id).unwrap().key);
        let out = guest
            .on_event(NetEvent::Message {
                from: carl.profile.id(),
                bytes: Payload::Share { share: id, sealed }.to_bytes(),
            })
            .unwrap();
        assert!(
            !out.iter()
                .any(|a| matches!(a, Action::Send { device, .. } if *device == carl.profile.id()))
        );
        // Nothing of it was kept: a later edit does not build on an answer
        // never sent.
        mark(&guest, D1, 2, 99);
        let link = guest.shares.get_mut(&id).unwrap();
        assert!(
            link.engine
                .push_doc(carl.profile.id(), D1)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_relay_lying_about_the_key_gets_nothing_sealed_with_the_old_one() {
        // Eve's relay hides that the key changed and withholds the new one.
        let (owner, mut guest, _carl, eve, id) = stale_member();
        let relay = on_relay(&mut guest);
        caught_up(&mut guest, relay, id);
        let eves = eve.share(id).unwrap().clone();
        let opens = |out: &[Action]| {
            published(out, relay)
                .iter()
                .any(|(_, sealed)| eves.open_any(sealed).is_some())
        };
        // Once the lease runs out, nothing goes to the mailbox.
        guest.shares.get_mut(&id).unwrap().share.lease = Some(old_lease(&owner, &guest, id));
        assert!(!guest.lease_holds(id));
        let events = guest.tick().unwrap();
        assert!(events.contains(&Action::Event(DriverEvent::LeaseExpired(id))));
        mark(&guest, D1, 2, 7);
        assert!(!opens(&guest.push_doc(D1).unwrap()));
        let left = guest.leave_share(id);
        assert!(
            published(&left, relay).is_empty(),
            "a departure went out under no lease"
        );

        // Nor sooner, once a member passes on the owner's newer lease.
        let (_owner, mut guest, mut carl, _eve, id) = stale_member();
        let relay = on_relay(&mut guest);
        caught_up(&mut guest, relay, id);
        connect_both(&mut guest, &mut carl);
        assert!(
            !guest.lease_holds(id),
            "a newer lease tells the key changed"
        );
        mark(&guest, D1, 2, 8);
        assert!(published(&guest.push_doc(D1).unwrap(), relay).is_empty());
        assert!(published(&guest.leave_share(id), relay).is_empty());
    }

    #[test]
    fn a_member_behind_on_the_key_shares_nothing_with_the_one_thrown_out() {
        // Its lease run out: Eve, still listed here, hears nothing.
        let (owner, mut guest, _carl, eve, id) = stale_member();
        guest.shares.get_mut(&id).unwrap().share.lease = Some(old_lease(&owner, &guest, id));
        let eve_id = eve.profile.id();
        let to_eve = |out: &[Action]| {
            out.iter()
                .filter(|a| matches!(a, Action::Send { device, .. } if *device == eve_id))
                .count()
        };
        let out = guest.on_event(NetEvent::Connected(info(&eve, 3))).unwrap();
        assert_eq!(to_eve(&out), 0, "{out:?}");
        assert_eq!(to_eve(&guest.live(D1, b"ink")), 0);
        mark(&guest, D1, 2, 9);
        assert_eq!(to_eve(&guest.push_doc(D1).unwrap()), 0);
        assert_eq!(to_eve(&guest.announce().unwrap()), 0);

        // Lease still young, but Carl passes on the owner's newer one:
        // Ana stops at once and goes to find the owner.
        let (owner, mut guest, mut carl, eve, id) = stale_member();
        let relay = on_relay(&mut guest);
        let eve_id = eve.profile.id();
        let carl_id = carl.profile.id();
        let out = carl.on_event(NetEvent::Connected(info(&guest, 4))).unwrap();
        let mut dialed = false;
        for action in out {
            if let Action::Send { device, bytes, .. } = action
                && device == guest.profile.id()
            {
                let replies = guest
                    .on_event(NetEvent::Message {
                        from: carl_id,
                        bytes,
                    })
                    .unwrap();
                dialed |= replies
                    .iter()
                    .any(|a| matches!(a, Action::Connect { .. } | Action::ConnectVia { .. }));
            }
        }
        assert!(!guest.lease_holds(id));
        assert!(dialed, "the owner is sought out");
        drop((owner, relay));
        let out = guest.on_event(NetEvent::Connected(info(&eve, 3))).unwrap();
        assert_eq!(
            out.iter()
                .filter(|a| matches!(a, Action::Send { device, .. } if *device == eve_id))
                .count(),
            0
        );
        // Still the owner may be talked to, to catch up.
        assert!(guest.may_share(id, guest.share(id).unwrap().owner.device));
    }

    #[test]
    fn a_member_behind_on_the_key_publishes_nothing_until_it_catches_up() {
        let mut owner = driver("Sergio");
        let mut carl = driver("Carl");
        let mut eve = driver("Eve");
        let share = shared_doc(&owner);
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        for member in [&mut carl, &mut eve] {
            connect_both(&mut owner, member);
            deliver(member.join(invite.clone()), member, &mut owner);
            owner
                .on_event(NetEvent::Disconnected(member.profile.id()))
                .unwrap();
            member
                .on_event(NetEvent::Disconnected(owner.profile.id()))
                .unwrap();
        }
        // One relay for everyone, as the share names it.
        let relay_key = crate::identity::DeviceKey::generate();
        let at_owner = on_relay_at(&mut owner, &relay_key);
        let mut handed = published(&owner.expel(id, eve.profile.id()), at_owner);
        handed.extend(published(&owner.announce().unwrap(), at_owner));
        let keys: Vec<crate::relay::Record> = handed
            .into_iter()
            .zip(1..)
            .map(|((kind, sealed), seq)| crate::relay::Record {
                epoch: 1,
                ..record(seq, owner.profile.id(), kind, sealed)
            })
            .collect();
        // Carl comes back on the relay alone and writes at once.
        let at_carl = on_relay_at(&mut carl, &relay_key);
        mark(&carl, D1, 3, 1);
        let out = carl.push_doc(D1).unwrap();
        assert!(
            published(&out, at_carl).is_empty(),
            "published before the relay answered"
        );
        let answer = |records| RelayMessage::Records {
            share: id,
            records,
            epoch: 1,
        };
        let out = carl
            .on_event(from_relay(at_carl, &answer(Vec::new())))
            .unwrap();
        assert!(
            published(&out, at_carl).is_empty(),
            "published with the old key"
        );
        // With the key in, it publishes, sealed with it.
        let out = carl.on_event(from_relay(at_carl, &answer(keys))).unwrap();
        assert_eq!(carl.share(id).unwrap().epoch, 1);
        let sent = published(&out, at_carl);
        assert!(!sent.is_empty());
        let eves = eve.share(id).unwrap();
        for (kind, sealed) in sent {
            assert!(ShareMessage::open_signed(eves, &sealed, kind).is_err());
            assert!(ShareMessage::open_signed(owner.share(id).unwrap(), &sealed, kind).is_ok());
        }
    }

    #[test]
    fn an_expelled_member_waits_for_an_organizer_to_come_back() {
        let (mut owner, mut guest, id) = joined_then_apart();
        let old = owner.share(id).unwrap().invite();
        let device = guest.profile.id();
        owner.expel(id, device);
        // Told it is out once it is back; the old link no longer works.
        connect_both(&mut owner, &mut guest);
        assert!(guest.share(id).is_none());
        let events = deliver(guest.join(old), &mut guest, &mut owner);
        assert!(
            events.contains(&DriverEvent::JoinFailed {
                share: id,
                reason: JoinFailure::Refused,
            }),
            "{events:?}"
        );
        // Even with a new one it waits for someone to let it in.
        let invite = owner.invite_for(id).unwrap();
        let events = deliver(guest.join(invite), &mut guest, &mut owner);
        assert!(events.contains(&DriverEvent::JoinPending(id)), "{events:?}");
        let share = owner.share(id).unwrap();
        assert!(!share.is_member(device));
        assert!(share.pending.iter().any(|p| p.device == device));
        // Only an explicit yes lets it back in.
        let out = owner.edit_share(
            id,
            ShareEdit::Approve {
                device,
                role: Role::Editor,
            },
        );
        deliver(out, &mut owner, &mut guest);
        assert!(owner.share(id).unwrap().is_member(device));
        assert!(!owner.share(id).unwrap().was_expelled(device));
        assert!(guest.share(id).is_some());
    }

    #[test]
    fn a_member_leaving_while_the_owner_is_away_is_told_through_the_mailbox() {
        let (mut owner, mut guest, id) = joined_then_apart();
        let at_guest = on_relay(&mut guest);
        caught_up(&mut guest, at_guest, id);
        let at_owner = on_relay(&mut owner);
        let out = guest.leave_share(id);
        let sealed = out
            .iter()
            .find_map(|a| match a {
                Action::Send { device, bytes, .. } if *device == at_guest => {
                    match relay_message(bytes) {
                        Some(RelayMessage::Publish {
                            kind: RecordKind::Left,
                            sealed,
                            ..
                        }) => Some(sealed),
                        _ => None,
                    }
                }
                _ => None,
            })
            .expect("the departure goes to the mailbox");
        let guest_id = guest.profile.id();
        // Someone else's departure cannot be filed in the guest's name.
        let eve = driver("Eve");
        let forged = ShareMessage::Left { joined_at: 1 }.seal_signed(
            owner.share(id).unwrap(),
            RecordKind::Left,
            eve.profile.key(),
        );
        let records = vec![
            record(1, guest_id, RecordKind::Left, forged),
            record(2, guest_id, RecordKind::Left, sealed),
        ];
        owner
            .on_event(from_relay(
                at_owner,
                &RelayMessage::Records {
                    share: id,
                    records,
                    epoch: 0,
                },
            ))
            .unwrap();
        assert!(!owner.share(id).unwrap().is_member(guest_id));
        assert_eq!(
            owner.share(id).unwrap().epoch,
            1,
            "a departure changes the key"
        );
    }

    #[test]
    fn an_old_departure_does_not_throw_out_a_member_who_came_back() {
        for told_live in [false, true] {
            let (mut owner, mut guest, id) = joined_then_apart();
            let at_guest = on_relay(&mut guest);
            caught_up(&mut guest, at_guest, id);
            let at_owner = on_relay(&mut owner);
            let sealed = published(&guest.leave_share(id), at_guest)
                .into_iter()
                .find(|(k, _)| *k == RecordKind::Left)
                .map(|(_, s)| s)
                .expect("the departure goes to the mailbox");
            if !told_live {
                // Back on the network before the owner heard it left.
                guest.outbox.clear();
            }
            std::thread::sleep(Duration::from_millis(3));
            connect_both(&mut owner, &mut guest);
            deliver(guest.rejoin(id), &mut guest, &mut owner);
            let guest_id = guest.profile.id();
            assert!(
                owner.share(id).unwrap().is_member(guest_id),
                "live {told_live}"
            );
            let records = RelayMessage::Records {
                share: id,
                records: vec![record(1, guest_id, RecordKind::Left, sealed)],
                epoch: 0,
            };
            owner.on_event(from_relay(at_owner, &records)).unwrap();
            assert!(
                owner.share(id).unwrap().is_member(guest_id),
                "live {told_live}"
            );
            // Read again after a restart: still nothing.
            let book = ShareBook::decode(&owner.book().encode()).unwrap();
            let mut owner = SyncDriver::new(owner.profile.clone(), MemReplica::default(), book);
            let at_owner = on_relay(&mut owner);
            owner.on_event(from_relay(at_owner, &records)).unwrap();
            assert!(
                owner.share(id).unwrap().is_member(guest_id),
                "live {told_live}"
            );
        }
    }

    #[test]
    fn a_newcomers_records_ahead_of_the_view_listing_it_still_count() {
        for split in [false, true] {
            let (mut owner, mut guest, id) = joined_then_apart();
            let relay = on_relay(&mut guest);
            let mut eve = driver("Eve");
            let invite = owner.share(id).unwrap().invite();
            connect_both(&mut owner, &mut eve);
            deliver(eve.join(invite), &mut eve, &mut owner);
            mark(&eve, D1, 7, 1);
            let snapshot = Message::DocSnapshot {
                doc: D1,
                bytes: eve.with_replica(|r| r.doc_snapshot(D1).unwrap()),
            };
            let kind = RecordKind::Snapshot { doc: D1 };
            let by_eve =
                ShareMessage::Sync(snapshot.to_bytes::<std::convert::Infallible>().unwrap())
                    .seal_signed(eve.share(id).unwrap(), kind, eve.profile.key());
            let view = ShareMessage::Meta(Box::new(owner.share(id).unwrap().for_member()))
                .seal_signed(
                    owner.share(id).unwrap(),
                    RecordKind::Meta,
                    owner.profile.key(),
                );
            let first = vec![record(10, eve.profile.id(), kind, by_eve)];
            let then = vec![record(12, owner.profile.id(), RecordKind::Meta, view)];
            let batches = if split {
                vec![first, then]
            } else {
                vec![first.into_iter().chain(then).collect()]
            };
            for records in batches {
                guest
                    .on_event(from_relay(
                        relay,
                        &RelayMessage::Records {
                            share: id,
                            records,
                            epoch: 0,
                        },
                    ))
                    .unwrap();
            }
            assert!(marks(&guest, D1).contains(&(7, 1)), "split {split}");
            assert_eq!(guest.share(id).unwrap().relay_cursor, 12);
        }
    }

    #[test]
    fn an_older_view_replayed_after_a_newer_one_changes_nothing() {
        let (mut owner, mut guest, id) = joined_then_apart();
        let relay = on_relay(&mut guest);
        let sealed_view = |owner: &SyncDriver<MemReplica>| {
            ShareMessage::Meta(Box::new(owner.share(id).unwrap().for_member())).seal_signed(
                owner.share(id).unwrap(),
                RecordKind::Meta,
                owner.profile.key(),
            )
        };
        // Two changes within one millisecond still order: the clock here
        // stays behind the share's.
        owner.shares.get_mut(&id).unwrap().share.updated_at = u64::MAX / 2;
        owner.edit_share(id, ShareEdit::Rename("Primera".into()));
        let first = sealed_view(&owner);
        owner.edit_share(id, ShareEdit::Rename("Segunda".into()));
        let second = sealed_view(&owner);
        let o = owner.profile.id();
        let records = vec![
            record(1, o, RecordKind::Meta, second),
            record(2, o, RecordKind::Meta, first),
        ];
        guest
            .on_event(from_relay(
                relay,
                &RelayMessage::Records {
                    share: id,
                    records,
                    epoch: 0,
                },
            ))
            .unwrap();
        assert_eq!(guest.share(id).unwrap().title, "Segunda");
    }

    #[test]
    fn a_share_naming_a_relay_not_linked_yet_takes_no_records_from_ours() {
        let (owner, mut guest, id) = joined_then_apart();
        let own = on_relay(&mut guest);
        guest.shares.get_mut(&id).unwrap().share.server = Some(
            RelayAddr {
                host: "unseen:7710".into(),
                public: *crate::identity::DeviceKey::generate().public(),
            }
            .to_string(),
        );
        let mut view = owner.share(id).unwrap().for_member();
        view.title = "Química".into();
        view.updated_at += 5;
        let sealed = ShareMessage::Meta(Box::new(view)).seal_signed(
            owner.share(id).unwrap(),
            RecordKind::Meta,
            owner.profile.key(),
        );
        let records = RelayMessage::Records {
            share: id,
            records: vec![record(3, owner.profile.id(), RecordKind::Meta, sealed)],
            epoch: 0,
        };
        guest.on_event(from_relay(own, &records)).unwrap();
        assert_ne!(guest.share(id).unwrap().title, "Química");
        assert_eq!(guest.share(id).unwrap().relay_cursor, 0);
    }

    #[test]
    fn a_share_moving_to_another_relay_is_read_there_from_the_start() {
        let (mut owner, mut guest, id) = joined_then_apart();
        guest.shares.get_mut(&id).unwrap().share.relay_cursor = 40;
        owner.shares.get_mut(&id).unwrap().share.relay_cursor = 40;
        let key = crate::identity::DeviceKey::generate();
        let moved = RelayAddr {
            host: "new:7710".into(),
            public: *key.public(),
        };
        owner.set_relay(Some(moved.clone()));
        let share = owner.share(id).unwrap();
        assert_eq!(share.server, Some(moved.to_string()));
        assert_eq!(share.relay_cursor, 0);
        // The guest learns it from the owner's view and starts over too.
        connect_both(&mut guest, &mut owner);
        let share = guest.share(id).unwrap();
        assert_eq!(share.server, Some(moved.to_string()));
        assert_eq!(share.relay_cursor, 0);
        assert!(guest.relays.contains_key(&key.id()));
    }

    #[test]
    fn records_count_only_from_the_relay_the_share_meets_at() {
        let (owner, mut guest, id) = joined_then_apart();
        let own = on_relay(&mut guest);
        // The share meets at another relay than the guest's own.
        let key = crate::identity::DeviceKey::generate();
        let theirs = RelayAddr {
            host: "theirs:7710".into(),
            public: *key.public(),
        };
        guest.shares.get_mut(&id).unwrap().share.server = Some(theirs.to_string());
        guest.sync_relays();
        guest
            .on_event(NetEvent::Connected(PeerInfo {
                device: key.id(),
                public: *key.public(),
                name: "theirs".into(),
                addr: addr(10),
                relayed: false,
                through_relay: false,
                session: 1,
            }))
            .unwrap();
        let held = guest.share(id).unwrap().clone();
        let mut view = owner.share(id).unwrap().for_member();
        view.title = "Química".into();
        view.server = Some(theirs.to_string());
        view.updated_at += 5;
        let sealed = ShareMessage::Meta(Box::new(view)).seal_signed(
            owner.share(id).unwrap(),
            RecordKind::Meta,
            owner.profile.key(),
        );
        let records = RelayMessage::Records {
            share: id,
            records: vec![record(3, owner.profile.id(), RecordKind::Meta, sealed)],
            epoch: 0,
        };
        let out = guest.on_event(from_relay(own, &records)).unwrap();
        assert_eq!(guest.share(id).unwrap(), &held, "{out:?}");
        let out = guest
            .on_event(from_relay(own, &RelayMessage::Gone { share: id }))
            .unwrap();
        assert!(out.is_empty(), "{out:?}");
        guest.on_event(from_relay(key.id(), &records)).unwrap();
        assert_eq!(guest.share(id).unwrap().title, "Química");
        assert_eq!(guest.share(id).unwrap().relay_cursor, 3);
    }

    #[test]
    fn mailbox_records_only_count_as_their_signed_author() {
        let (owner, mut guest, eve, id) = three_members();
        guest
            .on_event(NetEvent::Disconnected(owner.profile.id()))
            .unwrap();
        let relay = on_relay(&mut guest);
        let held = guest.share(id).unwrap().clone();
        // Eve, with the key and the relay on her side, writes a view of
        // the share as if she were the owner.
        let mut forged = owner.share(id).unwrap().for_member();
        forged.members.retain(|m| m.device == eve.profile.id());
        forged.server = Some("evil:1/00".into());
        forged.updated_at += 10;
        let meta = ShareMessage::Meta(Box::new(forged));
        let as_owner = meta.seal_signed(&held, RecordKind::Meta, eve.profile.key());
        let unsigned = meta.seal(&held.key);
        let records = vec![
            record(5, owner.profile.id(), RecordKind::Meta, as_owner.clone()),
            record(6, eve.profile.id(), RecordKind::Meta, as_owner),
            record(7, owner.profile.id(), RecordKind::Meta, unsigned),
            record(
                8,
                eve.profile.id(),
                RecordKind::Meta,
                ShareMessage::Expelled.seal_signed(&held, RecordKind::Meta, eve.profile.key()),
            ),
            record(
                u64::MAX,
                owner.profile.id(),
                RecordKind::Meta,
                b"garbage".to_vec(),
            ),
        ];
        let out = guest
            .on_event(from_relay(
                relay,
                &RelayMessage::Records {
                    share: id,
                    records,
                    epoch: 0,
                },
            ))
            .unwrap();
        assert!(
            !out.iter()
                .any(|a| matches!(a, Action::Event(DriverEvent::ShareRemoved { .. }))),
            "{out:?}"
        );
        let now = guest.share(id).unwrap();
        assert_eq!(now.members, held.members);
        assert_eq!(now.server, held.server);
        // The unsigned record either reads as one by someone not listed
        // yet, and holds the cursor before it, or as no record at all; its
        // bytes are the random share's, so either may happen.
        assert!(matches!(now.relay_cursor, 6 | 8), "{}", now.relay_cursor);

        // The owner's own signed view does land.
        let mut real = owner.share(id).unwrap().for_member();
        real.title = "Química".into();
        real.updated_at += 20;
        let sealed = ShareMessage::Meta(Box::new(real)).seal_signed(
            owner.share(id).unwrap(),
            RecordKind::Meta,
            owner.profile.key(),
        );
        guest
            .on_event(from_relay(
                relay,
                &RelayMessage::Records {
                    share: id,
                    records: vec![record(9, owner.profile.id(), RecordKind::Meta, sealed)],
                    epoch: 0,
                },
            ))
            .unwrap();
        assert_eq!(guest.share(id).unwrap().title, "Química");
        // What never opens is given up on in time, and the cursor catches up.
        if let Some(parked) = guest.parked.get_mut(&id) {
            for (_, since) in &mut parked.records {
                *since = 0;
            }
        }
        let empty = RelayMessage::Records {
            share: id,
            records: Vec::new(),
            epoch: 0,
        };
        guest.on_event(from_relay(relay, &empty)).unwrap();
        assert_eq!(guest.share(id).unwrap().relay_cursor, 9);
    }

    #[test]
    fn a_relay_that_lost_a_share_does_not_end_it() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let share = shared_doc(&owner);
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        connect_both(&mut owner, &mut guest);
        deliver(guest.join(invite), &mut guest, &mut owner);
        guest
            .on_event(NetEvent::Disconnected(owner.profile.id()))
            .unwrap();

        let relay = on_relay(&mut guest);
        let out = guest
            .on_event(from_relay(relay, &RelayMessage::Gone { share: id }))
            .unwrap();
        assert!(guest.share(id).is_some());
        assert!(
            !out.iter()
                .any(|a| matches!(a, Action::Event(DriverEvent::ShareRemoved { .. })))
        );
        // The owner is asked instead, over a channel through the relay.
        let asked = out.iter().any(|a| {
            matches!(a, Action::ConnectVia { relay: r, public } if *r == relay && public == owner.profile.key().public())
        });
        assert!(asked, "{out:?}");
    }

    #[test]
    fn a_guest_leaving_drops_out_of_the_owners_member_list() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let share = shared_doc(&owner);
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        connect_both(&mut owner, &mut guest);
        deliver(guest.join(invite), &mut guest, &mut owner);
        assert_eq!(owner.share(id).unwrap().members.len(), 1);

        let events = deliver(guest.leave_share(id), &mut guest, &mut owner);
        assert!(events.contains(&DriverEvent::ShareRemoved {
            share: id,
            reason: Removal::Left,
        }));
        assert!(events.contains(&DriverEvent::ShareChanged(id)));
        assert!(guest.share(id).is_none());
        assert!(owner.share(id).unwrap().members.is_empty());
    }

    #[test]
    fn role_changes_reach_members() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let share = shared_doc(&owner);
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        connect_both(&mut owner, &mut guest);
        deliver(guest.join(invite), &mut guest, &mut owner);
        assert!(guest.share(id).unwrap().can_edit(guest.profile.id()));

        let device = guest.profile.id();
        let actions = owner.edit_share(
            id,
            ShareEdit::SetRole {
                device,
                role: Role::Viewer,
            },
        );
        deliver(actions, &mut owner, &mut guest);
        assert!(!guest.share(id).unwrap().can_edit(guest.profile.id()));
        assert_eq!(guest.share(id).unwrap().key, owner.share(id).unwrap().key);
    }

    /// Members ask the owner for changes; the owner applies what their
    /// role allows and everyone hears the result.
    #[test]
    fn members_edit_through_the_owner_within_their_role() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let share = shared_doc(&owner);
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        connect_both(&mut owner, &mut guest);
        deliver(guest.join(invite), &mut guest, &mut owner);

        // An editor may rename…
        let actions = guest.edit_share(id, ShareEdit::Rename("Física II".into()));
        assert!(!actions.is_empty());
        let events = deliver(actions, &mut guest, &mut owner);
        assert!(events.contains(&DriverEvent::ShareChanged(id)));
        assert_eq!(owner.share(id).unwrap().title, "Física II");
        assert_eq!(guest.share(id).unwrap().title, "Física II");

        // …but not add documents until the owner says editors may.
        let add = ShareEdit::AddDocs {
            docs: vec![D2],
            blobs: Vec::new(),
        };
        assert!(guest.edit_share(id, add.clone()).is_empty());
        let actions = owner.edit_share(id, ShareEdit::SetEditorsAddDocs(true));
        deliver(actions, &mut owner, &mut guest);
        let actions = guest.edit_share(id, add);
        deliver(actions, &mut guest, &mut owner);
        assert!(owner.share(id).unwrap().docs.contains(&D2));
        assert!(guest.share(id).unwrap().docs.contains(&D2));

        // Promoting oneself is not on the menu.
        let me = guest.profile.id();
        assert!(
            guest
                .edit_share(
                    id,
                    ShareEdit::SetRole {
                        device: me,
                        role: Role::CoOrganizer
                    }
                )
                .is_empty()
        );
    }

    /// A document a member brings in reaches the owner as soon as the
    /// owner agrees, without waiting for the next tick.
    #[test]
    fn documents_a_member_brings_in_travel_right_away() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let mut share = shared_doc(&owner);
        share.editors_add_docs = true;
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        connect_both(&mut owner, &mut guest);
        deliver(guest.join(invite), &mut guest, &mut owner);

        let d3 = DocId([3; 16]);
        mark(&guest, d3, 2, 1);
        guest.with_replica(|r| r.put_blob(BlobKind::Item, 3, 7, "doc-3"));
        let item = BlobKey {
            kind: BlobKind::Item,
            id: [3; 16],
        };
        let actions = guest.edit_share(
            id,
            ShareEdit::AddDocs {
                docs: vec![d3],
                blobs: vec![item],
            },
        );
        deliver(actions, &mut guest, &mut owner);
        assert_eq!(owner.with_replica(|r| r.marks(d3)), vec![(2, 1)]);
        assert!(owner.with_replica(|r| r.blobs.contains_key(&item)));
    }

    /// With approval on, a joiner waits until an organizer lets them in
    /// and gets the role chosen then.
    #[test]
    fn approval_holds_a_joiner_until_an_organizer_answers() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let mut share = shared_doc(&owner);
        share.approve_entry = true;
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        connect_both(&mut owner, &mut guest);
        let events = deliver(guest.join(invite), &mut guest, &mut owner);
        assert!(events.contains(&DriverEvent::JoinPending(id)));
        assert!(events.iter().any(|e| matches!(
            e,
            DriverEvent::EntryRequested { share, member } if *share == id && member.name == "Ana"
        )));
        assert!(guest.share(id).is_none());
        assert!(guest.pending_joins().any(|j| j.awaiting_approval));
        assert_eq!(owner.share(id).unwrap().pending.len(), 1);

        let device = guest.profile.id();
        let actions = owner.edit_share(
            id,
            ShareEdit::Approve {
                device,
                role: Role::Viewer,
            },
        );
        let events = deliver(actions, &mut owner, &mut guest);
        assert!(events.contains(&DriverEvent::ShareChanged(id)));
        assert_eq!(guest.pending_joins().count(), 0);
        assert_eq!(guest.share(id).unwrap().role_of(device), Some(Role::Viewer));
        assert!(owner.share(id).unwrap().pending.is_empty());
        assert_eq!(marks(&guest, D1), vec![(1, 1)]);
    }

    #[test]
    fn a_refused_joiner_is_told() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let mut share = shared_doc(&owner);
        share.approve_entry = true;
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        connect_both(&mut owner, &mut guest);
        deliver(guest.join(invite), &mut guest, &mut owner);
        let device = guest.profile.id();
        let actions = owner.edit_share(id, ShareEdit::Refuse(device));
        let events = deliver(actions, &mut owner, &mut guest);
        assert!(events.contains(&DriverEvent::JoinFailed {
            share: id,
            reason: JoinFailure::Refused
        }));
        assert_eq!(guest.pending_joins().count(), 0);
    }

    /// Leaving keeps the invitation, so the member can ask back in; the
    /// owner keeps what it ended, so it can open it again under a new key.
    #[test]
    fn a_member_asking_in_again_keeps_its_role_and_membership() {
        let (mut owner, mut guest, id) = joined_then_apart();
        let device = guest.profile.id();
        owner.edit_share(
            id,
            ShareEdit::SetRole {
                device,
                role: Role::Viewer,
            },
        );
        let before = owner.share(id).unwrap().member(device).cloned().unwrap();
        // The guest restarts with its join still pending, and asks again.
        let invite = owner.share(id).unwrap().invite();
        guest.shares.remove(&id);
        std::thread::sleep(Duration::from_millis(3));
        connect_both(&mut owner, &mut guest);
        deliver(guest.join(invite), &mut guest, &mut owner);
        let after = owner.share(id).unwrap().member(device).cloned().unwrap();
        assert_eq!(after, before);
        assert!(!guest.share(id).unwrap().can_edit(device));
    }

    #[test]
    fn coming_back_waits_as_any_newcomer_and_can_be_closed() {
        // Left while newcomers had to be let in: it waits too.
        let (mut owner, mut guest, id) = joined_then_apart();
        owner.shares.get_mut(&id).unwrap().share.approve_entry = true;
        connect_both(&mut owner, &mut guest);
        deliver(guest.leave_share(id), &mut guest, &mut owner);
        let events = deliver(guest.rejoin(id), &mut guest, &mut owner);
        assert!(events.contains(&DriverEvent::JoinPending(id)), "{events:?}");
        assert!(guest.share(id).is_none());

        // Left while entry was open, but it is not any more: it waits as
        // anyone would, unless the owner closed its way back.
        for closed in [false, true] {
            let (mut owner, mut guest, id) = joined_then_apart();
            connect_both(&mut owner, &mut guest);
            deliver(guest.leave_share(id), &mut guest, &mut owner);
            owner.shares.get_mut(&id).unwrap().share.approve_entry = true;
            if closed {
                assert!(!owner.forget_departed(id, guest.profile.id()).is_empty());
            }
            let events = deliver(guest.rejoin(id), &mut guest, &mut owner);
            assert!(guest.share(id).is_none());
            let waits = events.contains(&DriverEvent::JoinPending(id));
            assert_eq!(waits, !closed, "{events:?}");
        }

        // Thrown out after leaving: its way back closes too.
        let (mut owner, mut guest, id) = joined_then_apart();
        connect_both(&mut owner, &mut guest);
        deliver(guest.leave_share(id), &mut guest, &mut owner);
        owner.expel(id, guest.profile.id());
        assert!(owner.share(id).unwrap().was_expelled(guest.profile.id()));
        deliver(guest.rejoin(id), &mut guest, &mut owner);
        assert!(guest.share(id).is_none());
    }

    #[test]
    fn recent_shares_can_be_rejoined_or_reopened() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let share = shared_doc(&owner);
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        connect_both(&mut owner, &mut guest);
        deliver(guest.join(invite), &mut guest, &mut owner);

        let events = deliver(guest.leave_share(id), &mut guest, &mut owner);
        assert!(events.contains(&DriverEvent::ShareRemoved {
            share: id,
            reason: Removal::Left
        }));
        assert_eq!(guest.left().count(), 1);
        let book = ShareBook::decode(&guest.book().encode()).unwrap();
        assert_eq!(book.left.len(), 1);
        let events = deliver(guest.rejoin(id), &mut guest, &mut owner);
        assert!(events.contains(&DriverEvent::ShareChanged(id)));
        assert!(guest.share(id).is_some());
        assert_eq!(guest.left().count(), 0);

        owner.end_share(id);
        let ended = owner.ended().next().unwrap().clone();
        assert_eq!(ended.title, "Física");
        let (fresh, _) = owner.reopen(id).unwrap();
        assert_ne!(fresh, id);
        assert_eq!(owner.share(fresh).unwrap().docs, vec![D1]);
        assert!(owner.ended().next().is_none());
    }

    /// A guest that dropped its copy announces a tombstone. Letting it
    /// edit must not let that tombstone delete the owner's document:
    /// deletions are an organizer's call.
    #[test]
    fn guest_tombstones_need_an_organizer_not_an_editor() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let mut share = shared_doc(&owner);
        share.entry_role = Role::Viewer;
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        connect_both(&mut owner, &mut guest);
        deliver(guest.join(invite), &mut guest, &mut owner);
        assert_eq!(
            owner.with_replica(|r| r.blob_text(BlobKind::Item, 1)),
            Some("doc-1".to_owned())
        );

        guest.with_replica(|r| r.put_gone(BlobKind::Item, 1, 9));
        deliver(guest.announce().unwrap(), &mut guest, &mut owner);
        assert_eq!(
            owner.with_replica(|r| r.blob_text(BlobKind::Item, 1)),
            Some("doc-1".to_owned())
        );

        let device = guest.profile.id();
        let actions = owner.edit_share(
            id,
            ShareEdit::SetRole {
                device,
                role: Role::Editor,
            },
        );
        deliver(actions, &mut owner, &mut guest);
        deliver(guest.announce().unwrap(), &mut guest, &mut owner);
        deliver(owner.announce().unwrap(), &mut owner, &mut guest);
        assert_eq!(
            owner.with_replica(|r| r.blob_text(BlobKind::Item, 1)),
            Some("doc-1".to_owned()),
            "an editor still cannot delete for everyone"
        );

        let actions = owner.edit_share(
            id,
            ShareEdit::SetRole {
                device,
                role: Role::CoOrganizer,
            },
        );
        deliver(actions, &mut owner, &mut guest);
        deliver(guest.announce().unwrap(), &mut guest, &mut owner);
        assert!(
            owner.with_replica(|r| r
                .blobs
                .get(&BlobKey {
                    kind: BlobKind::Item,
                    id: [1; 16]
                })
                .map(|b| b.head.gone))
                == Some(true),
            "once an organizer, the tombstone lands"
        );
    }
    /// After one round each side knows what the other holds, so a local
    /// edit travels on its own without another inventory exchange, and
    /// nothing is resent once it landed.
    #[test]
    fn a_local_edit_is_pushed_right_away_and_only_once() {
        let mut a = driver("a");
        let mut b = driver("b");
        let a_dev = TrustedDevice::new(*a.profile.key().public(), "a", 1);
        let b_dev = TrustedDevice::new(*b.profile.key().public(), "b", 1);
        a.profile_mut().trust(b_dev);
        b.profile_mut().trust(a_dev);
        mark(&a, D1, 1, 1);
        connect_both(&mut a, &mut b);
        deliver(a.announce().unwrap(), &mut a, &mut b);
        deliver(b.announce().unwrap(), &mut b, &mut a);
        assert_eq!(marks(&b, D1), vec![(1, 1)]);

        mark(&a, D1, 1, 2);
        let actions = a.push_doc(D1).unwrap();
        assert_eq!(
            actions
                .iter()
                .filter(|x| matches!(x, Action::Send { .. }))
                .count(),
            1
        );
        deliver(actions, &mut a, &mut b);
        assert_eq!(marks(&b, D1), vec![(1, 1), (1, 2)]);

        let again = a.push_doc(D1).unwrap();
        assert!(again.iter().all(|x| !matches!(x, Action::Send { .. })));
    }
}
