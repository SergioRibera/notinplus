//! Moving bytes between devices.
//!
//! [`lan::LanNode`] is the only transport today: it advertises the
//! device on the local network, finds the others, opens authenticated
//! channels and shuttles opaque payloads. It knows nothing about what
//! the payloads mean; that is [`crate::driver`]'s job.

use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::identity::{DeviceId, DeviceKey};

pub mod lan;

/// Application tag every beacon and probe carries, so unrelated
/// programs on the same network are ignored.
pub const APP_ID: &str = "notein";
/// Data port a device listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 7710;
/// Bumped whenever the frames exchanged on a channel change shape.
pub const WIRE_VERSION: u16 = 1;

/// Random for each run of the process: a device that comes back with
/// another one was restarted, whatever its channels looked like.
#[must_use]
pub fn session_id() -> u64 {
    static SESSION: OnceLock<u64> = OnceLock::new();
    *SESSION.get_or_init(rand::random)
}

/// What a device says about itself, both in LAN beacons and as the
/// first frame on every channel.
#[derive(wincode::SchemaWrite, wincode::SchemaRead, Clone, PartialEq, Eq, Debug)]
pub struct Beacon {
    pub device: DeviceId,
    pub public: [u8; 32],
    pub name: String,
    pub wire: u16,
    /// See [`session_id`].
    pub session: u64,
}

impl Beacon {
    #[must_use]
    pub fn new(key: &DeviceKey, name: impl Into<String>) -> Self {
        Self {
            device: key.id(),
            public: *key.public(),
            name: name.into(),
            wire: WIRE_VERSION,
            session: session_id(),
        }
    }

    /// Whether the beacon is internally consistent: the id really is
    /// the hash of the key and the wire version is ours.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.wire == WIRE_VERSION && DeviceId::of_public_key(&self.public) == self.device
    }
}

/// A device reachable on the network.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PeerInfo {
    pub device: DeviceId,
    pub public: [u8; 32],
    pub name: String,
    /// Where frames go: the device itself, or the relay carrying its
    /// session when [`Self::relayed`].
    pub addr: SocketAddr,
    /// The channel runs through a relay rather than straight to the
    /// device. A punched channel goes direct and reports `false`.
    pub relayed: bool,
    /// The channel was opened through a relay, punched or not since.
    pub through_relay: bool,
    /// The run of the process it is in, see [`session_id`].
    pub session: u64,
}

impl PeerInfo {
    #[must_use]
    pub fn from_beacon(beacon: &Beacon, addr: SocketAddr) -> Self {
        Self {
            device: beacon.device,
            public: beacon.public,
            name: beacon.name.clone(),
            addr,
            relayed: false,
            through_relay: false,
            session: beacon.session,
        }
    }
}

/// What the transport reports upward.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NetEvent {
    /// A beacon was seen on the LAN. Repeats when a device re-appears.
    Discovered(PeerInfo),
    /// An authenticated channel is open and both sides said hello.
    Connected(PeerInfo),
    /// A device with an open channel announced a new name.
    Renamed(PeerInfo),
    /// The channel closed, by request or by silence.
    Disconnected(DeviceId),
    /// An application payload arrived.
    Message { from: DeviceId, bytes: Vec<u8> },
    /// A channel through a relay moved onto a direct path (`relayed`
    /// false) or fell back to the relay.
    PathChanged { device: DeviceId, relayed: bool },
    /// A requested connection could not be opened. For a dial through a
    /// relay `addr` is the relay's.
    ConnectFailed { addr: SocketAddr, reason: String },
    /// The transport thread is gone.
    Stopped(String),
}

/// How a [`lan::LanNode`] binds and behaves.
#[derive(Clone, Debug)]
pub struct LanConfig {
    pub key: DeviceKey,
    pub name: String,
    /// Data port; `0` picks a free one.
    pub port: u16,
    pub discovery_port: u16,
    pub broadcast: Ipv4Addr,
    /// Whether to answer probes from other devices.
    pub advertise: bool,
    /// Whether to look for other devices.
    pub scan: bool,
    pub probe_interval: Duration,
    pub ping_interval: Duration,
    /// Silence after which a peer is dropped.
    pub peer_timeout: Duration,
    /// How long an outbound handshake may take.
    pub connect_timeout: Duration,
    /// Whether channels through a relay may move onto a direct path
    /// once both ends approve it.
    pub punching: bool,
    /// Set on the relay itself: forward sessions between its clients.
    pub(crate) relay: Option<RelayRole>,
}

/// What a node acting as a relay forwards, and for whom.
#[derive(Clone)]
pub(crate) struct RelayRole {
    pub(crate) limits: zudp::RelayLimits,
    /// Sessions held at once, the relay's own clients included.
    pub(crate) max_peers: usize,
    pub(crate) policy: Arc<dyn zudp::RelayPolicy>,
    /// Sees every forwarded frame; tests use it to look at what the
    /// relay could read.
    pub(crate) tap: Option<zudp::RelayTap>,
}

