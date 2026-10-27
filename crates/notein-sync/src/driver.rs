//! Policy on top of the transport: who to connect to, who to trust,
//! how to pair, which shares to serve, and when to run an exchange
//! round.
//!
//! The driver is a state machine. The app feeds it [`NetEvent`]s and
//! clock ticks and carries out the [`Action`]s it returns, so it can
//! be tested without a network and lives on whatever thread the app
//! prefers.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::engine::{Message, SyncEngine};
use crate::error::SyncError;
use crate::identity::{DeviceId, DeviceProfile, PairingCode, TrustedDevice};
use crate::net::{NetEvent, PeerInfo};
use crate::relay::{RecordKind, RelayAddr, RelayMessage};
use crate::replica::{BlobKey, BlobKind, DocId, Replica, Scope, Version};
use crate::share::{Invite, Member, Share, ShareId, ShareMessage, ShareMode, proof_is_valid};

/// How long a pairing offer stays valid.
pub const OFFER_TTL: Duration = Duration::from_secs(5 * 60);
/// How often [`SyncDriver::tick`] should be called.
pub const TICK: Duration = Duration::from_secs(15);

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
        name: String,
        proof: Vec<u8>,
    },
    /// The owner's answer, sealed with the share key.
    JoinReply {
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
    fn to_bytes(&self) -> Vec<u8> {
        bincode::encode_to_vec(self, bincode::config::standard()).unwrap_or_default()
    }

    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        bincode::decode_from_slice(bytes, bincode::config::standard())
            .ok()
            .map(|(payload, _)| payload)
    }
}

/// What the owner says to a joiner.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
enum JoinAnswer {
    Welcome(Box<Share>),
    Refused,
}

/// The pairing handshake, carried over an already authenticated
/// channel so both sides know exactly which key they are trusting.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub enum PairMessage {
    Request { code: u32, name: String },
    Accept { name: String },
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
}

impl PairingOffer {
    /// `notein://pair?pk=<hex>&name=<hex>&code=<6 digits>`.
    #[must_use]
    pub fn to_link(&self) -> String {
        format!(
            "{LINK_SCHEME}pk={}&name={}&code={}",
            hex::encode(self.public),
            hex::encode(self.name.as_bytes()),
            self.code
        )
    }

