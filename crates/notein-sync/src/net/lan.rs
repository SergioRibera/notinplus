//! LAN transport on top of zudp.
//!
//! One listening socket takes inbound channels; every outbound
//! channel is its own socket pinned to the peer's key. All of it runs
//! on a dedicated thread with its own async runtime; the rest of the
//! app talks to it through channels and never blocks on the network.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tokio::sync::mpsc as tmpsc;
use zudp::{Discovery, DiscoveryConfig, ZudpSender, ZudpSocket};

use super::{APP_ID, Beacon, Frame, LanConfig, NetError, NetEvent, PeerInfo};
use crate::identity::DeviceId;

/// Stream carrying hellos, pings and byes.
const CONTROL_STREAM: u16 = 0;
/// Stream carrying application payloads.
const APP_STREAM: u16 = 1;

enum Cmd {
    Connect { addr: SocketAddr, public: [u8; 32] },
    Send { device: DeviceId, bytes: Vec<u8> },
    SetName(String),
    Disconnect(DeviceId),
    Shutdown,
}

/// Handle to the transport thread. Dropping it shuts the transport
/// down.
#[derive(Debug)]
pub struct LanNode {
    cmds: flume::Sender<Cmd>,
    events: flume::Receiver<NetEvent>,
    local_addr: SocketAddr,
    thread: Option<JoinHandle<()>>,
}

impl LanNode {
    /// Bind the data socket, start advertising / scanning as
    /// configured, and return once the socket is listening.
    pub fn start(config: LanConfig) -> Result<Self, NetError> {
        let (cmd_tx, cmd_rx) = flume::unbounded();
        let (event_tx, event_rx) = flume::unbounded();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("notein-sync-lan".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(err) => {
                        let _ = ready_tx.send(Err(NetError::Thread(err)));
                        return;
                    }
                };
                runtime.block_on(run(config, cmd_rx, event_tx, ready_tx));
            })
            .map_err(NetError::Thread)?;
        let local_addr = ready_rx.recv().map_err(|_| NetError::Stopped)??;
        Ok(Self {
            cmds: cmd_tx,
            events: event_rx,
            local_addr,
            thread: Some(thread),
        })
    }

    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Everything the transport reports. Poll it from wherever the
    /// app loop lives.
    #[must_use]
    pub const fn events(&self) -> &flume::Receiver<NetEvent> {
        &self.events
    }

    /// Open a channel to `addr`, accepting only the device whose key
    /// is `public`.
    pub fn connect(&self, addr: SocketAddr, public: [u8; 32]) -> Result<(), NetError> {
        self.command(Cmd::Connect { addr, public })
    }

    /// Deliver an application payload to a connected device.
    pub fn send(&self, device: DeviceId, bytes: Vec<u8>) -> Result<(), NetError> {
        self.command(Cmd::Send { device, bytes })
    }

    /// Change the name carried by beacons and hellos from now on.
    pub fn set_name(&self, name: impl Into<String>) -> Result<(), NetError> {
        self.command(Cmd::SetName(name.into()))
    }

    pub fn disconnect(&self, device: DeviceId) -> Result<(), NetError> {
        self.command(Cmd::Disconnect(device))
    }

    fn command(&self, cmd: Cmd) -> Result<(), NetError> {
        self.cmds.send(cmd).map_err(|_| NetError::Stopped)
    }
}

