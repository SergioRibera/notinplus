//! Policy on top of the transport: who to connect to, who to trust,
//! how to pair, and when to run an exchange round.
//!
//! The driver is a state machine. The app feeds it [`NetEvent`]s and
//! clock ticks and carries out the [`Action`]s it returns, so it can
//! be tested without a network and lives on whatever thread the app
//! prefers.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use crate::engine::{Message, SyncEngine};
use crate::error::SyncError;
use crate::identity::{DeviceId, DeviceProfile, PairingCode, TrustedDevice};
use crate::net::{NetEvent, PeerInfo};
use crate::replica::{BlobKey, DocId, Replica};

/// How long a pairing offer stays valid.
pub const OFFER_TTL: Duration = Duration::from_secs(5 * 60);
/// How often [`SyncDriver::tick`] should be called.
pub const TICK: Duration = Duration::from_secs(15);

const LINK_SCHEME: &str = "notein://pair?";

/// What travels inside a transport payload.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub enum Payload {
    Sync(Vec<u8>),
    Pair(PairMessage),
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

/// Something the app should react to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DriverEvent {
    /// A device joined the trusted set; the profile changed.
    Paired(TrustedDevice),
    PairFailed {
        device: DeviceId,
        reason: PairFailure,
    },
    /// A trusted device came online or went away.
    PeerOnline { device: DeviceId, online: bool },
    /// Documents that received remote content.
    Imported(Vec<DocId>),
    /// Blobs replaced by a newer remote copy.
    Merged(Vec<BlobKey>),
}

/// What the app must carry out.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Action {
    Send { device: DeviceId, bytes: Vec<u8> },
    Connect { addr: SocketAddr, public: [u8; 32] },
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

/// Owns the profile, the exchange engine and the view of the network.
#[derive(Debug)]
pub struct SyncDriver<R> {
    profile: DeviceProfile,
    engine: SyncEngine<R>,
    discovered: HashMap<DeviceId, PeerInfo>,
    connected: HashMap<DeviceId, PeerInfo>,
    offer: Option<(PairingCode, Instant)>,
    intent: Option<PairIntent>,
}

impl<R: Replica> SyncDriver<R> {
    #[must_use]
    pub fn new(profile: DeviceProfile, engine: SyncEngine<R>) -> Self {
        Self {
            profile,
            engine,
            discovered: HashMap::new(),
            connected: HashMap::new(),
            offer: None,
            intent: None,
        }
    }

    #[must_use]
    pub const fn profile(&self) -> &DeviceProfile {
        &self.profile
    }

    pub const fn profile_mut(&mut self) -> &mut DeviceProfile {
        &mut self.profile
    }

    #[must_use]
    pub const fn engine(&self) -> &SyncEngine<R> {
        &self.engine
    }

    pub const fn engine_mut(&mut self) -> &mut SyncEngine<R> {
        &mut self.engine
    }

    /// Devices seen on the network, trusted or not.
    #[must_use]
    pub fn discovered(&self) -> impl Iterator<Item = &PeerInfo> {
        self.discovered.values()
    }

    /// Trusted devices with an open channel.
    #[must_use]
    pub fn online(&self) -> impl Iterator<Item = &PeerInfo> {
        self.connected
            .values()
            .filter(|p| self.profile.is_trusted(p.device))
    }

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
        match self.discovered.get(&offer.device) {
            Some(peer) => vec![Action::Connect {
                addr: peer.addr,
                public: peer.public,
            }],
            None => Vec::new(),
        }
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