    /// Reverse of [`Self::to_link`].
    #[must_use]
    pub fn parse_link(link: &str) -> Option<Self> {
        let query = link.trim().strip_prefix(LINK_SCHEME)?;
        let mut public = None;
        let mut name = None;
        let mut code = None;
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=')?;
            match key {
                "pk" => {
                    let bytes = hex::decode(value).ok()?;
                    public = Some(<[u8; 32]>::try_from(bytes).ok()?);
                }
                "name" => name = Some(String::from_utf8(hex::decode(value).ok()?).ok()?),
                "code" => code = PairingCode::parse(value),
                _ => {}
            }
        }
        let public = public?;
        Some(Self {
            device: DeviceId::of_public_key(&public),
            public,
            name: name?,
            code: code?,
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    /// The relay came within reach or went away.
    RelayOnline(bool),
    /// Ephemeral state of a document from another device, see
    /// [`SyncDriver::live`]. Opaque to the driver.
    Live {
        from: DeviceId,
        name: String,
        doc: DocId,
        bytes: Vec<u8>,
    },
}

/// What the app must carry out.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Action {
    Send {
        device: DeviceId,
        bytes: Vec<u8>,
    },
    Connect {
        addr: SocketAddr,
        public: [u8; 32],
    },
    /// Dial a `host:port` that still needs resolving.
    ConnectHost {
        host: String,
        public: [u8; 32],
    },
    Event(DriverEvent),
}

#[derive(Clone, Debug)]
struct PairIntent {
    device: DeviceId,
    public: [u8; 32],
    code: PairingCode,
    name: String,
    since: Instant,
}

#[derive(Clone, Debug)]
struct JoinIntent {
    invite: Invite,
    since: Instant,
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
                BlobKind::Item | BlobKind::Snapshot | BlobKind::Asset => share.can_edit(me),
                BlobKind::Link => share.can_link(me),
                BlobKind::Folder | BlobKind::Tag | BlobKind::Workspace => false,
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

/// The relay this device meets others at.
#[derive(Debug)]
struct RelayLink {
    addr: RelayAddr,
    device: DeviceId,
    connected: bool,
    published: HashMap<ShareId, Published>,
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
    offer: Option<(PairingCode, Instant)>,
    intent: Option<PairIntent>,
    joins: HashMap<ShareId, JoinIntent>,
    relay: Option<RelayLink>,
    /// Public keys of every device heard of, so a message can be
    /// routed through the relay when no direct channel exists.
    known_keys: HashMap<DeviceId, [u8; 32]>,
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
    pub fn new(profile: DeviceProfile, replica: R, shares: Vec<Share>) -> Self {
        let replica = Arc::new(Mutex::new(replica));
        let engine = SyncEngine::new(Arc::clone(&replica), Scope::All);
        let shares = shares
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
            joins: HashMap::new(),
            relay: None,
            known_keys: HashMap::new(),
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

    /// This device as a share member.
    #[must_use]
    pub fn me(&self) -> Member {
        Member {
            device: self.profile.id(),
            public: *self.profile.key().public(),
            name: self.profile.name().to_owned(),
            joined_at: now_ms(),
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

    /// Start accepting one pairing: the returned offer is what to show.
    pub fn begin_pairing(&mut self) -> PairingOffer {
        let code = PairingCode::random();
        self.offer = Some((code, Instant::now()));
        PairingOffer {
            device: self.profile.id(),
            public: *self.profile.key().public(),
            name: self.profile.name().to_owned(),
            code,
        }
    }

    pub fn cancel_pairing(&mut self) {
        self.offer = None;
        self.intent = None;
    }

    /// Pair with the device described by `offer` (scanned, pasted or
    /// picked from the discovered list with its code typed in).
    pub fn pair_with(&mut self, offer: &PairingOffer) -> Vec<Action> {
        self.intent = Some(PairIntent {
            device: offer.device,
            public: offer.public,
            code: offer.code,
            name: offer.name.clone(),
            since: Instant::now(),
        });
        if self.connected.contains_key(&offer.device) {
            return vec![self.pair_request(offer.device)];
        }
        self.connect_to(offer.device).into_iter().collect()
    }

    fn pair_request(&self, device: DeviceId) -> Action {
        let code = self.intent.as_ref().map_or(0, |i| i.code.value());
        send(
            device,
            &Payload::Pair(PairMessage::Request {
                code,
                name: self.profile.name().to_owned(),
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
        let (code, since) = self.offer?;
        if since.elapsed() > OFFER_TTL {
            return None;
        }
        Some(PairingOffer {
            device: self.profile.id(),
            public: *self.profile.key().public(),
            name: self.profile.name().to_owned(),
            code,
        })
    }

    // ---------- Shares ------------------------------------------------

    /// Start owning a share. `share.owner` must be this device.
    pub fn create_share(&mut self, share: Share) -> Vec<Action> {
        let id = share.id;
        let engine = SyncEngine::new(Arc::clone(&self.replica), share.scope());
        self.shares.insert(id, ShareLink { share, engine });
        let mut out = self.relay_register(id);
        out.push(Action::Event(DriverEvent::ShareChanged(id)));
        out
    }

    /// Owner-side change of scope, policy or title; members learn it
    /// on their next message.
    pub fn update_share(&mut self, id: ShareId, change: impl FnOnce(&mut Share)) -> Vec<Action> {
        let Some(link) = self.shares.get_mut(&id) else {
            return Vec::new();
        };
        change(&mut link.share);
        link.share.updated_at = now_ms();
        link.engine.set_scope(link.share.scope());
        let mut out = self.broadcast_meta(id);
        out.extend(self.relay_register(id));
        out.push(Action::Event(DriverEvent::ShareChanged(id)));
        out
    }

    /// Owner throws a member out.
    pub fn expel(&mut self, id: ShareId, device: DeviceId) -> Vec<Action> {
        let Some(link) = self.shares.get_mut(&id) else {
            return Vec::new();
        };
        if !link.share.is_owner(self.profile.id()) || !link.share.expel(device) {
            return Vec::new();
        }
        link.share.updated_at = now_ms();
        link.engine.forget_peer(device);
        let mut out = vec![send(
            device,
            &Payload::Share {
                share: id,
                sealed: ShareMessage::Expelled.seal(&link.share.key),
            },
        )];
        out.extend(self.broadcast_meta(id));
        out.extend(self.relay_register(id));
        out.push(Action::Event(DriverEvent::ShareChanged(id)));
        out
    }

    /// Owner closes the share for everyone.
    pub fn end_share(&mut self, id: ShareId) -> Vec<Action> {
        let Some(link) = self.shares.remove(&id) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        out.extend(self.relay_send(&RelayMessage::Unregister { share: id }));
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
    pub fn leave_share(&mut self, id: ShareId) -> Vec<Action> {
        let Some(link) = self.shares.remove(&id) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        out.extend(self.route(
            link.share.owner.device,
            &Payload::Share {
                share: id,
                sealed: ShareMessage::Left.seal(&link.share.key),
            },
        ));
        out.push(Action::Event(DriverEvent::ShareRemoved {
            share: id,
            reason: Removal::Left,
        }));
        out
    }

    /// Accept an invitation: connect to the owner and ask in.
    pub fn join(&mut self, invite: Invite) -> Vec<Action> {
        let mut out = Vec::new();
        if self.relay.is_none()
            && let Some(server) = invite.server.as_deref()
            && let Ok(addr) = server.parse::<RelayAddr>()
        {
            out.extend(self.set_relay(Some(addr)));
        }
        out.extend(self.join_inner(invite));
        out
    }

    fn join_inner(&mut self, invite: Invite) -> Vec<Action> {
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
            },
        );
        if let Some(request) = self.join_request(share) {
            return vec![request];
        }
        self.connect_to(owner).into_iter().collect()
    }

    fn join_request(&self, share: ShareId) -> Option<Action> {
        let intent = self.joins.get(&share)?;
        self.route(
            intent.invite.owner.device,
            &Payload::Join {
                share,
                name: self.profile.name().to_owned(),
                proof: intent.invite.proof(),
            },
        )
    }

    /// Deliver `payload` to `device`: straight down an open channel,
    /// or through the relay when its key is known.
    fn route(&self, device: DeviceId, payload: &Payload) -> Option<Action> {
        if self.connected.contains_key(&device) {
            return Some(send(device, payload));
        }
        let public = *self.known_keys.get(&device)?;
        self.relay_send(&RelayMessage::Forward {
            to: public,
            bytes: payload.to_bytes(),
        })
    }

    /// Owner's current view sent to every connected member.
    fn broadcast_meta(&self, id: ShareId) -> Vec<Action> {
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

    /// Connect to `device` if we know where it is.
    fn connect_to(&self, device: DeviceId) -> Option<Action> {
        self.discovered.get(&device).map(|peer| Action::Connect {
            addr: peer.addr,
            public: peer.public,
        })
    }

    // ---------- Events ------------------------------------------------

    /// React to the transport.
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
            NetEvent::ConnectFailed { addr, .. } => self.on_connect_failed(addr, &mut out),
            NetEvent::Stopped(_) => self.on_stopped(&mut out),
        }
        Ok(out)
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

    fn on_connected(
        &mut self,
        peer: PeerInfo,
        out: &mut Vec<Action>,
    ) -> Result<(), SyncError<R::Error>> {
        self.refresh_name(&peer, out);
        let device = peer.device;
        self.known_keys.insert(device, peer.public);
        self.connected.insert(device, peer);
        if let Some(relay) = self.relay.as_mut()
            && relay.device == device
        {
            relay.connected = true;
            out.extend(self.relay_hello()?);
            return Ok(());
        }
        if self.is_related(device) {
            out.push(Action::Event(DriverEvent::PeerOnline {
                device,
                online: true,
            }));
        }
        if self.profile.is_trusted(device) {
            out.extend(self.round_for(device)?);
        }
        for share in self.shares_with(device) {
            out.extend(self.share_round_for(share, device)?);
        }
        if self.intent.as_ref().is_some_and(|i| i.device == device) {
            out.push(self.pair_request(device));
        }
        for share in self.joins_to(device) {
            out.extend(self.join_request(share));
        }
        Ok(())
    }

    fn on_disconnected(&mut self, device: DeviceId, out: &mut Vec<Action>) {
        self.connected.remove(&device);
        self.engine.forget_peer(device);
        if let Some(relay) = self.relay.as_mut()
            && relay.device == device
        {
            relay.connected = false;
            relay.published.clear();
            out.push(Action::Event(DriverEvent::RelayOnline(false)));
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

    fn on_message(
        &mut self,
        from: DeviceId,
        bytes: &[u8],
        out: &mut Vec<Action>,
    ) -> Result<(), SyncError<R::Error>> {
        let Some(payload) = Payload::from_bytes(bytes) else {
            return Ok(());
        };
        match payload {
            Payload::Sync(bytes) => {
                if self.profile.is_trusted(from) {
                    self.on_sync(from, &bytes, out)?;
                }
            }
            Payload::Pair(message) => self.on_pair(from, message, out),
            Payload::Share { share, sealed } => self.on_share(from, share, &sealed, out)?,
            Payload::Join { share, name, proof } => {
                self.on_join(from, share, name, &proof, out)?;
            }
            Payload::JoinReply { share, sealed } => {
                self.on_join_reply(from, share, &sealed, out)?;
            }
            Payload::Relay(bytes) => self.on_relay(from, &bytes, out)?,
            Payload::Live { doc, bytes } => {
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

    fn on_share(
        &mut self,
        from: DeviceId,
        id: ShareId,
        sealed: &[u8],
        out: &mut Vec<Action>,
    ) -> Result<(), SyncError<R::Error>> {
        let name = self.peer_name(from);
        let Some(link) = self.shares.get_mut(&id) else {
            return Ok(());
        };
        if !link.share.is_member(from) {
            return Ok(());
        }
        let Some(message) = ShareMessage::open(&link.share.key, sealed) else {
            return Ok(());
        };
        let me = self.profile.id();
        match message {
            ShareMessage::Live { doc, bytes } => {
                if link.share.docs.contains(&doc) {
                    out.push(Action::Event(DriverEvent::Live {
                        from,
                        name,
                        doc,
                        bytes,
                    }));
                }
            }
            ShareMessage::Sync(bytes) => {
                let message = Message::from_bytes(&bytes)?;
                let Some(message) = permitted(&link.share, from, message) else {
                    return Ok(());
                };
                let handled = link.engine.handle(from, message)?;
                for reply in &handled.replies {
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
            }
            ShareMessage::Meta(share) => {
                if link.share.is_owner(from)
                    && !link.share.is_owner(me)
                    && share.id == id
                    && share.updated_at >= link.share.updated_at
                {
                    let key = link.share.key.clone();
                    let cursor = link.share.relay_cursor;
                    link.share = Share {
                        key,
                        relay_cursor: cursor,
                        ..*share
                    };
                    link.engine.set_scope(link.share.scope());
                    out.push(Action::Event(DriverEvent::ShareChanged(id)));
                    if !link.share.is_member(me) {
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
            ShareMessage::Left => {
                if link.share.is_owner(me) && link.share.expel(from) {
                    link.share.updated_at = now_ms();
                    link.engine.forget_peer(from);
                    out.extend(self.broadcast_meta(id));
                    out.push(Action::Event(DriverEvent::ShareChanged(id)));
                }
            }
        }
        Ok(())
    }

    /// The owner closed the share for this device: drop it and say
    /// why. Returns whether anything was dropped.
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

    fn on_join(
        &mut self,
        from: DeviceId,
        id: ShareId,
        name: String,
        proof: &[u8],
        out: &mut Vec<Action>,
    ) -> Result<(), SyncError<R::Error>> {
        let Some(public) = self.known_keys.get(&from).copied() else {
            return Ok(());
        };
        let me = self.profile.id();
        let answer = match self.shares.get_mut(&id) {
            Some(link) if link.share.is_owner(me) && proof_is_valid(&link.share, proof) => {
                link.share.admit(Member {
                    device: from,
                    public,
                    name,
                    joined_at: now_ms(),
                });
                link.share.updated_at = now_ms();
                let welcome = JoinAnswer::Welcome(Box::new(link.share.for_member()));
                let sealed = seal_answer(&link.share, &welcome);
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
        let answer = intent.invite.key.open(sealed).ok().and_then(|plain| {
            bincode::decode_from_slice(&plain, bincode::config::standard())
                .ok()
                .map(|(answer, _): (JoinAnswer, usize)| answer)
        });
        let intent = self.joins.remove(&id).map(|j| j.invite);
        match (answer, intent) {
            (Some(JoinAnswer::Welcome(share)), Some(invite)) if share.id == id => {
                let share = Share {
                    key: invite.key,
                    ..*share
                };
                for member in share.participants() {
                    self.known_keys.insert(member.device, member.public);
                }
                let engine = SyncEngine::new(Arc::clone(&self.replica), share.scope());
                self.shares.insert(id, ShareLink { share, engine });
                out.extend(self.relay_send(&RelayMessage::Subscribe {
                    share: id,
                    after: 0,
                }));
                out.push(Action::Event(DriverEvent::ShareChanged(id)));
                out.push(Action::Event(DriverEvent::PeerOnline {
                    device: from,
                    online: true,
                }));
                out.extend(self.share_round_for(id, from)?);
            }
            _ => out.push(Action::Event(DriverEvent::JoinFailed {
                share: id,
                reason: JoinFailure::Refused,
            })),
        }
        Ok(())
    }

    fn on_pair(&mut self, from: DeviceId, message: PairMessage, out: &mut Vec<Action>) {
        let Some(peer) = self.connected.get(&from).cloned() else {
            return;
        };
        match message {
            PairMessage::Request { code, name } => {
                let accepted = self
                    .offer
                    .take_if(|(_, since)| since.elapsed() <= OFFER_TTL)
                    .is_some_and(|(expected, _)| expected.value() == code);
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
                    out.push(Action::Event(DriverEvent::PeerOnline {
                        device: from,
                        online: true,
                    }));
                    if let Ok(round) = self.round_for(from) {
                        out.extend(round);
                    }
                } else {
                    self.offer = None;
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
                out.push(Action::Event(DriverEvent::PeerOnline {
                    device: from,
                    online: true,
                }));
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

    /// Same as [`Self::round_for`] inside one share.
    fn share_round_for(
        &self,
        id: ShareId,
        device: DeviceId,
    ) -> Result<Vec<Action>, SyncError<R::Error>> {
        let Some(link) = self.shares.get(&id) else {
            return Ok(Vec::new());
        };
        let mut messages = link.engine.push_for(device)?;
        if !link.share.can_edit(self.profile.id()) {
            // Only read here: announce so the others send, never push.
            messages.clear();
        }
        messages.push(link.engine.inventory());
        let mut out = Vec::with_capacity(messages.len());
        for message in messages {
            out.extend(self.route(
                device,
                &Payload::Share {
                    share: id,
                    sealed: ShareMessage::Sync(message.to_bytes()?).seal(&link.share.key),
                },
            ));
        }
        Ok(out)
    }

    /// A round towards every online trusted device and share member.
    /// Call after local edits land and on every [`TICK`].
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
    pub fn live(&self, doc: DocId, bytes: &[u8]) -> Vec<Action> {
        let mut out: Vec<Action> = self
            .online()
            .map(|p| {
                send(
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
                if member.device == me || !self.connected.contains_key(&member.device) {
                    continue;
                }
                out.push(send(
                    member.device,
                    &Payload::Share {
                        share: link.share.id,
                        sealed: sealed.clone(),
                    },
                ));
            }
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
    pub fn tick(&mut self) -> Result<Vec<Action>, SyncError<R::Error>> {
        let mut out = Vec::new();
        if self
            .offer
            .is_some_and(|(_, since)| since.elapsed() > OFFER_TTL)
        {
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
        out.extend(self.relay_reconnect());
        let pending: Vec<ShareId> = self.joins.keys().copied().collect();
        for share in pending {
            out.extend(self.join_request(share));
        }
        // Subscriptions are cheap to repeat and heal a refused one once
        // the owner's registration has landed.
        let held: Vec<(ShareId, u64)> = self
            .shares
            .values()
            .map(|l| (l.share.id, l.share.relay_cursor))
            .collect();
        for (share, after) in held {
            out.extend(self.relay_send(&RelayMessage::Subscribe { share, after }));
        }
        out.extend(self.announce()?);
        Ok(out)
    }

    /// Shares we are still waiting to be let into.
    pub fn pending_joins(&self) -> impl Iterator<Item = &Invite> {
        self.joins.values().map(|j| &j.invite)
    }
}

// ---------- Relay ---------------------------------------------------

/// Wire bytes of a relay message, as the transport carries them.
#[must_use]
pub fn relay_payload(message: &RelayMessage) -> Vec<u8> {
    Payload::Relay(message.to_bytes()).to_bytes()
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
        | Payload::Live { .. } => None,
    }
}

impl<R: Replica> SyncDriver<R> {
    /// Use (or drop) a relay. Connecting happens on the next tick or
    /// right away when the relay is already known.
    pub fn set_relay(&mut self, addr: Option<RelayAddr>) -> Vec<Action> {
        self.relay = addr.map(|addr| RelayLink {
            device: addr.device(),
            addr,
            connected: false,
            published: HashMap::new(),
        });
        self.relay_reconnect().into_iter().collect()
    }

    #[must_use]
    pub fn relay(&self) -> Option<&RelayAddr> {
        self.relay.as_ref().map(|r| &r.addr)
    }

    #[must_use]
    pub fn relay_online(&self) -> bool {
        self.relay.as_ref().is_some_and(|r| r.connected)
    }

    /// The invitation for a share we own, pointing at our relay.
    #[must_use]
    pub fn invite_for(&self, id: ShareId) -> Option<Invite> {
        let share = self.share(id)?;
        let mut invite = share.invite();
        invite.server = self.relay().map(ToString::to_string);
        Some(invite)
    }

    fn relay_reconnect(&self) -> Option<Action> {
        let relay = self.relay.as_ref()?;
        if relay.connected || self.connected.contains_key(&relay.device) {
            return None;
        }
        Some(Action::ConnectHost {
            host: relay.addr.host.clone(),
            public: relay.addr.public,
        })
    }

    fn relay_send(&self, message: &RelayMessage) -> Option<Action> {
        let relay = self.relay.as_ref()?;
        relay.connected.then(|| Action::Send {
            device: relay.device,
            bytes: relay_payload(message),
        })
    }

    /// Owner tells the relay who may use the share's mailbox.
    fn relay_register(&self, id: ShareId) -> Vec<Action> {
        let Some(share) = self.share(id) else {
            return Vec::new();
        };
        if !share.is_owner(self.profile.id()) {
            return Vec::new();
        }
        self.relay_send(&RelayMessage::Register {
            share: id,
            members: share.members.iter().map(|m| m.public).collect(),
            retain: share.mode == ShareMode::Permanent,
        })
        .into_iter()
        .collect()
    }

    /// Just connected to the relay: register what we own, subscribe to
    /// everything, hand in what the relay has not seen.
    fn relay_hello(&mut self) -> Result<Vec<Action>, SyncError<R::Error>> {
        let mut out = vec![Action::Event(DriverEvent::RelayOnline(true))];
        let ids: Vec<ShareId> = self.shares.keys().copied().collect();
        for id in ids {
            out.extend(self.relay_register(id));
            if let Some(share) = self.share(id)
                && let Some(action) = self.relay_send(&RelayMessage::Subscribe {
                    share: id,
                    after: share.relay_cursor,
                })
            {
                out.push(action);
            }
        }
        let pending: Vec<ShareId> = self.joins.keys().copied().collect();
        for share in pending {
            out.extend(self.join_request(share));
        }
        out.extend(self.relay_publish()?);
        Ok(out)
    }

    /// Hand the relay whatever changed since the last time.
    fn relay_publish(&mut self) -> Result<Vec<Action>, SyncError<R::Error>> {
        let mut out = Vec::new();
        if !self.relay_online() {
            return Ok(out);
        }
        let me = self.profile.id();
        let ids: Vec<ShareId> = self.shares.keys().copied().collect();
        for id in ids {
            let Some(link) = self.shares.get(&id) else {
                continue;
            };
            let share = link.share.clone();
            let Some(relay) = self.relay.as_mut() else {
                break;
            };
            let published = relay.published.entry(id).or_default();
            let mut records = if share.can_edit(me) {
                published.doc_records(&self.replica, &share.docs)?
            } else {
                Vec::new()
            };
            records.extend(published.blob_records(&self.replica, &share, me));
            for (kind, message) in records {
                let sealed = ShareMessage::Sync(message.to_bytes()?).seal(&share.key);
                out.push(Action::Send {
                    device: relay.device,
                    bytes: relay_payload(&RelayMessage::Publish {
                        share: id,
                        kind,
                        sealed,
                    }),
                });
            }
            if share.is_owner(me) && published.meta_at < share.updated_at {
                published.meta_at = share.updated_at;
                out.push(Action::Send {
                    device: relay.device,
                    bytes: relay_payload(&RelayMessage::Publish {
                        share: id,
                        kind: RecordKind::Meta,
                        sealed: ShareMessage::Meta(Box::new(share.for_member())).seal(&share.key),
                    }),
                });
            }
        }
        Ok(out)
    }

    fn on_relay(
        &mut self,
        from: DeviceId,
        bytes: &[u8],
        out: &mut Vec<Action>,
    ) -> Result<(), SyncError<R::Error>> {
        if self.relay.as_ref().is_none_or(|r| r.device != from) {
            return Ok(());
        }
        let Some(message) = RelayMessage::from_bytes(bytes) else {
            return Ok(());
        };
        match message {
            RelayMessage::Records { share, records } => {
                let mut touched = false;
                for record in records {
                    touched |= self.apply_record(share, &record, out)?;
                }
                if touched {
                    out.push(Action::Event(DriverEvent::ShareChanged(share)));
                }
            }
            RelayMessage::Gone { share } => {
                let me = self.profile.id();
                if self.share(share).is_some_and(|s| s.is_owner(me)) {
                    out.extend(self.relay_register(share));
                } else if self.shares.remove(&share).is_some() {
                    out.push(Action::Event(DriverEvent::ShareRemoved {
                        share,
                        reason: Removal::Ended,
                    }));
                }
            }
            RelayMessage::Registered { share } => {
                if let Some(s) = self.share(share)
                    && let Some(action) = self.relay_send(&RelayMessage::Subscribe {
                        share,
                        after: s.relay_cursor,
                    })
                {
                    out.push(action);
                }
            }
            RelayMessage::Forwarded {
                from: public,
                bytes,
            } => {
                let device = DeviceId::of_public_key(&public);
                self.known_keys.insert(device, public);
                match Payload::from_bytes(&bytes) {
                    Some(Payload::Join { share, name, proof }) => {
                        self.on_join(device, share, name, &proof, out)?;
                    }
                    Some(Payload::JoinReply { share, sealed }) => {
                        self.on_join_reply(device, share, &sealed, out)?;
                    }
                    Some(Payload::Share { share, sealed }) => {
                        self.on_share(device, share, &sealed, out)?;
                    }
                    Some(Payload::Live { doc, bytes }) => {
                        // Relayed ephemera reach members that only meet here.
                        if self.profile.is_trusted(device) {
                            out.push(Action::Event(DriverEvent::Live {
                                from: device,
                                name: self.peer_name(device),
                                doc,
                                bytes,
                            }));
                        }
                    }
                    Some(Payload::Sync(_) | Payload::Pair(_) | Payload::Relay(_)) | None => {}
                }
            }
            RelayMessage::Refused { .. }
            | RelayMessage::Forward { .. }
            | RelayMessage::Register { .. }
            | RelayMessage::Unregister { .. }
            | RelayMessage::Subscribe { .. }
            | RelayMessage::Publish { .. } => {}
        }
        Ok(())
    }

    /// Apply one mailbox record; returns whether the share changed.
    fn apply_record(
        &mut self,
        id: ShareId,
        record: &crate::relay::Record,
        out: &mut Vec<Action>,
    ) -> Result<bool, SyncError<R::Error>> {
        let me = self.profile.id();
        let Some(link) = self.shares.get_mut(&id) else {
            return Ok(false);
        };
        if record.seq <= link.share.relay_cursor {
            return Ok(false);
        }
        link.share.relay_cursor = record.seq;
        if record.from == me || !link.share.is_member(record.from) {
            return Ok(true);
        }
        let Some(message) = ShareMessage::open(&link.share.key, &record.sealed) else {
            return Ok(true);
        };
        match message {
            // Mailboxes never hold ephemera; nothing to apply.
            ShareMessage::Live { .. } => {}
            ShareMessage::Sync(bytes) => {
                let message = Message::from_bytes(&bytes)?;
                if let Some(message) = permitted(&link.share, record.from, message) {
                    let handled = link.engine.handle(record.from, message)?;
                    if !handled.imported.is_empty() {
                        out.push(Action::Event(DriverEvent::Imported(handled.imported)));
                    }
                    if !handled.merged.is_empty() {
                        out.push(Action::Event(DriverEvent::Merged(handled.merged)));
                    }
                }
            }
            ShareMessage::Meta(share) => {
                if link.share.is_owner(record.from)
                    && !link.share.is_owner(me)
                    && share.id == id
                    && share.updated_at >= link.share.updated_at
                {
                    let key = link.share.key.clone();
                    let cursor = link.share.relay_cursor;
                    link.share = Share {
                        key,
                        relay_cursor: cursor,
                        ..*share
                    };
                    link.engine.set_scope(link.share.scope());
                    if !link.share.is_member(me) {
                        self.shares.remove(&id);
                        out.push(Action::Event(DriverEvent::ShareRemoved {
                            share: id,
                            reason: Removal::Expelled,
                        }));
                        return Ok(false);
                    }
                }
            }
            ShareMessage::Ended => {
                if self.removed_by_owner(record.from, id, Removal::Ended, out) {
                    return Ok(false);
                }
            }
            ShareMessage::Expelled => {
                if self.removed_by_owner(record.from, id, Removal::Expelled, out) {
                    return Ok(false);
                }
            }
            ShareMessage::Left => {
                if link.share.is_owner(me) && link.share.expel(record.from) {
                    link.share.updated_at = now_ms();
                    out.extend(self.broadcast_meta(id));
                    out.extend(self.relay_register(id));
                }
            }
        }
        Ok(true)
    }
}

/// Drop what `from` may not do inside `share`.
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
                    BlobKind::Item if blob.head.gone => share.can_delete(from),
                    BlobKind::Item | BlobKind::Snapshot | BlobKind::Asset => share.can_edit(from),
                    BlobKind::Link => share.can_link(from),
                    BlobKind::Folder | BlobKind::Tag | BlobKind::Workspace => false,
                })
                .collect();
            (!kept.is_empty()).then_some(Message::Blobs(kept))
        }
    }
}

fn seal_answer(share: &Share, answer: &JoinAnswer) -> Vec<u8> {
    let plain = bincode::encode_to_vec(answer, bincode::config::standard()).unwrap_or_default();
    share.key.seal(&plain).unwrap_or_default()
}

fn send(device: DeviceId, payload: &Payload) -> Action {
    Action::Send {
        device,
        bytes: payload.to_bytes(),
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
    use crate::replica::BlobKind;
    use crate::share::{ShareMode, SharePolicy, Who};
    use crate::testing::MemReplica;

    const D1: DocId = DocId([1; 16]);
    const D2: DocId = DocId([2; 16]);

    fn driver(name: &str) -> SyncDriver<MemReplica> {
        SyncDriver::new(
            DeviceProfile::generate(name),
            MemReplica::default(),
            Vec::new(),
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
                Action::Connect { .. } | Action::ConnectHost { .. } => {}
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
        let offer = a.begin_pairing();
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
        assert!(matches!(request[..], [Action::Send { .. }]));

        let events = deliver(request, &mut b, &mut a);
        assert!(a.profile.is_trusted(b.profile.id()));
        assert!(b.profile.is_trusted(a.profile.id()));
        assert_eq!(a.profile.trusted()[0].name, "PC");
        assert_eq!(b.profile.trusted()[0].name, "Tablet");
        assert!(events.iter().any(|e| matches!(e, DriverEvent::Paired(_))));
        assert!(b.pairing_target().is_none());
    }

    #[test]
    fn wrong_code_is_rejected() {
        let mut a = driver("Tablet");
        let mut b = driver("PC");
        let mut offer = a.begin_pairing();
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
        share.policy = SharePolicy::read_only();
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
    fn policy_updates_reach_members() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let share = shared_doc(&owner);
        let id = share.id;
        let invite = share.invite();
        owner.create_share(share);
        connect_both(&mut owner, &mut guest);
        deliver(guest.join(invite), &mut guest, &mut owner);
        assert!(guest.share(id).unwrap().can_edit(guest.profile.id()));

        let actions = owner.update_share(id, |s| s.policy = SharePolicy::read_only());
        deliver(actions, &mut owner, &mut guest);
        assert!(!guest.share(id).unwrap().can_edit(guest.profile.id()));
        assert_eq!(guest.share(id).unwrap().key, owner.share(id).unwrap().key);
    }

    /// A guest that dropped its copy announces a tombstone. Relaxing
    /// "read only" lets its edits through but must not let that
    /// tombstone delete the owner's document: deletions follow
    /// `delete_docs`, which stays owner-only by default.
    #[test]
    fn guest_tombstones_follow_the_delete_policy_not_the_edit_policy() {
        let mut owner = driver("Sergio");
        let mut guest = driver("Ana");
        let mut share = shared_doc(&owner);
        share.policy = SharePolicy::read_only();
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

        let actions = owner.update_share(id, |s| s.policy = SharePolicy::default());
        deliver(actions, &mut owner, &mut guest);
        deliver(guest.announce().unwrap(), &mut guest, &mut owner);
        deliver(owner.announce().unwrap(), &mut owner, &mut guest);
        assert_eq!(
            owner.with_replica(|r| r.blob_text(BlobKind::Item, 1)),
            Some("doc-1".to_owned()),
            "an editable share still keeps deletions owner-only"
        );

        let actions = owner.update_share(id, |s| s.policy.delete_docs = Who::Everyone);
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
            "once members may delete, the tombstone lands"
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
