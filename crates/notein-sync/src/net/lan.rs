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

use super::{APP_ID, Beacon, Frame, LanConfig, Lane, NetError, NetEvent, PeerInfo};
use crate::identity::DeviceId;

/// Stream carrying hellos, pings and byes.
const CONTROL_STREAM: u16 = 0;
/// Stream carrying application payloads that must arrive.
const APP_STREAM: u16 = 1;
/// Stream carrying small urgent payloads (cursors, ink in progress).
const LIVE_STREAM: u16 = 2;
/// Stream carrying the parts of big payloads (files, whole documents).
const BULK_STREAM: u16 = 3;
/// Largest payload sent in one frame. zudp drops messages of more than
/// 1024 fragments (about 1.4 MB), so bigger ones travel in parts.
const PART_SIZE: usize = 256 * 1024;
/// Most parts one payload may have (256 MB).
const MAX_PARTS: u32 = 1024;
/// Pause between parts: zudp sends without flow control, so bulk
/// transfers are paced here (about 12 MB/s).
const PART_PAUSE: Duration = Duration::from_millis(20);
/// A payload whose parts stop coming is dropped after this.
const ASSEMBLY_TIMEOUT: Duration = Duration::from_secs(60);
/// Packets per second a peer may send before zudp drops them; parts
/// burst well above zudp's default of 1000.
const MAX_PPS: f64 = 50_000.0;
/// Frames kept per stream for retransmission, enough for a few parts.
const SENT_BUFFER: usize = 4096;

enum Cmd {
    Connect {
        addr: SocketAddr,
        public: [u8; 32],
    },
    ConnectHost {
        host: String,
        public: [u8; 32],
    },
    Send {
        device: DeviceId,
        lane: Lane,
        bytes: Vec<u8>,
    },
    SetName(String),
    Disconnect(DeviceId),
    Pause,
    Resume,
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

    /// Like [`Self::connect`] for a `host:port` that still needs
    /// resolving.
    pub fn connect_host(&self, host: impl Into<String>, public: [u8; 32]) -> Result<(), NetError> {
        self.command(Cmd::ConnectHost {
            host: host.into(),
            public,
        })
    }

    /// A handle that only sends, for threads that do not own the node.
    #[must_use]
    pub fn sender(&self) -> LanSender {
        LanSender {
            cmds: self.cmds.clone(),
        }
    }

    /// Deliver an application payload to a connected device.
    pub fn send(&self, device: DeviceId, lane: Lane, bytes: Vec<u8>) -> Result<(), NetError> {
        self.command(Cmd::Send {
            device,
            lane,
            bytes,
        })
    }

    /// Change the name carried by beacons and hellos from now on.
    pub fn set_name(&self, name: impl Into<String>) -> Result<(), NetError> {
        self.command(Cmd::SetName(name.into()))
    }

    pub fn disconnect(&self, device: DeviceId) -> Result<(), NetError> {
        self.command(Cmd::Disconnect(device))
    }

    /// Leave the network for now: say goodbye on every channel, stop
    /// advertising and refuse new channels until [`Self::resume`]. For
    /// an app sent to the background, which must not look present.
    pub fn pause(&self) -> Result<(), NetError> {
        self.command(Cmd::Pause)
    }