    /// React to the transport.
    pub fn on_event(&mut self, event: NetEvent) -> Result<Vec<Action>, SyncError<R::Error>> {
        let mut out = Vec::new();
        match event {
            NetEvent::Discovered(peer) => {
                let device = peer.device;
                let wants_pair = self.intent.as_ref().is_some_and(|i| i.device == device);
                let trusted = self.profile.is_trusted(device);
                let initiates = self.profile.id() < device;
                if !self.connected.contains_key(&device) && (wants_pair || (trusted && initiates)) {
                    out.push(Action::Connect {
                        addr: peer.addr,
                        public: peer.public,
                    });
                }
                self.discovered.insert(device, peer);
            }
            NetEvent::Connected(peer) => {
                let device = peer.device;
                self.connected.insert(device, peer);
                if self.profile.is_trusted(device) {
                    out.push(Action::Event(DriverEvent::PeerOnline {
                        device,
                        online: true,
                    }));
                    out.extend(self.round_for(device)?);
                }
                if self.intent.as_ref().is_some_and(|i| i.device == device) {
                    out.push(self.pair_request(device));
                }
            }
            NetEvent::Disconnected(device) => {
                self.connected.remove(&device);
                self.engine.forget_peer(device);
                if self.profile.is_trusted(device) {
                    out.push(Action::Event(DriverEvent::PeerOnline {
                        device,
                        online: false,
                    }));
                }
            }
            NetEvent::Message { from, bytes } => {
                let Some(payload) = Payload::from_bytes(&bytes) else {
                    return Ok(out);
                };
                match payload {
                    Payload::Sync(bytes) => {
                        if self.profile.is_trusted(from) {
                            self.on_sync(from, &bytes, &mut out)?;
                        }
                    }
                    Payload::Pair(message) => self.on_pair(from, message, &mut out),
                }
            }
            NetEvent::ConnectFailed { addr, .. } => {
                let device = self
                    .discovered
                    .values()
                    .find(|p| p.addr == addr)
                    .map(|p| p.device);
                if let Some(device) = device
                    && self.intent.as_ref().is_some_and(|i| i.device == device)
                {
                    self.intent = None;
                    out.push(Action::Event(DriverEvent::PairFailed {
                        device,
                        reason: PairFailure::Unreachable,
                    }));
                }
            }
            NetEvent::Stopped(_) => {
                for device in self.connected.keys().copied().collect::<Vec<_>>() {
                    self.engine.forget_peer(device);
                    if self.profile.is_trusted(device) {
                        out.push(Action::Event(DriverEvent::PeerOnline {
                            device,
                            online: false,
                        }));
                    }
                }
                self.connected.clear();
            }
        }
        Ok(out)
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

    /// A round towards every online trusted device. Call after local
    /// edits land and on every [`TICK`].
    pub fn announce(&mut self) -> Result<Vec<Action>, SyncError<R::Error>> {
        let devices: Vec<DeviceId> = self.online().map(|p| p.device).collect();
        let mut out = Vec::new();
        for device in devices {
            out.extend(self.round_for(device)?);
        }
        Ok(out)
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
        out.extend(self.announce()?);
        Ok(out)
    }

    /// Name shown to the device we are pairing with, if any.
    #[must_use]
    pub fn pairing_target(&self) -> Option<&str> {
        self.intent.as_ref().map(|i| i.name.as_str())
    }
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
    use crate::replica::Scope;
    use crate::testing::MemReplica;

    const D1: DocId = DocId([1; 16]);

    fn driver(name: &str) -> SyncDriver<MemReplica> {
        SyncDriver::new(
            DeviceProfile::generate(name),
            SyncEngine::new(MemReplica::default(), Scope::All),
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

    /// Deliver every `Send` in `actions` from `from` to `to`, looping
    /// until nothing is left to carry. Returns the events seen.
    fn deliver(
        actions: Vec<Action>,
        from: &mut SyncDriver<MemReplica>,
        to: &mut SyncDriver<MemReplica>,
    ) -> Vec<DriverEvent> {
        let mut events = Vec::new();
        let mut queue: Vec<(bool, Action)> = actions.into_iter().map(|a| (true, a)).collect();
        while let Some((forward, action)) = queue.pop() {
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
                Action::Connect { .. } => {}
                Action::Event(event) => events.push(event),
            }
        }
        events
    }

    #[test]
    fn pairing_over_a_connected_channel_trusts_both_sides() {
        let mut a = driver("Tablet");
        let mut b = driver("PC");
        let offer = a.begin_pairing();
        let parsed = PairingOffer::parse_link(&offer.to_link()).unwrap();
        assert_eq!(parsed, offer);

        // B sees A on the LAN, decides to pair, connects.
        assert!(b.pair_with(&parsed).is_empty());
        let actions = b.on_event(NetEvent::Discovered(info(&a, 1))).unwrap();
        assert!(matches!(actions[..], [Action::Connect { .. }]));

        // Both channels come up.
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
        let mut a = driver("Tablet");
        let mut b = driver("PC");
        a.engine_mut().replica_mut().mark(D1, 1, 1);
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
        a.engine_mut().replica_mut().mark(D1, 1, 1);
        b.engine_mut().replica_mut().mark(D1, 2, 1);

        // Only the lower id connects when both see each other.
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

        assert_eq!(a.engine().replica().marks(D1), vec![(1, 1), (2, 1)]);
        assert_eq!(b.engine().replica().marks(D1), vec![(1, 1), (2, 1)]);
        assert!(events.contains(&DriverEvent::Imported(vec![D1])));

        // A local edit on A reaches B on the next announcement.
        a.engine_mut().replica_mut().mark(D1, 1, 2);
        let round = a.announce().unwrap();
        deliver(round, &mut a, &mut b);
        assert_eq!(b.engine().replica().marks(D1), vec![(1, 1), (1, 2), (2, 1)]);

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
    fn link_parsing_rejects_garbage() {
        assert_eq!(PairingOffer::parse_link("notein://join?x=1"), None);
        assert_eq!(
            PairingOffer::parse_link("notein://pair?pk=zz&name=&code=123456"),
            None
        );
    }
}