impl Drop for LanNode {
    fn drop(&mut self) {
        let _ = self.cmds.send(Cmd::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Something that happened on a socket task.
enum Internal {
    /// An outbound channel finished its handshake.
    Outbound {
        addr: SocketAddr,
        public: [u8; 32],
        sender: ZudpSender,
    },
    /// An outbound connection attempt failed.
    OutboundFailed { addr: SocketAddr, reason: String },
    /// A frame arrived; `key` is the authenticated remote key when
    /// the socket can tell.
    Frame {
        addr: SocketAddr,
        bytes: Vec<u8>,
        key: Option<[u8; 32]>,
    },
    /// The listening socket died.
    ListenStopped(String),
}

struct Link {
    addr: SocketAddr,
    sender: ZudpSender,
    last_seen: Instant,
}

struct Node {
    config: LanConfig,
    beacon: Beacon,
    links: HashMap<DeviceId, Link>,
    by_addr: HashMap<SocketAddr, DeviceId>,
    /// Outbound channels that completed the handshake but have not
    /// heard a hello back yet.
    outbound: HashMap<SocketAddr, ([u8; 32], ZudpSender)>,
    listen_sender: ZudpSender,
    events: flume::Sender<NetEvent>,
}

impl Node {
    fn emit(&self, event: NetEvent) {
        let _ = self.events.send(event);
    }

    async fn send_frame(sender: &ZudpSender, addr: SocketAddr, frame: &Frame, stream: u16) {
        if let Err(err) = sender.send_stream(frame.to_bytes(), addr, stream).await {
            tracing::warn!(target: "notein-sync", peer = %addr, "send failed: {err}");
        }
    }

    async fn on_frame(&mut self, addr: SocketAddr, bytes: Vec<u8>, key: Option<[u8; 32]>) {
        let Some(frame) = Frame::from_bytes(&bytes) else {
            tracing::warn!(target: "notein-sync", peer = %addr, "undecodable frame");
            return;
        };
        if let Frame::Hello(beacon) = frame {
            self.on_hello(addr, beacon, key).await;
            return;
        }
        let Some(device) = self.by_addr.get(&addr).copied() else {
            return;
        };
        if let Some(link) = self.links.get_mut(&device) {
            link.last_seen = Instant::now();
        }
        match frame {
            Frame::Hello(_) => {}
            Frame::Ping => {
                if let Some(link) = self.links.get(&device) {
                    Self::send_frame(&link.sender, addr, &Frame::Pong, CONTROL_STREAM).await;
                }
            }
            Frame::Pong => {}
            Frame::App(bytes) => self.emit(NetEvent::Message {
                from: device,
                bytes,
            }),
            Frame::Bye => self.drop_link(device),
        }
    }

    async fn on_hello(&mut self, addr: SocketAddr, beacon: Beacon, key: Option<[u8; 32]>) {
        if !beacon.is_valid() {
            tracing::warn!(target: "notein-sync", peer = %addr, "hello with inconsistent identity");
            return;
        }
        let (sender, reply) = match self.outbound.remove(&addr) {
            Some((pinned, sender)) => {
                if pinned != beacon.public {
                    tracing::warn!(target: "notein-sync", peer = %addr, "hello key differs from pinned key");
                    return;
                }
                (sender, false)
            }
            None => {
                if key != Some(beacon.public) {
                    tracing::warn!(target: "notein-sync", peer = %addr, "hello key differs from channel key");
                    return;
                }
                (self.listen_sender.clone(), true)
            }
        };
        if beacon.device == self.beacon.device {
            return;
        }
        if let Some(previous) = self.links.remove(&beacon.device) {
            self.by_addr.remove(&previous.addr);
        }
        let info = PeerInfo::from_beacon(&beacon, addr);
        self.links.insert(
            beacon.device,
            Link {
                addr,
                sender: sender.clone(),
                last_seen: Instant::now(),
            },
        );
        self.by_addr.insert(addr, beacon.device);
        if reply {
            Self::send_frame(
                &sender,
                addr,
                &Frame::Hello(self.beacon.clone()),
                CONTROL_STREAM,
            )
            .await;
        }
        self.emit(NetEvent::Connected(info));
    }

    fn drop_link(&mut self, device: DeviceId) {
        if let Some(link) = self.links.remove(&device) {
            self.by_addr.remove(&link.addr);
            self.emit(NetEvent::Disconnected(device));
        }
    }

    async fn on_cmd(&mut self, cmd: Cmd, internal: &tmpsc::UnboundedSender<Internal>) -> bool {
        match cmd {
            Cmd::Connect { addr, public } => {
                if self.by_addr.contains_key(&addr) || self.outbound.contains_key(&addr) {
                    return true;
                }
                spawn_outbound(
                    self.config.key.keypair().clone(),
                    addr,
                    public,
                    self.config.connect_timeout,
                    internal.clone(),
                );
            }
            Cmd::Send { device, bytes } => {
                if let Some(link) = self.links.get(&device) {
                    Self::send_frame(&link.sender, link.addr, &Frame::App(bytes), APP_STREAM).await;
                }
            }
            Cmd::SetName(name) => {
                self.beacon.name = name;
            }
            Cmd::Disconnect(device) => {
                if let Some(link) = self.links.get(&device) {
                    Self::send_frame(&link.sender, link.addr, &Frame::Bye, CONTROL_STREAM).await;
                }
                self.drop_link(device);
            }
            Cmd::Shutdown => return false,
        }
        true
    }

    async fn on_internal(&mut self, internal: Internal) -> bool {
        match internal {
            Internal::Outbound {
                addr,
                public,
                sender,
            } => {
                Self::send_frame(
                    &sender,
                    addr,
                    &Frame::Hello(self.beacon.clone()),
                    CONTROL_STREAM,
                )
                .await;
                self.outbound.insert(addr, (public, sender));
            }
            Internal::OutboundFailed { addr, reason } => {
                self.emit(NetEvent::ConnectFailed { addr, reason });
            }
            Internal::Frame { addr, bytes, key } => self.on_frame(addr, bytes, key).await,
            Internal::ListenStopped(reason) => {
                self.emit(NetEvent::Stopped(reason));
                return false;
            }
        }
        true
    }

    async fn on_tick(&mut self) {
        let now = Instant::now();
        let silent: Vec<DeviceId> = self
            .links
            .iter()
            .filter(|(_, link)| now.duration_since(link.last_seen) > self.config.peer_timeout)
            .map(|(device, _)| *device)
            .collect();
        for device in silent {
            self.drop_link(device);
        }
        for link in self.links.values() {
            Self::send_frame(&link.sender, link.addr, &Frame::Ping, CONTROL_STREAM).await;
        }
    }

    async fn say_goodbye(&self) {
        for link in self.links.values() {
            Self::send_frame(&link.sender, link.addr, &Frame::Bye, CONTROL_STREAM).await;
        }
    }
}

fn spawn_outbound(
    keypair: zudp::Keypair,
    addr: SocketAddr,
    public: [u8; 32],
    timeout: Duration,
    internal: tmpsc::UnboundedSender<Internal>,
) {
    tokio::spawn(async move {
        let conn = zudp::Zudp::default()
            .port(0)
            .security(keypair)
            .pin_remote_key(public)
            .connect::<Vec<u8>>(addr);
        let mut conn = match tokio::time::timeout(timeout, conn).await {
            Ok(Ok(conn)) => conn,
            Ok(Err(err)) => {
                let _ = internal.send(Internal::OutboundFailed {
                    addr,
                    reason: err.to_string(),
                });
                return;
            }
            Err(_) => {
                let _ = internal.send(Internal::OutboundFailed {
                    addr,
                    reason: "handshake timed out".into(),
                });
                return;
            }
        };
        if internal
            .send(Internal::Outbound {
                addr,
                public,
                sender: conn.sender(),
            })
            .is_err()
        {
            return;
        }
        while let Ok(packet) = conn.recv().await {
            if internal
                .send(Internal::Frame {
                    addr,
                    bytes: packet.msg,
                    key: Some(public),
                })
                .is_err()
            {
                return;
            }
        }
    });
}

fn spawn_listener(mut socket: ZudpSocket<Vec<u8>>, internal: tmpsc::UnboundedSender<Internal>) {
    tokio::spawn(async move {
        loop {
            match socket.recv().await {
                Ok(packet) => {
                    let key = socket.peer_remote_static_key(packet.from);
                    if internal
                        .send(Internal::Frame {
                            addr: packet.from,
                            bytes: packet.msg,
                            key,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                Err(err) => {
                    let _ = internal.send(Internal::ListenStopped(err.to_string()));
                    return;
                }
            }
        }
    });
}

fn beacon_bytes(beacon: &Beacon) -> Vec<u8> {
    bincode::encode_to_vec(beacon, bincode::config::standard()).unwrap_or_default()
}

fn decode_beacon(bytes: &[u8]) -> Option<Beacon> {
    bincode::decode_from_slice(bytes, bincode::config::standard())
        .ok()
        .map(|(beacon, _): (Beacon, usize)| beacon)
}

#[allow(clippy::too_many_lines)]
async fn run(
    config: LanConfig,
    cmds: flume::Receiver<Cmd>,
    events: flume::Sender<NetEvent>,
    ready: mpsc::SyncSender<Result<SocketAddr, NetError>>,
) {
    let socket = zudp::Zudp::default()
        .port(config.port)
        .security(config.key.keypair().clone())
        .listen::<Vec<u8>>()
        .await;
    let socket = match socket {
        Ok(socket) => socket,
        Err(err) => {
            let _ = ready.send(Err(NetError::Bind(err)));
            return;
        }
    };
    let local_addr = match socket.local_addr() {
        Ok(addr) => addr,
        Err(err) => {
            let _ = ready.send(Err(NetError::Bind(err)));
            return;
        }
    };
    let _ = ready.send(Ok(local_addr));

    let (internal_tx, mut internal_rx) = tmpsc::unbounded_channel();
    let beacon = Beacon::new(&config.key, config.name.clone());
    let mut node = Node {
        beacon: beacon.clone(),
        links: HashMap::new(),
        by_addr: HashMap::new(),
        outbound: HashMap::new(),
        listen_sender: socket.sender(),
        events,
        config,
    };
    spawn_listener(socket, internal_tx.clone());

    let advertise = if node.config.advertise {
        let cfg = DiscoveryConfig::new(APP_ID, local_addr.port())
            .discovery_port(node.config.discovery_port)
            .meta(beacon_bytes(&beacon));
        match Discovery::advertise(cfg) {
            Ok(handle) => Some(handle),
            Err(err) => {
                tracing::warn!(target: "notein-sync", "advertise failed: {err}");
                None
            }
        }
    } else {
        None
    };
    let mut scan = if node.config.scan {
        let cfg = DiscoveryConfig::new(APP_ID, 0)
            .discovery_port(node.config.discovery_port)
            .broadcast_addr(node.config.broadcast)
            .probe_interval(node.config.probe_interval);
        match Discovery::scan_stream::<Vec<u8>>(cfg).await {
            Ok(stream) => Some(stream),
            Err(err) => {
                tracing::warn!(target: "notein-sync", "scan failed: {err}");
                None
            }
        }
    } else {
        None
    };

    let mut ticker = tokio::time::interval(node.config.ping_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;

    loop {
        tokio::select! {
            cmd = cmds.recv_async() => {
                let Ok(cmd) = cmd else { break };
                let rename = matches!(cmd, Cmd::SetName(_));
                if !node.on_cmd(cmd, &internal_tx).await {
                    break;
                }
                if rename && let Some(handle) = &advertise {
                    handle.set_meta(beacon_bytes(&node.beacon));
                }
            }
            internal = internal_rx.recv() => {
                let Some(internal) = internal else { break };
                if !node.on_internal(internal).await {
                    break;
                }
            }
            _ = ticker.tick() => node.on_tick().await,
            found = next_peer(&mut scan) => {
                let Some(beacon) = decode_beacon(&found.meta) else { continue };
                if beacon.is_valid() && beacon.device != node.beacon.device {
                    node.emit(NetEvent::Discovered(PeerInfo::from_beacon(&beacon, found.data_addr)));
                }
            }
        }
    }
    node.say_goodbye().await;
    drop(advertise);
}

/// Next beacon from the scanner, or never when scanning is off.
async fn next_peer(scan: &mut Option<zudp::ScanStream<Vec<u8>>>) -> zudp::DiscoveredPeer<Vec<u8>> {
    match scan {
        Some(stream) => loop {
            match stream.next().await {
                Ok(peer) => return peer,
                Err(err) => {
                    tracing::warn!(target: "notein-sync", "scan error: {err}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        },
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, UdpSocket};

    use super::*;
    use crate::identity::DeviceKey;

    const WAIT: Duration = Duration::from_secs(5);

    fn free_udp_port() -> u16 {
        UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// The node binds every interface; tests dial it through loopback.
    fn loopback(node: &LanNode) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, node.local_addr().port()))
    }

    fn quiet(key: DeviceKey, name: &str) -> LanConfig {
        LanConfig::new(key, name)
            .port(0)
            .advertise(false)
            .scan(false)
            .liveness(Duration::from_millis(200), Duration::from_secs(2))
            .connect_timeout(Duration::from_secs(2))
    }

    fn wait_for<T>(node: &LanNode, pick: impl Fn(NetEvent) -> Option<T>) -> T {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let event = node
                .events()
                .recv_timeout(left)
                .expect("event before timeout");
            if let Some(found) = pick(event) {
                return found;
            }
        }
    }

    #[test]
    fn connect_hello_send_and_bye() {
        let key_a = DeviceKey::generate();
        let key_b = DeviceKey::generate();
        let a = LanNode::start(quiet(key_a.clone(), "A")).unwrap();
        let b = LanNode::start(quiet(key_b.clone(), "B")).unwrap();

        a.connect(loopback(&b), *key_b.public()).unwrap();
        let seen_by_a = wait_for(&a, |e| match e {
            NetEvent::Connected(p) => Some(p),
            _ => None,
        });
        let seen_by_b = wait_for(&b, |e| match e {
            NetEvent::Connected(p) => Some(p),
            _ => None,
        });
        assert_eq!(seen_by_a.device, key_b.id());
        assert_eq!(seen_by_a.name, "B");
        assert_eq!(seen_by_b.device, key_a.id());
        assert_eq!(seen_by_b.name, "A");

        a.send(key_b.id(), b"hola".to_vec()).unwrap();
        let (from, bytes) = wait_for(&b, |e| match e {
            NetEvent::Message { from, bytes } => Some((from, bytes)),
            _ => None,
        });
        assert_eq!(from, key_a.id());
        assert_eq!(bytes, b"hola");

        b.send(key_a.id(), b"chau".to_vec()).unwrap();
        let bytes = wait_for(&a, |e| match e {
            NetEvent::Message { bytes, .. } => Some(bytes),
            _ => None,
        });
        assert_eq!(bytes, b"chau");

        a.disconnect(key_b.id()).unwrap();
        let gone = wait_for(&b, |e| match e {
            NetEvent::Disconnected(d) => Some(d),
            _ => None,
        });
        assert_eq!(gone, key_a.id());
    }

    #[test]
    fn wrong_pinned_key_never_connects() {
        let key_a = DeviceKey::generate();
        let key_b = DeviceKey::generate();
        let a = LanNode::start(quiet(key_a, "A")).unwrap();
        let b = LanNode::start(quiet(key_b, "B")).unwrap();
        a.connect(loopback(&b), *DeviceKey::generate().public())
            .unwrap();
        let event = a.events().recv_timeout(Duration::from_secs(4)).unwrap();
        assert!(
            matches!(event, NetEvent::ConnectFailed { .. }),
            "got {event:?}"
        );
    }

    #[test]
    fn dropping_a_node_says_goodbye() {
        let key_a = DeviceKey::generate();
        let key_b = DeviceKey::generate();
        let a = LanNode::start(quiet(key_a.clone(), "A")).unwrap();
        let b = LanNode::start(quiet(key_b.clone(), "B")).unwrap();
        a.connect(loopback(&b), *key_b.public()).unwrap();
        wait_for(&b, |e| matches!(e, NetEvent::Connected(_)).then_some(()));
        drop(a);
        let gone = wait_for(&b, |e| match e {
            NetEvent::Disconnected(d) => Some(d),
            _ => None,
        });
        assert_eq!(gone, key_a.id());
    }

    #[test]
    fn scanner_finds_an_advertising_node() {
        let discovery_port = free_udp_port();
        let key_b = DeviceKey::generate();
        let b = LanNode::start(
            quiet(key_b.clone(), "Tablet")
                .advertise(true)
                .discovery_port(discovery_port),
        )
        .unwrap();
        let a = LanNode::start(
            quiet(DeviceKey::generate(), "PC")
                .scan(true)
                .discovery_port(discovery_port)
                .broadcast(Ipv4Addr::new(127, 255, 255, 255))
                .probe_interval(Duration::from_millis(200)),
        )
        .unwrap();
        let found = wait_for(&a, |e| match e {
            NetEvent::Discovered(p) => Some(p),
            _ => None,
        });
        assert_eq!(found.device, key_b.id());
        assert_eq!(found.name, "Tablet");
        assert_eq!(found.addr.port(), b.local_addr().port());
    }
}