    /// Advertise and accept channels again after [`Self::pause`].
    pub fn resume(&self) -> Result<(), NetError> {
        self.command(Cmd::Resume)
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

/// Send-only handle to a [`LanNode`].
#[derive(Clone, Debug)]
pub struct LanSender {
    cmds: flume::Sender<Cmd>,
}

impl LanSender {
    pub fn send(&self, device: DeviceId, lane: Lane, bytes: Vec<u8>) -> Result<(), NetError> {
        self.cmds
            .send(Cmd::Send {
                device,
                lane,
                bytes,
            })
            .map_err(|_| NetError::Stopped)
    }
}

/// Something that happened on a socket task.
enum Internal {
    /// `host:port` of a [`Cmd::ConnectHost`] resolved to `addr`.
    Resolved { addr: SocketAddr, public: [u8; 32] },
    /// Outbound channel `dial` finished its handshake.
    Outbound {
        addr: SocketAddr,
        dial: u64,
        sender: ZudpSender,
    },
    /// An outbound connection attempt failed; `dial` is `None` when it
    /// never got as far as dialing.
    OutboundFailed {
        addr: SocketAddr,
        dial: Option<u64>,
        reason: String,
    },
    /// The socket of outbound channel `dial` stopped.
    OutboundClosed { addr: SocketAddr, dial: u64 },
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

/// The task that owns an outbound channel's socket. Dropping it aborts
/// the task, which drops the socket: zudp never tells a channel its
/// peer is gone, so nothing else would ever close it.
struct ConnTask {
    dial: u64,
    abort: tokio::task::AbortHandle,
}

impl Drop for ConnTask {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

/// An outbound channel that is not a link yet.
struct Dial {
    public: [u8; 32],
    /// Set once the handshake is done and our hello went out.
    sender: Option<ZudpSender>,
    /// When the dial started, then when the hello went out.
    since: Instant,
    task: ConnTask,
}

struct Link {
    addr: SocketAddr,
    sender: ZudpSender,
    lanes: Lanes,
    last_seen: Instant,
    /// The socket behind an outbound link, closed with it.
    conn: Option<ConnTask>,
    /// Which end opened the channel.
    dialer: DeviceId,
    since: Instant,
}

impl Link {
    /// Whether this link stays when the same device says hello on
    /// another channel that `dialer` opened. When both ends dial at once
    /// each sees two channels; both keep the one the smaller id opened,
    /// or each would send on a channel the other has dropped. Past the
    /// race a new channel always wins: the device only dials again once
    /// it has lost this one.
    fn outranks(&self, dialer: DeviceId, race: Duration) -> bool {
        self.dialer < dialer && self.since.elapsed() < race
    }
}

/// One writer task per stream of a link, so the node never waits on a
/// send and a slow stream never holds back another.
struct Lanes {
    data: flume::Sender<Vec<u8>>,
    live: flume::Sender<Vec<u8>>,
    bulk: flume::Sender<Vec<u8>>,
}

impl Lanes {
    fn spawn(sender: &ZudpSender, addr: SocketAddr) -> Self {
        Self {
            data: writer(sender.clone(), addr, APP_STREAM, None),
            live: writer(sender.clone(), addr, LIVE_STREAM, None),
            bulk: writer(sender.clone(), addr, BULK_STREAM, Some(PART_PAUSE)),
        }
    }
}

/// Task sending encoded frames on `stream` in order, `pause` apart.
fn writer(
    sender: ZudpSender,
    addr: SocketAddr,
    stream: u16,
    pause: Option<Duration>,
) -> flume::Sender<Vec<u8>> {
    let (queue, frames) = flume::unbounded::<Vec<u8>>();
    tokio::spawn(async move {
        while let Ok(frame) = frames.recv_async().await {
            if let Err(err) = sender.send_stream(frame, addr, stream).await {
                tracing::warn!(target: "notein-sync", peer = %addr, stream, "send failed: {err}");
            }
            if let Some(pause) = pause {
                tokio::time::sleep(pause).await;
            }
        }
    });
    queue
}

/// Parts of one payload received so far.
struct Assembly {
    parts: Vec<Option<Vec<u8>>>,
    missing: usize,
    started: Instant,
}

struct Node {
    config: LanConfig,
    beacon: Beacon,
    links: HashMap<DeviceId, Link>,
    by_addr: HashMap<SocketAddr, DeviceId>,
    /// Outbound channels dialing or waiting for the hello back.
    dials: HashMap<SocketAddr, Dial>,
    next_dial: u64,
    listen_sender: ZudpSender,
    events: flume::Sender<NetEvent>,
    /// Away: no channels, no hellos answered, nothing discovered.
    paused: bool,
    /// Id of the next payload sent in parts.
    next_part: u32,
    assemblies: HashMap<(DeviceId, u32), Assembly>,
}

impl Node {
    fn emit(&self, event: NetEvent) {
        let _ = self.events.send(event);
    }

    /// A new scan for other devices, when scanning is on.
    async fn open_scan(&self) -> Option<zudp::ScanStream<Vec<u8>>> {
        if !self.config.scan {
            return None;
        }
        let cfg = DiscoveryConfig::new(APP_ID, 0)
            .discovery_port(self.config.discovery_port)
            .broadcast_addr(self.config.broadcast)
            .probe_interval(self.config.probe_interval);
        match Discovery::scan_stream::<Vec<u8>>(cfg).await {
            Ok(stream) => Some(stream),
            Err(err) => {
                tracing::warn!(target: "notein-sync", "scan failed: {err}");
                None
            }
        }
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
            // A device away from the network turns our dial down.
            if frame == Frame::Bye && self.dials.remove(&addr).is_some() {
                self.emit(NetEvent::ConnectFailed {
                    addr,
                    reason: "refused".into(),
                });
            }
            return;
        };
        if let Some(link) = self.links.get_mut(&device) {
            link.last_seen = Instant::now();
        }
        match frame {
            Frame::Hello(_) | Frame::Pong => {}
            Frame::Ping => {
                if let Some(link) = self.links.get(&device) {
                    Self::send_frame(&link.sender, addr, &Frame::Pong, CONTROL_STREAM).await;
                }
            }
            Frame::App(bytes) => self.emit(NetEvent::Message {
                from: device,
                bytes,
            }),
            Frame::Part {
                id,
                index,
                total,
                bytes,
            } => self.on_part(device, id, index, total, bytes),
            Frame::Bye => self.drop_link(device),
        }
    }

    async fn on_hello(&mut self, addr: SocketAddr, beacon: Beacon, key: Option<[u8; 32]>) {
        if !beacon.is_valid() {
            tracing::warn!(target: "notein-sync", peer = %addr, "hello with inconsistent identity");
            return;
        }
        if self.by_addr.get(&addr) == Some(&beacon.device) && key == Some(beacon.public) {
            // A hello on a channel already open: the device renamed itself.
            self.emit(NetEvent::Renamed(PeerInfo::from_beacon(&beacon, addr)));
            return;
        }
        let (sender, conn, reply) = match self.dials.remove(&addr) {
            Some(Dial {
                public,
                sender: Some(sender),
                task,
                ..
            }) => {
                if public != beacon.public {
                    tracing::warn!(target: "notein-sync", peer = %addr, "hello key differs from pinned key");
                    return;
                }
                (sender, Some(task), false)
            }
            Some(dial) => {
                // The handshake is reported before any frame of the same
                // channel, so this is a stray frame of an older dial.
                self.dials.insert(addr, dial);
                return;
            }
            None => {
                if key != Some(beacon.public) {
                    tracing::warn!(target: "notein-sync", peer = %addr, "hello key differs from channel key");
                    return;
                }
                (self.listen_sender.clone(), None, true)
            }
        };
        if beacon.device == self.beacon.device {
            return;
        }
        if self.paused {
            // Say so instead of staying silent, so the dialer gives up
            // now rather than waiting on a hello that never comes.
            Self::send_frame(&sender, addr, &Frame::Bye, CONTROL_STREAM).await;
            return;
        }
        let dialer = if conn.is_some() {
            self.beacon.device
        } else {
            beacon.device
        };
        if self
            .links
            .get(&beacon.device)
            .is_some_and(|current| current.outranks(dialer, self.config.connect_timeout))
        {
            return;
        }
        if let Some(previous) = self.links.remove(&beacon.device) {
            self.by_addr.remove(&previous.addr);
            // Parts of the old channel can never complete, and a restarted
            // sender may reuse their ids.
            self.assemblies
                .retain(|(from, _), _| *from != beacon.device);
        }
        let info = PeerInfo::from_beacon(&beacon, addr);
        self.links.insert(
            beacon.device,
            Link {
                addr,
                lanes: Lanes::spawn(&sender, addr),
                sender: sender.clone(),
                last_seen: Instant::now(),
                conn,
                dialer,
                since: Instant::now(),
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

    /// Queue an app payload on its lane, in parts on the bulk stream when
    /// it is too big for one frame.
    fn send_app(&mut self, device: DeviceId, lane: Lane, bytes: Vec<u8>) {
        let Some(link) = self.links.get(&device) else {
            return;
        };
        if bytes.len() <= PART_SIZE {
            let queue = match lane {
                Lane::Data => &link.lanes.data,
                Lane::Live => &link.lanes.live,
            };
            let _ = queue.send(Frame::App(bytes).to_bytes());
            return;
        }
        let Ok(total) = u32::try_from(bytes.len().div_ceil(PART_SIZE)) else {
            return;
        };
        if total > MAX_PARTS {
            tracing::warn!(target: "notein-sync", bytes = bytes.len(), "payload too big to send");
            return;
        }
        let id = self.next_part;
        self.next_part = self.next_part.wrapping_add(1);
        for (index, chunk) in (0..total).zip(bytes.chunks(PART_SIZE)) {
            let part = Frame::Part {
                id,
                index,
                total,
                bytes: chunk.to_vec(),
            };
            let _ = link.lanes.bulk.send(part.to_bytes());
        }
    }

    /// Keep a part; once a payload has them all, it is a message.
    fn on_part(&mut self, device: DeviceId, id: u32, index: u32, total: u32, bytes: Vec<u8>) {
        if total == 0 || total > MAX_PARTS || index >= total {
            return;
        }
        let assembly = self
            .assemblies
            .entry((device, id))
            .or_insert_with(|| Assembly {
                parts: vec![None; total as usize],
                missing: total as usize,
                started: Instant::now(),
            });
        let Some(slot) = assembly.parts.get_mut(index as usize) else {
            return;
        };
        if slot.is_none() {
            *slot = Some(bytes);
            assembly.missing -= 1;
        }
        if assembly.missing > 0 {
            return;
        }
        if let Some(assembly) = self.assemblies.remove(&(device, id)) {
            let bytes = assembly.parts.into_iter().flatten().flatten().collect();
            self.emit(NetEvent::Message {
                from: device,
                bytes,
            });
        }
    }

    fn drop_link(&mut self, device: DeviceId) {
        if let Some(link) = self.links.remove(&device) {
            self.by_addr.remove(&link.addr);
            self.assemblies.retain(|(from, _), _| *from != device);
            self.emit(NetEvent::Disconnected(device));
        }
    }

    /// Dial `addr` unless a channel to it is open or on its way.
    fn dial(
        &mut self,
        addr: SocketAddr,
        public: [u8; 32],
        internal: &tmpsc::UnboundedSender<Internal>,
    ) {
        if self.paused || self.by_addr.contains_key(&addr) || self.dials.contains_key(&addr) {
            return;
        }
        let dial = self.next_dial;
        self.next_dial = self.next_dial.wrapping_add(1);
        let outbound = Outbound {
            keypair: self.config.key.keypair().clone(),
            addr,
            public,
            dial,
            timeout: self.config.connect_timeout,
        };
        let abort = tokio::spawn(outbound.run(internal.clone())).abort_handle();
        self.dials.insert(
            addr,
            Dial {
                public,
                sender: None,
                since: Instant::now(),
                task: ConnTask { dial, abort },
            },
        );
    }

    /// Whether `dial` is still the attempt in flight towards `addr`.
    fn is_current(&self, addr: SocketAddr, dial: u64) -> bool {
        self.dials.get(&addr).is_some_and(|d| d.task.dial == dial)
    }

    async fn on_cmd(&mut self, cmd: Cmd, internal: &tmpsc::UnboundedSender<Internal>) -> bool {
        match cmd {
            Cmd::ConnectHost { .. } if self.paused => {}
            Cmd::ConnectHost { host, public } => {
                let internal = internal.clone();
                tokio::spawn(async move {
                    let resolved = match tokio::net::lookup_host(host.as_str()).await {
                        Ok(mut addrs) => addrs.next(),
                        Err(err) => {
                            tracing::warn!(target: "notein-sync", "resolve {host}: {err}");
                            None
                        }
                    };
                    let event = resolved.map_or_else(
                        || Internal::OutboundFailed {
                            addr: SocketAddr::from(([0, 0, 0, 0], 0)),
                            dial: None,
                            reason: format!("could not resolve {host}"),
                        },
                        |addr| Internal::Resolved { addr, public },
                    );
                    let _ = internal.send(event);
                });
            }
            Cmd::Connect { addr, public } => self.dial(addr, public, internal),
            Cmd::Send {
                device,
                lane,
                bytes,
            } => self.send_app(device, lane, bytes),
            Cmd::SetName(name) => {
                self.beacon.name = name;
                let hello = Frame::Hello(self.beacon.clone());
                for link in self.links.values() {
                    Self::send_frame(&link.sender, link.addr, &hello, CONTROL_STREAM).await;
                }
            }
            Cmd::Pause => {
                self.say_goodbye().await;
                let devices: Vec<DeviceId> = self.links.keys().copied().collect();
                for device in devices {
                    self.drop_link(device);
                }
                self.dials.clear();
                self.paused = true;
            }
            Cmd::Resume => {
                self.dials.clear();
                self.paused = false;
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

    async fn on_internal(
        &mut self,
        event: Internal,
        internal: &tmpsc::UnboundedSender<Internal>,
    ) -> bool {
        match event {
            Internal::Resolved { addr, public } => self.dial(addr, public, internal),
            Internal::Outbound { addr, dial, sender } => {
                if !self.is_current(addr, dial) {
                    return true;
                }
                Self::send_frame(
                    &sender,
                    addr,
                    &Frame::Hello(self.beacon.clone()),
                    CONTROL_STREAM,
                )
                .await;
                if let Some(pending) = self.dials.get_mut(&addr) {
                    pending.sender = Some(sender);
                    pending.since = Instant::now();
                }
            }
            Internal::OutboundFailed { addr, dial, reason } => {
                if let Some(dial) = dial {
                    if !self.is_current(addr, dial) {
                        return true;
                    }
                    self.dials.remove(&addr);
                }
                self.emit(NetEvent::ConnectFailed { addr, reason });
            }
            Internal::OutboundClosed { addr, dial } => {
                if self.is_current(addr, dial) {
                    self.dials.remove(&addr);
                    self.emit(NetEvent::ConnectFailed {
                        addr,
                        reason: "channel closed".into(),
                    });
                }
                let linked = self
                    .links
                    .iter()
                    .find(|(_, link)| link.conn.as_ref().is_some_and(|c| c.dial == dial))
                    .map(|(device, _)| *device);
                if let Some(device) = linked {
                    self.drop_link(device);
                }
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
        self.assemblies
            .retain(|_, assembly| now.duration_since(assembly.started) < ASSEMBLY_TIMEOUT);
        // A handshake that went through without a hello back: the far
        // side never saw our hello or chose to stay quiet.
        let unanswered: Vec<SocketAddr> = self
            .dials
            .iter()
            .filter(|(_, dial)| {
                dial.sender.is_some()
                    && now.duration_since(dial.since) > self.config.connect_timeout
            })
            .map(|(addr, _)| *addr)
            .collect();
        for addr in unanswered {
            self.dials.remove(&addr);
            self.emit(NetEvent::ConnectFailed {
                addr,
                reason: "no hello back".into(),
            });
        }
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

/// One outbound channel: handshake, then forward every frame.
struct Outbound {
    keypair: zudp::Keypair,
    addr: SocketAddr,
    public: [u8; 32],
    dial: u64,
    timeout: Duration,
}

impl Outbound {
    async fn run(self, internal: tmpsc::UnboundedSender<Internal>) {
        let Self {
            keypair,
            addr,
            public,
            dial,
            timeout,
        } = self;
        let conn = zudp::Zudp::default()
            .port(0)
            .rate_limit(MAX_PPS)
            .sent_buffer(SENT_BUFFER)
            .security(keypair)
            .pin_remote_key(public)
            .connect::<Vec<u8>>(addr);
        let mut conn = match tokio::time::timeout(timeout, conn).await {
            Ok(Ok(conn)) => conn,
            Ok(Err(err)) => {
                let _ = internal.send(Internal::OutboundFailed {
                    addr,
                    dial: Some(dial),
                    reason: err.to_string(),
                });
                return;
            }
            Err(_) => {
                let _ = internal.send(Internal::OutboundFailed {
                    addr,
                    dial: Some(dial),
                    reason: "handshake timed out".into(),
                });
                return;
            }
        };
        if internal
            .send(Internal::Outbound {
                addr,
                dial,
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
        let _ = internal.send(Internal::OutboundClosed { addr, dial });
    }
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
        .rate_limit(MAX_PPS)
        .sent_buffer(SENT_BUFFER)
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
        dials: HashMap::new(),
        next_dial: 0,
        listen_sender: socket.sender(),
        events,
        config,
        paused: false,
        // Random, so a restart does not reuse the ids of parts still
        // being assembled on the other side.
        next_part: rand::random(),
        assemblies: HashMap::new(),
    };
    spawn_listener(socket, internal_tx.clone());

    let start_advertising = |node: &Node| {
        if !node.config.advertise {
            return None;
        }
        let cfg = DiscoveryConfig::new(APP_ID, local_addr.port())
            .discovery_port(node.config.discovery_port)
            .meta(beacon_bytes(&node.beacon));
        match Discovery::advertise(cfg) {
            Ok(handle) => Some(handle),
            Err(err) => {
                tracing::warn!(target: "notein-sync", "advertise failed: {err}");
                None
            }
        }
    };
    let mut advertise = start_advertising(&node);
    let mut scan = node.open_scan().await;

    let mut ticker = tokio::time::interval(node.config.ping_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;

    loop {
        tokio::select! {
            cmd = cmds.recv_async() => {
                let Ok(cmd) = cmd else { break };
                let (rename, pause, resume) = (
                    matches!(cmd, Cmd::SetName(_)),
                    matches!(cmd, Cmd::Pause),
                    matches!(cmd, Cmd::Resume),
                );
                if !node.on_cmd(cmd, &internal_tx).await {
                    break;
                }
                if rename && let Some(handle) = &advertise {
                    handle.set_meta(beacon_bytes(&node.beacon));
                }
                if pause {
                    advertise = None;
                    // Nothing is heard while away; the scan restarts on resume
                    // and reports everyone still there as found.
                    scan = None;
                } else if resume {
                    if advertise.is_none() {
                        advertise = start_advertising(&node);
                    }
                    scan = node.open_scan().await;
                }
            }
            internal = internal_rx.recv() => {
                let Some(internal) = internal else { break };
                if !node.on_internal(internal, &internal_tx).await {
                    break;
                }
            }
            _ = ticker.tick() => node.on_tick().await,
            found = next_peer(&mut scan) => {
                let Some(beacon) = decode_beacon(&found.meta) else { continue };
                if beacon.is_valid() && beacon.device != node.beacon.device && !node.paused {
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
            match stream.next_event().await {
                // A restarted device comes back as found again or updated.
                Ok(zudp::DiscoveryEvent::Found(peer) | zudp::DiscoveryEvent::Updated(peer)) => {
                    return peer;
                }
                Ok(_) => {}
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

        a.send(key_b.id(), Lane::Data, b"hola".to_vec()).unwrap();
        let (from, bytes) = wait_for(&b, |e| match e {
            NetEvent::Message { from, bytes } => Some((from, bytes)),
            _ => None,
        });
        assert_eq!(from, key_a.id());
        assert_eq!(bytes, b"hola");

        b.send(key_a.id(), Lane::Live, b"chau".to_vec()).unwrap();
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
    fn big_payloads_arrive_whole_in_parts() {
        let key_a = DeviceKey::generate();
        let key_b = DeviceKey::generate();
        let a = LanNode::start(quiet(key_a, "A")).unwrap();
        let b = LanNode::start(quiet(key_b.clone(), "B")).unwrap();
        a.connect(loopback(&b), *key_b.public()).unwrap();
        wait_for(&b, |e| matches!(e, NetEvent::Connected(_)).then_some(()));
        wait_for(&a, |e| matches!(e, NetEvent::Connected(_)).then_some(()));

        // Over zudp's 1.4 MB message limit, and not a multiple of a part.
        let big: Vec<u8> = (0..3_000_123u32).map(|i| (i % 251) as u8).collect();
        a.send(key_b.id(), Lane::Data, big.clone()).unwrap();
        a.send(key_b.id(), Lane::Live, b"cursor".to_vec()).unwrap();
        let mut got = Vec::new();
        while got.len() < 2 {
            got.push(wait_for(&b, |e| match e {
                NetEvent::Message { bytes, .. } => Some(bytes),
                _ => None,
            }));
        }
        // The cursor never waits behind the transfer.
        assert_eq!(got[0], b"cursor");
        assert_eq!(got[1].len(), big.len());
        assert!(got[1] == big);
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
    fn a_rename_reaches_open_channels() {
        let key_a = DeviceKey::generate();
        let key_b = DeviceKey::generate();
        let a = LanNode::start(quiet(key_a.clone(), "A")).unwrap();
        let b = LanNode::start(quiet(key_b.clone(), "B")).unwrap();
        a.connect(loopback(&b), *key_b.public()).unwrap();
        wait_for(&b, |e| matches!(e, NetEvent::Connected(_)).then_some(()));
        wait_for(&a, |e| matches!(e, NetEvent::Connected(_)).then_some(()));
        a.set_name("A nuevo").unwrap();
        let renamed = wait_for(&b, |e| match e {
            NetEvent::Renamed(p) => Some(p),
            _ => None,
        });
        assert_eq!(renamed.device, key_a.id());
        assert_eq!(renamed.name, "A nuevo");
        // The channel is still the same one: messages keep flowing.
        a.send(key_b.id(), Lane::Data, b"sigo".to_vec()).unwrap();
        let bytes = wait_for(&b, |e| match e {
            NetEvent::Message { bytes, .. } => Some(bytes),
            _ => None,
        });
        assert_eq!(bytes, b"sigo");
    }

    #[test]
    fn a_paused_node_leaves_and_comes_back() {
        let key_a = DeviceKey::generate();
        let key_b = DeviceKey::generate();
        let a = LanNode::start(quiet(key_a.clone(), "A")).unwrap();
        let b = LanNode::start(quiet(key_b.clone(), "B")).unwrap();
        a.connect(loopback(&b), *key_b.public()).unwrap();
        wait_for(&b, |e| matches!(e, NetEvent::Connected(_)).then_some(()));
        a.pause().unwrap();
        let gone = wait_for(&b, |e| match e {
            NetEvent::Disconnected(d) => Some(d),
            _ => None,
        });
        assert_eq!(gone, key_a.id());
        // While away nobody gets in.
        b.connect(loopback(&a), *key_a.public()).unwrap();
        let until = Instant::now() + Duration::from_millis(800);
        while let Ok(event) = b
            .events()
            .recv_timeout(until.saturating_duration_since(Instant::now()))
        {
            assert!(
                !matches!(event, NetEvent::Connected(_)),
                "joined a paused node"
            );
        }
        a.resume().unwrap();
        a.connect(loopback(&b), *key_b.public()).unwrap();
        wait_for(&b, |e| matches!(e, NetEvent::Connected(_)).then_some(()));
    }

    #[test]
    fn a_paused_node_turns_dials_away_and_takes_them_after_resume() {
        let key_a = DeviceKey::generate();
        let key_b = DeviceKey::generate();
        let a = LanNode::start(quiet(key_a.clone(), "A")).unwrap();
        let b = LanNode::start(quiet(key_b, "B")).unwrap();
        a.pause().unwrap();
        b.connect(loopback(&a), *key_a.public()).unwrap();
        // Told off right away, not after a timeout.
        let deadline = Instant::now() + Duration::from_secs(1);
        let refused = loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match b.events().recv_timeout(left) {
                Ok(NetEvent::ConnectFailed { .. }) => break true,
                Ok(_) => {}
                Err(_) => break false,
            }
        };
        assert!(refused, "a paused node must refuse the dial");
        // The refused dial no longer blocks the next one.
        a.resume().unwrap();
        b.connect(loopback(&a), *key_a.public()).unwrap();
        wait_for(&b, |e| matches!(e, NetEvent::Connected(_)).then_some(()));
    }

    #[test]
    fn a_resumed_node_finds_the_devices_it_already_knew() {
        let discovery_port = free_udp_port();
        let key_b = DeviceKey::generate();
        let _b = LanNode::start(
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
        let discovered = |node: &LanNode| {
            wait_for(node, |e| match e {
                NetEvent::Discovered(p) => Some(p.device),
                _ => None,
            })
        };
        assert_eq!(discovered(&a), key_b.id());
        a.pause().unwrap();
        a.resume().unwrap();
        assert_eq!(discovered(&a), key_b.id(), "heard again after coming back");
    }

    #[test]
    fn dialing_each_other_at_once_settles_on_one_channel() {
        // The race only bites in some orderings: run it a few times.
        for _ in 0..4 {
            let key_a = DeviceKey::generate();
            let key_b = DeviceKey::generate();
            let a = LanNode::start(quiet(key_a.clone(), "A")).unwrap();
            let b = LanNode::start(quiet(key_b.clone(), "B")).unwrap();
            a.connect(loopback(&b), *key_b.public()).unwrap();
            b.connect(loopback(&a), *key_a.public()).unwrap();
            wait_for(&a, |e| matches!(e, NetEvent::Connected(_)).then_some(()));
            wait_for(&b, |e| matches!(e, NetEvent::Connected(_)).then_some(()));
            // Let the second channel arrive and be kept or dropped.
            std::thread::sleep(Duration::from_millis(500));
            a.send(key_b.id(), Lane::Data, b"a->b".to_vec()).unwrap();
            b.send(key_a.id(), Lane::Data, b"b->a".to_vec()).unwrap();
            let got_b = wait_for(&b, |e| match e {
                NetEvent::Message { bytes, .. } => Some(bytes),
                _ => None,
            });
            let got_a = wait_for(&a, |e| match e {
                NetEvent::Message { bytes, .. } => Some(bytes),
                _ => None,
            });
            assert_eq!(got_b, b"a->b");
            assert_eq!(got_a, b"b->a");
        }
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