impl fmt::Debug for RelayRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RelayRole")
            .field("limits", &self.limits)
            .field("tap", &self.tap.is_some())
            .finish_non_exhaustive()
    }
}

impl LanConfig {
    #[must_use]
    pub fn new(key: DeviceKey, name: impl Into<String>) -> Self {
        Self {
            key,
            name: name.into(),
            port: DEFAULT_PORT,
            discovery_port: zudp::DISCOVERY_PORT,
            broadcast: Ipv4Addr::BROADCAST,
            advertise: true,
            scan: true,
            probe_interval: Duration::from_secs(5),
            ping_interval: Duration::from_secs(5),
            peer_timeout: Duration::from_secs(20),
            connect_timeout: Duration::from_secs(10),
            punching: true,
            relay: None,
        }
    }

    #[must_use]
    pub const fn port(mut self, port: u16) -> Self {
        self.port = port;
        self
    }

    #[must_use]
    pub const fn discovery_port(mut self, port: u16) -> Self {
        self.discovery_port = port;
        self
    }

    #[must_use]
    pub const fn broadcast(mut self, addr: Ipv4Addr) -> Self {
        self.broadcast = addr;
        self
    }

    #[must_use]
    pub const fn advertise(mut self, on: bool) -> Self {
        self.advertise = on;
        self
    }

    #[must_use]
    pub const fn scan(mut self, on: bool) -> Self {
        self.scan = on;
        self
    }

    #[must_use]
    pub const fn probe_interval(mut self, every: Duration) -> Self {
        self.probe_interval = every;
        self
    }

    #[must_use]
    pub const fn liveness(mut self, ping_every: Duration, timeout: Duration) -> Self {
        self.ping_interval = ping_every;
        self.peer_timeout = timeout;
        self
    }

    #[must_use]
    pub const fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    #[must_use]
    pub const fn punching(mut self, on: bool) -> Self {
        self.punching = on;
        self
    }

    #[must_use]
    pub(crate) fn relay(mut self, role: RelayRole) -> Self {
        self.relay = Some(role);
        self
    }
}

/// The transport could not start or is no longer running.
#[derive(Debug)]
pub enum NetError {
    /// The data socket could not be bound.
    Bind(zudp::Error),
    /// The transport thread could not be spawned.
    Thread(std::io::Error),
    /// The transport thread has exited.
    Stopped,
}

impl fmt::Display for NetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bind(err) => write!(f, "bind: {err}"),
            Self::Thread(err) => write!(f, "transport thread: {err}"),
            Self::Stopped => f.write_str("transport stopped"),
        }
    }
}

impl std::error::Error for NetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bind(err) => Some(err),
            Self::Thread(err) => Some(err),
            Self::Stopped => None,
        }
    }
}

/// Which channel a message rides. Each has its own ordered stream, so a
/// big transfer never holds back a cursor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Lane {
    /// Documents, records and everything that must arrive.
    #[default]
    Data,
    /// Small, urgent and soon stale: cursors, ink in progress.
    Live,
}

/// One unit on a channel.
#[derive(wincode::SchemaWrite, wincode::SchemaRead, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Frame {
    Hello(Beacon),
    Ping,
    Pong,
    App(Vec<u8>),
    /// Piece `index` of `of` of an app payload too big for one frame.
    // Not `total`: wincode-derive 0.5 names its size accumulator that.
    Part {
        id: u32,
        index: u32,
        of: u32,
        bytes: Vec<u8>,
    },
    Bye,
}

impl Frame {
    #[cfg_attr(feature = "sismo", sismo::function("sync::net::frame_encode"))]
    fn to_bytes(&self) -> Vec<u8> {
        // Every variant is plain data; the standard config cannot fail on it.
        istmo_core::codec::encode(self).unwrap_or_default()
    }

    #[cfg_attr(feature = "sismo", sismo::function("sync::net::frame_decode"))]
    fn from_bytes(bytes: &[u8]) -> Option<Self> {
        crate::decode(bytes, crate::cap::FRAME).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beacon_validates_its_own_id() {
        let key = DeviceKey::generate();
        let mut beacon = Beacon::new(&key, "Tablet");
        assert!(beacon.is_valid());
        beacon.device = DeviceId([0; 32]);
        assert!(!beacon.is_valid());
    }

    #[test]
    fn frames_round_trip() {
        let frame = Frame::App(vec![1, 2, 3]);
        assert_eq!(Frame::from_bytes(&frame.to_bytes()), Some(frame));
        assert_eq!(Frame::from_bytes(&[255, 255]), None);
    }
}
