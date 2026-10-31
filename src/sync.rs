//! Keeps this library in step with the user's other devices and with
//! the people documents are shared with.
//!
//! One background thread owns the device profile, the share book, the
//! LAN transport and the exchange driver. The UI talks to it through
//! commands and reads a snapshot that the thread refreshes after every
//! step; subscribers of [`changes`] wake whenever the snapshot moved.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use notein_sync::{
    Action, BlobKey, BlobKind, DeviceId, DeviceProfile, DocId, DocStatus, DriverEvent, Invite,
    JoinFailure, LanConfig, LanNode, Member, NetEvent, PairFailure, PairingCode, PairingOffer,
    PeerInfo, RelayAddr, Share, ShareBook, ShareId, ShareMode, SharePolicy, SyncDriver,
    TrustedDevice,
};

use notein_i18n::tr;

use crate::library::{ItemId, Library, bodies};
use crate::session_end::SessionEnd;

pub mod live;
pub mod open_docs;
pub mod presence;
pub mod problem;
pub mod replica;

use live::{LiveEvent, LiveMessage};
use problem::ExchangeError;
pub use problem::SyncProblem;
use replica::LibraryReplica;

/// Coalescing window for local change bursts.
const DEBOUNCE: Duration = Duration::from_millis(800);
const PROFILE_FILE: &str = "device.bin";
const SHARES_FILE: &str = "shares.bin";

/// One share as the UI shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareView {
    pub id: ShareId,
    pub title: String,
    pub owner: Member,
    pub is_owner: bool,
    pub mode: ShareMode,
    pub policy: SharePolicy,
    pub docs: Vec<ItemId>,
    /// Members other than the owner, with whether they are online.
    pub members: Vec<(Member, bool)>,
    pub owner_online: bool,
    pub invite_link: String,
    /// Whether this device may edit the share's documents.
    pub can_edit: bool,
    /// Members set to only look.
    pub viewers: Vec<DeviceId>,
}

impl ShareView {
    /// Whether someone else in the session is connected to this device
    /// (`me`): the owner, for a guest, or any other member.
    #[must_use]
    pub fn anyone_here(&self, me: Option<DeviceId>) -> bool {
        (!self.is_owner && self.owner_online)
            || self
                .members
                .iter()
                .any(|(member, online)| *online && Some(member.device) != me)
    }
}

/// What the UI reads.
#[derive(Clone, Debug, Default)]
pub struct SyncSnapshot {
    /// `None` until the service is up.
    pub device: Option<DeviceId>,
    pub name: String,
    pub trusted: Vec<TrustedDevice>,
    pub online: Vec<DeviceId>,
    pub discovered: Vec<PeerInfo>,
    /// Offer currently shown, if the user opened pairing.
    pub offer: Option<PairingOffer>,
    /// Name of the device we are trying to pair with.
    pub pairing_with: Option<String>,
    pub last_pair_failure: Option<(DeviceId, PairFailure)>,
    pub docs: HashMap<ItemId, DocStatus>,
    pub shares: Vec<ShareView>,
    /// Titles of shares we asked into and are still waiting for.
    pub pending_joins: Vec<String>,
    pub last_join_failure: Option<JoinFailure>,
    /// Relay in use, as configured.
    pub relay: Option<String>,
    pub relay_online: bool,
    pub error: Option<SyncProblem>,
}

/// What a new share should cover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareRequest {
    pub title: String,
    pub docs: Vec<ItemId>,
    pub mode: ShareMode,
    pub policy: SharePolicy,
}

enum Cmd {
    LocalChanged,
    DocChanged(DocId),
    BlobChanged(BlobKey),
    Live(DocId, Vec<u8>),
    BeginPairing,
    CancelPairing,
    PairWith(PairingOffer),
    PairDiscovered {
        device: DeviceId,
        code: PairingCode,
    },
    SetName(String),
    Revoke(DeviceId),
    CreateShare(ShareRequest),
    Join(Invite),
    Leave(ShareId),
    End(ShareId),
    Expel(ShareId, DeviceId),
    SetPolicy(ShareId, SharePolicy),
    SetViewer(ShareId, DeviceId, bool),
    SetRelay(Option<String>),
    /// The app came to the front (`true`) or went to the background.
    Present(bool),
    /// Say goodbye and end the service; answered once the network is
    /// down.
    Quit(flume::Sender<()>),
}

static CMDS: OnceLock<flume::Sender<Cmd>> = OnceLock::new();
static SNAPSHOT: Mutex<Option<SyncSnapshot>> = Mutex::new(None);
static LISTENERS: Mutex<Vec<flume::Sender<()>>> = Mutex::new(Vec::new());

fn snapshot_slot() -> std::sync::MutexGuard<'static, Option<SyncSnapshot>> {
    SNAPSHOT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Current state; empty until [`start`] ran.
#[must_use]
pub fn snapshot() -> SyncSnapshot {
    snapshot_slot().clone().unwrap_or_default()
}

/// Every document some share carries.
#[must_use]
pub fn shared_docs() -> std::collections::HashSet<ItemId> {
    snapshot_slot()
        .as_ref()
        .map(|s| {
            s.shares
                .iter()
                .flat_map(|v| v.docs.iter().copied())
                .collect()
        })
        .unwrap_or_default()
}

/// Where `item` stands against the other devices.
#[must_use]
pub fn doc_status(item: ItemId) -> DocStatus {
    snapshot_slot()
        .as_ref()
        .and_then(|s| s.docs.get(&item).copied())
        .unwrap_or(DocStatus::Unknown)
}

/// The share `item` belongs to, if any.
#[must_use]
pub fn share_of(item: ItemId) -> Option<ShareView> {
    snapshot_slot()
        .as_ref()
        .and_then(|s| s.shares.iter().find(|v| v.docs.contains(&item)).cloned())
}

/// Whether this device may edit `item`: false while a share holds
/// it for us to look at only.
#[must_use]
pub fn can_edit(item: ItemId) -> bool {
    snapshot()
        .shares
        .iter()
        .filter(|share| share.docs.contains(&item))
        .all(|share| share.can_edit)
}

/// A receiver that fires whenever the snapshot changes.
#[must_use]
pub fn changes() -> flume::Receiver<()> {
    let (tx, rx) = flume::unbounded();
    LISTENERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(tx);
    rx
}

/// The sync snapshot as reactive state; re-renders the caller on change.
pub fn use_sync() -> freya::prelude::State<SyncSnapshot> {
    use freya::prelude::{WritableUtils, spawn, use_hook, use_state};
    let state = use_state(snapshot);
    use_hook(move || {
        let changes = changes();
        let mut state = state;
        spawn(async move {
            while changes.recv_async().await.is_ok() {
                while changes.try_recv().is_ok() {}
                state.set(snapshot());
            }
        });
    });
    state
}

fn publish(snapshot: SyncSnapshot) {
    *snapshot_slot() = Some(snapshot);
    LISTENERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|tx| tx.send(()).is_ok());
}

fn command(cmd: Cmd) {
    if let Some(tx) = CMDS.get() {
        let _ = tx.send(cmd);
    }
}

/// Ephemeral state of `item` for whoever is looking at it right now.
pub fn live(item: ItemId, message: &LiveMessage) {
    command(Cmd::Live(DocId(item.0), message.to_bytes()));
}

/// A plugin wrote its snapshot for `item`; push it right away.
pub fn snapshot_changed(item: ItemId) {
    command(Cmd::BlobChanged(BlobKey {
        kind: BlobKind::Snapshot,
        id: item.0,
    }));
}

/// The attachment of `item` changed; push it.
pub fn attachment_changed(item: ItemId) {
    command(Cmd::BlobChanged(BlobKey {
        kind: BlobKind::Attachment,
        id: item.0,
    }));
}

/// `item` got the file `name`; push it.
pub fn asset_added(item: ItemId, name: &str) {
    command(Cmd::BlobChanged(BlobKey {
        kind: BlobKind::Asset,
        id: crate::library::bodies::asset_key(item, name),
    }));
}

/// A document body changed; push it to everyone right away.
pub fn doc_changed(item: ItemId) {
    command(Cmd::DocChanged(DocId(item.0)));
}

/// Something in the library changed; announce it soon.
pub fn local_changed() {
    command(Cmd::LocalChanged);
}

pub fn begin_pairing() {
    command(Cmd::BeginPairing);
}

pub fn cancel_pairing() {
    command(Cmd::CancelPairing);
}

/// Pair with a scanned or pasted offer.
pub fn pair_with(offer: PairingOffer) {
    command(Cmd::PairWith(offer));
}

/// Pair with a device seen on the network, typing the code it shows.
pub fn pair_discovered(device: DeviceId, code: PairingCode) {
    command(Cmd::PairDiscovered { device, code });
}

pub fn set_device_name(name: impl Into<String>) {
    command(Cmd::SetName(name.into()));
}

/// Whether this device should look present to the others: an app in
/// the background leaves the network until it comes back.
pub fn set_present(present: bool) {
    command(Cmd::Present(present));
}

/// On phones and tablets an app in the background stays alive with
/// its sockets open; after this long out of sight it leaves the
/// network so the other devices stop showing it as connected.
const AWAY_AFTER: std::time::Duration = std::time::Duration::from_secs(10);

/// Follow the window to the background and back, from the app root.
///
/// Writes waiting for a pause go to disk as soon as the app loses
/// focus. Desktop windows lose focus all the time without the app
/// going anywhere, so only mobile targets leave the network.
pub fn use_presence() {
    let focused = freya::prelude::Platform::get()
        .current_window()
        .is_app_focused;
    let changes = freya::prelude::use_hook(|| {
        let (tx, rx) = flume::unbounded::<bool>();
        if cfg!(any(target_os = "android", target_os = "ios")) {
            follow_presence(rx);
        }
        tx
    });
    freya::prelude::use_side_effect(move || {
        let focused = *focused.read();
        if !focused {
            crate::route::save_open_documents();
            if let Err(err) = std::thread::Builder::new()
                .name("notein-flush".into())
                .spawn(crate::deferred::flush)
            {
                log::warn!("flush thread: {err}");
            }
        }
        let _ = changes.send(focused);
    });
}

fn follow_presence(changes: flume::Receiver<bool>) {
    freya::prelude::spawn_in_window(async move {
        let mut present = true;
        while let Ok(mut focused) = changes.recv_async().await {
            if !focused && present {
                // Coming back within the grace keeps the channels.
                async_io::Timer::after(AWAY_AFTER).await;
                focused = changes.try_iter().last().unwrap_or(focused);
            }
            if focused != present {
                present = focused;
                set_present(present);
            }
        }
    });
}

/// Leave the network before the process ends, so the other devices see
/// this one go at once instead of after a silence. Waits a moment for
/// the goodbye to go out.
pub fn stop() {
    let (tx, rx) = flume::bounded(1);
    command(Cmd::Quit(tx));
    // Nothing to do if the service never started or is already gone.
    let _ = rx.recv_timeout(std::time::Duration::from_secs(1));
}

pub fn revoke(device: DeviceId) {
    command(Cmd::Revoke(device));
}

/// Start sharing documents; the share shows up in the snapshot.
pub fn create_share(request: ShareRequest) {
    command(Cmd::CreateShare(request));
}

/// Accept an invitation link.
pub fn join(invite: Invite) {
    command(Cmd::Join(invite));
}

/// What a pairing or invitation link asked for, once acted on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Join {
        share: ShareId,
        title: String,
        owner: String,
        mode: ShareMode,
    },
    Pair {
        name: String,
    },
}

/// What `text` would do as a link, without acting on it.
#[must_use]
pub fn recognize_link(text: &str) -> Option<LinkTarget> {
    let text = text.trim();
    if let Some(invite) = Invite::parse_link(text) {
        Some(LinkTarget::Join {
            share: invite.share,
            title: invite.title,
            owner: invite.owner.name,
            mode: invite.mode,
        })
    } else {
        PairingOffer::parse_link(text).map(|offer| LinkTarget::Pair { name: offer.name })
    }
}

/// Act on a pairing or invitation link; `None` when `text` is neither.
#[must_use]
pub fn open_link(text: &str) -> Option<LinkTarget> {
    let text = text.trim();
    if let Some(invite) = Invite::parse_link(text) {
        let target = LinkTarget::Join {
            share: invite.share,
            title: invite.title.clone(),
            owner: invite.owner.name.clone(),
            mode: invite.mode,
        };
        join(invite);
        Some(target)
    } else if let Some(offer) = PairingOffer::parse_link(text) {
        let target = LinkTarget::Pair {
            name: offer.name.clone(),
        };
        pair_with(offer);
        Some(target)
    } else {
        None
    }
}

pub fn leave_share(id: ShareId) {
    command(Cmd::Leave(id));
}

pub fn end_share(id: ShareId) {
    command(Cmd::End(id));
}

pub fn expel(id: ShareId, device: DeviceId) {
    command(Cmd::Expel(id, device));
}

pub fn set_share_policy(id: ShareId, policy: SharePolicy) {
    command(Cmd::SetPolicy(id, policy));
}

/// Let `device` only look at share `id` (`true`), or edit again.
pub fn set_viewer(id: ShareId, device: DeviceId, viewer: bool) {
    command(Cmd::SetViewer(id, device, viewer));
}

/// Use a relay (`host:port/<key>`), or none.
pub fn set_relay(relay: Option<String>) {
    command(Cmd::SetRelay(relay));
}

/// Spawn the service once. Safe to call from any thread, any number
/// of times.
pub fn start() {
    if CMDS.get().is_some() {
        return;
    }
    let (tx, rx) = flume::unbounded();
    if CMDS.set(tx).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("notein-sync".into())
        .spawn(move || run(&rx));
    if let Err(err) = spawned {
        publish(SyncSnapshot {
            error: Some(SyncProblem::Thread(Arc::new(err))),
            ..SyncSnapshot::default()
        });
    }
}

fn default_device_name() -> String {
    if cfg!(target_os = "android") {
        return "Android".to_owned();
    }
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| tr!("sync-default-device-name"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

fn open_library() -> Option<async_lock::MutexGuardArc<Library>> {
    match crate::library::shared_blocking() {
        Ok(handle) => Some(pollster::block_on(handle.lock_arc())),
        Err(err) => {
            log::error!("sync: open library: {err}");
            None
        }
    }
}

/// What to remember about a share once the driver forgot it.
#[derive(Clone, Debug)]
struct ShareMemo {
    title: String,
    mode: ShareMode,
    is_owner: bool,
    docs: Vec<ItemId>,
}

/// First wait before starting a stopped transport again; it doubles up
/// to [`RESTART_MAX`].
const RESTART_FIRST: Duration = Duration::from_secs(1);
const RESTART_MAX: Duration = Duration::from_secs(60);

/// The LAN transport, started again with backoff whenever it stops.
struct Transport {
    node: Option<LanNode>,
    /// When to try starting it again, and the wait after that.
    retry: Option<(Instant, Duration)>,
    /// Whether this device shows itself on the network: a node started
    /// while the app is away has to start away too.
    present: bool,
}

impl Transport {
    fn start(profile: &DeviceProfile) -> Result<LanNode, notein_sync::NetError> {
        LanNode::start(LanConfig::new(profile.key().clone(), profile.name()))
    }

    /// The node is gone or never started: try again later.
    fn lost(&mut self, now: Instant) {
        self.node = None;
        let delay = self.retry.map_or(RESTART_FIRST, |(_, delay)| delay);
        self.retry = Some((now + delay, (delay * 2).min(RESTART_MAX)));
    }

    fn retry_due(&self, now: Instant) -> bool {
        self.node.is_none() && self.retry.is_some_and(|(at, _)| at <= now)
    }
}

struct Service {
    driver: SyncDriver<LibraryReplica>,
    transport: Transport,
    profile_path: PathBuf,
    shares_path: PathBuf,
    memos: HashMap<ShareId, ShareMemo>,
    last_failure: Option<(DeviceId, PairFailure)>,
    last_join_failure: Option<JoinFailure>,
    error: Option<SyncProblem>,
    /// Devices that spoke another protocol version; each is reported once.
    outdated: HashSet<DeviceId>,
}

impl Service {
    /// A peer runs a build with another sync protocol. Said once per
    /// device, and shown in settings so the user knows what to update.
    fn report_outdated(&mut self, from: Option<DeviceId>, version: u16) {
        let Some(from) = from else {
            return;
        };
        let name = self
            .driver
            .connected()
            .find(|p| p.device == from)
            .map_or_else(|| from.short(), |p| p.name.clone());
        if self.outdated.insert(from) {
            log::warn!(
                "sync: {name} speaks protocol {version}, this build speaks {}",
                notein_sync::PROTOCOL_VERSION
            );
        }
        self.error = Some(SyncProblem::Outdated { name, version });
    }

    fn save_profile(&mut self) {
        if let Err(err) = self.driver.profile().save(&self.profile_path) {
            log::error!("device profile save: {err}");
            self.error = Some(SyncProblem::ProfileSave(Arc::new(err)));
        }
    }

    fn save_shares(&mut self) {
        let book = self.driver.book();
        if let Err(err) = std::fs::write(&self.shares_path, book.encode()) {
            log::error!("share book save: {err}");
            self.error = Some(SyncProblem::SharesSave(Arc::new(err)));
        }
    }

    fn remember_shares(&mut self) {
        let me = self.driver.profile().id();
        self.memos = self
            .driver
            .shares()
            .map(|s| {
                (
                    s.id,
                    ShareMemo {
                        title: s.title.clone(),
                        mode: s.mode,
                        is_owner: s.is_owner(me),
                        docs: s.docs.iter().map(|d| ItemId(d.0)).collect(),
                    },
                )
            })
            .collect();
    }

    fn perform(&mut self, actions: Vec<Action>) {
        for action in actions {
            // Without a transport there is nobody to send to; the next
            // round after it is back covers whatever is dropped here.
            let node = self.transport.node.as_ref();
            match action {
                Action::Send {
                    device,
                    lane,
                    bytes,
                } => {
                    if let Some(Err(err)) = node.map(|n| n.send(device, lane, bytes)) {
                        log::warn!("sync send: {err}");
                    }
                }
                Action::Connect { addr, public } => {
                    if let Some(Err(err)) = node.map(|n| n.connect(addr, public)) {
                        log::warn!("sync connect: {err}");
                    }
                }
                Action::ConnectHost { host, public } => {
                    if let Some(Err(err)) = node.map(|n| n.connect_host(host, public)) {
                        log::warn!("sync connect: {err}");
                    }
                }
                Action::Event(event) => self.on_driver_event(event),
            }
        }
    }

    fn on_driver_event(&mut self, event: DriverEvent) {
        match event {
            DriverEvent::Paired(device) => {
                log::info!("sync: paired with {}", device.name);
                self.last_failure = None;
                self.save_profile();
            }
            DriverEvent::Renamed { device, name } => {
                log::info!("sync: {device:?} is now {name}");
                self.save_profile();
            }
            DriverEvent::PairFailed { device, reason } => {
                self.last_failure = Some((device, reason));
            }
            DriverEvent::Imported(_) | DriverEvent::Merged(_) => {
                crate::space::notify_changed();
            }
            DriverEvent::ShareChanged(_) => {
                self.last_join_failure = None;
                self.save_shares();
                self.remember_shares();
                crate::space::notify_changed();
            }
            DriverEvent::ShareRemoved { share, reason } => {
                self.save_shares();
                if let Some(memo) = self.memos.remove(&share)
                    && memo.mode == ShareMode::Session
                    && !memo.is_owner
                {
                    crate::session_end::announce(SessionEnd {
                        title: memo.title,
                        docs: memo.docs,
                        reason,
                    });
                }
                crate::space::notify_changed();
            }
            DriverEvent::JoinFailed { reason, .. } => {
                self.last_join_failure = Some(reason);
            }
            DriverEvent::PeerOnline {
                device,
                online: false,
            } => {
                // Gone from the network is gone from every document at
                // once, not only after its presence goes quiet.
                for item in presence::gone(device) {
                    on_live(item, device, String::new(), LiveMessage::Left);
                }
            }
            DriverEvent::PeerOnline { online: true, .. } | DriverEvent::RelayOnline(_) => {}
            DriverEvent::Live {
                from,
                name,
                doc,
                bytes,
            } => {
                let Some(message) = LiveMessage::from_bytes(&bytes) else {
                    return;
                };
                let item = ItemId(doc.0);
                match &message {
                    LiveMessage::Left => presence::left(item, from),
                    LiveMessage::Presence { state } => {
                        presence::seen(item, from, &name, Some(state.clone()));
                    }
                    _ => {
                        presence::seen(item, from, &name, None);
                    }
                }
                on_live(item, from, name, message);
            }
        }
    }

    fn create_share(&mut self, request: ShareRequest) -> Vec<Action> {
        let mut share = Share::new(self.driver.me(), request.title, request.mode, now_ms());
        share.policy = request.policy;
        share.docs = request.docs.iter().map(|id| DocId(id.0)).collect();
        share.blobs = request
            .docs
            .iter()
            .map(|id| BlobKey {
                kind: BlobKind::Item,
                id: id.0,
            })
            .collect();
        if let Some(lib) = open_library() {
            for link in &lib.links().links {
                if request.docs.contains(&link.from) && request.docs.contains(&link.to) {
                    share.blobs.push(BlobKey {
                        kind: BlobKind::Link,
                        id: link.id.0,
                    });
                }
            }
        }
        self.driver.create_share(share)
    }

    fn on_cmd(&mut self, cmd: Cmd) -> Result<(), ExchangeError> {
        let actions = match cmd {
            Cmd::LocalChanged => self.driver.announce()?,
            Cmd::DocChanged(doc) => self.driver.push_doc(doc)?,
            Cmd::BlobChanged(key) => self.driver.push_blob(key)?,
            Cmd::Live(doc, bytes) => self.driver.live(doc, &bytes),
            Cmd::BeginPairing => {
                self.last_failure = None;
                self.driver.begin_pairing();
                Vec::new()
            }
            Cmd::CancelPairing => {
                self.driver.cancel_pairing();
                Vec::new()
            }
            Cmd::PairWith(offer) => {
                self.last_failure = None;
                self.driver.pair_with(&offer)
            }
            Cmd::PairDiscovered { device, code } => {
                let Some(peer) = self
                    .driver
                    .discovered()
                    .find(|p| p.device == device)
                    .cloned()
                else {
                    return Ok(());
                };
                self.last_failure = None;
                self.driver.pair_with(&PairingOffer {
                    device: peer.device,
                    public: peer.public,
                    name: peer.name,
                    code,
                })
            }
            Cmd::SetName(name) => {
                self.driver.profile_mut().set_name(name.clone());
                self.save_profile();
                if let Some(Err(err)) = self.transport.node.as_ref().map(|n| n.set_name(name)) {
                    log::warn!("sync rename: {err}");
                }
                Vec::new()
            }
            Cmd::Revoke(device) => {
                if self.driver.profile_mut().revoke(device) {
                    self.save_profile();
                    if let Some(node) = &self.transport.node {
                        let _ = node.disconnect(device);
                    }
                }
                Vec::new()
            }
            Cmd::CreateShare(request) => self.create_share(request),
            Cmd::Join(invite) => {
                self.last_join_failure = None;
                self.driver.join(invite)
            }
            Cmd::Leave(id) => self.driver.leave_share(id),
            Cmd::End(id) => self.driver.end_share(id),
            Cmd::Expel(id, device) => self.driver.expel(id, device),
            Cmd::SetPolicy(id, policy) => self.driver.update_share(id, |s| s.policy = policy),
            Cmd::SetViewer(id, device, viewer) => self.driver.update_share(id, |s| {
                s.viewers.retain(|d| *d != device);
                if viewer {
                    s.viewers.push(device);
                }
            }),
            Cmd::Present(present) => self.set_present(present)?,
            // Handled by the service loop before it gets here.
            Cmd::Quit(_) => Vec::new(),
            Cmd::SetRelay(relay) => {
                let parsed = relay.as_deref().map(str::parse::<RelayAddr>);
                match parsed {
                    Some(Err(err)) => {
                        log::warn!("sync relay: {err}");
                        self.error = Some(SyncProblem::BadRelay);
                        Vec::new()
                    }
                    Some(Ok(addr)) => {
                        self.driver.profile_mut().set_relay(relay);
                        self.save_profile();
                        self.driver.set_relay(Some(addr))
                    }
                    None => {
                        self.driver.profile_mut().set_relay(None);
                        self.save_profile();
                        self.driver.set_relay(None)
                    }
                }
            }
        };
        self.perform(actions);
        Ok(())
    }

    /// Leave the network while the app is away, and come back to it.
    fn set_present(&mut self, present: bool) -> Result<Vec<Action>, ExchangeError> {
        self.transport.present = present;
        let result = self
            .transport
            .node
            .as_ref()
            .map(|node| if present { node.resume() } else { node.pause() });
        if let Some(Err(err)) = result {
            log::warn!("sync presence: {err}");
        }
        if !present {
            return Ok(Vec::new());
        }
        // Back on the network: dial everyone now rather than when their
        // backoff runs out, and say what changed while away.
        let now = Instant::now();
        self.driver.redial_now(now);
        let mut actions = self.driver.supervise(now);
        actions.extend(self.driver.announce()?);
        Ok(actions)
    }

    fn share_views(&self) -> Vec<ShareView> {
        let me = self.driver.profile().id();
        let online: Vec<DeviceId> = self.driver.connected().map(|p| p.device).collect();
        self.driver
            .shares()
            .map(|share| ShareView {
                id: share.id,
                title: share.title.clone(),
                owner: share.owner.clone(),
                is_owner: share.is_owner(me),
                mode: share.mode,
                policy: share.policy,
                docs: share.docs.iter().map(|d| ItemId(d.0)).collect(),
                members: share
                    .members
                    .iter()
                    .map(|m| (m.clone(), online.contains(&m.device)))
                    .collect(),
                owner_online: online.contains(&share.owner.device),
                invite_link: self
                    .driver
                    .invite_for(share.id)
                    .map_or_else(|| share.invite().to_link(), |i| i.to_link()),
                can_edit: share.can_edit(me),
                viewers: share.viewers.clone(),
            })
            .collect()
    }

    fn snapshot(&self) -> SyncSnapshot {
        let profile = self.driver.profile();
        let mut docs: HashMap<ItemId, DocStatus> = self
            .driver
            .with_replica(|r| r.known_docs().to_vec())
            .into_iter()
            .map(|doc| (ItemId(doc.0), self.driver.engine().status(doc)))
            .collect();
        // A shared document travels on its share, not between this
        // user's devices: its share knows where it stands.
        for share in self.driver.shares() {
            for doc in &share.docs {
                let Some(status) = self.driver.share_status(share.id, *doc) else {
                    continue;
                };
                docs.entry(ItemId(doc.0))
                    .and_modify(|known| {
                        if *known == DocStatus::Unknown {
                            *known = status;
                        }
                    })
                    .or_insert(status);
            }
        }
        SyncSnapshot {
            device: Some(profile.id()),
            name: profile.name().to_owned(),
            trusted: profile.trusted().to_vec(),
            online: self.driver.online().map(|p| p.device).collect(),
            discovered: self.driver.discovered().cloned().collect(),
            offer: self.driver.current_offer(),
            pairing_with: self.driver.pairing_target().map(str::to_owned),
            last_pair_failure: self.last_failure,
            docs,
            shares: self.share_views(),
            pending_joins: self
                .driver
                .pending_joins()
                .map(|i| i.title.clone())
                .collect(),
            last_join_failure: self.last_join_failure,
            relay: self.driver.relay().map(ToString::to_string),
            relay_online: self.driver.relay_online(),
            error: self.error.clone(),
        }
    }
}

/// Hand a live message about `item` to its open boards and listeners.
fn on_live(item: ItemId, from: DeviceId, name: String, message: LiveMessage) {
    for (_, board) in crate::active_boards::for_item(item) {
        crate::canvas::lock(&board).apply_live(from, &name, message.clone());
    }
    live::dispatch(
        item,
        &LiveEvent {
            from,
            name,
            message,
        },
    );
}

fn load_shares(path: &Path) -> ShareBook {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| ShareBook::decode(&bytes))
        .unwrap_or_default()
}

/// What woke the service loop.
enum Step {
    Cmd(Cmd),
    Net(NetEvent),
    /// The transport thread is gone without a word.
    NetGone,
    /// The app dropped its end: nothing will ever ask for sync again.
    CmdsGone,
}

impl Service {
    /// Load this device's profile and shares, start the network and
    /// reach the relay. `None`, with the reason published, when this
    /// device cannot sync. A network that does not start is retried.
    fn start() -> Option<Self> {
        let root = bodies::root_dir();
        let profile_path = root.join(PROFILE_FILE);
        let shares_path = root.join(SHARES_FILE);
        let profile = match DeviceProfile::load_or_create(&profile_path, &default_device_name()) {
            Ok(profile) => profile,
            Err(err) => {
                publish(SyncSnapshot {
                    error: Some(SyncProblem::ProfileLoad(Arc::new(err))),
                    ..SyncSnapshot::default()
                });
                return None;
            }
        };
        let mut transport = Transport {
            node: None,
            retry: None,
            present: true,
        };
        let mut error = None;
        match Transport::start(&profile) {
            Ok(node) => transport.node = Some(node),
            Err(err) => {
                log::warn!("sync network: {err}");
                error = Some(SyncProblem::Network(Arc::new(err)));
                transport.lost(Instant::now());
            }
        }
        let book = load_shares(&shares_path);
        let mut service = Self {
            driver: SyncDriver::new(profile, LibraryReplica::default(), book),
            transport,
            profile_path,
            shares_path,
            memos: HashMap::new(),
            last_failure: None,
            last_join_failure: None,
            error,
            outdated: HashSet::new(),
        };
        service.remember_shares();
        let relay = service
            .driver
            .profile()
            .relay()
            .and_then(|text| text.parse::<RelayAddr>().ok());
        let actions = service.driver.set_relay(relay);
        service.perform(actions);
        Some(service)
    }

    /// Something arrived from the network.
    fn on_net(&mut self, event: NetEvent) {
        let from = match &event {
            NetEvent::Message { from, .. } => Some(*from),
            _ => None,
        };
        match self.driver.on_event(event) {
            Ok(actions) => self.perform(actions),
            Err(notein_sync::SyncError::Version(version)) => self.report_outdated(from, version),
            Err(err) => {
                log::error!("sync: {err}");
                self.error = Some(SyncProblem::Exchange(Arc::new(err)));
            }
        }
    }

    /// The transport stopped: everyone is offline until it is back.
    fn transport_lost(&mut self, reason: &str) {
        log::warn!("sync network stopped: {reason}");
        self.on_net(NetEvent::Stopped(reason.to_owned()));
        self.transport.lost(Instant::now());
        self.error = Some(SyncProblem::Network(Arc::new(
            notein_sync::NetError::Stopped,
        )));
    }

    /// Start the transport again once its wait is over.
    fn revive_transport(&mut self, now: Instant) {
        if !self.transport.retry_due(now) {
            return;
        }
        match Transport::start(self.driver.profile()) {
            Ok(node) => {
                if !self.transport.present
                    && let Err(err) = node.pause()
                {
                    log::warn!("sync presence: {err}");
                }
                self.transport.node = Some(node);
                self.transport.retry = None;
                if matches!(self.error, Some(SyncProblem::Network(_))) {
                    self.error = None;
                }
            }
            Err(err) => {
                log::warn!("sync network: {err}");
                self.error = Some(SyncProblem::Network(Arc::new(err)));
                self.transport.lost(now);
            }
        }
    }

    fn local_changed(&mut self) {
        if let Err(err) = self.on_cmd(Cmd::LocalChanged) {
            log::error!("sync: {err}");
            self.error = Some(SyncProblem::Exchange(Arc::new(err)));
        }
    }
}

/// The sync service: commands from the app and network events, with
/// local changes debounced, a periodic tick, and lost devices dialed
/// again every [`notein_sync::driver::SUPERVISE`].
fn run(cmds: &flume::Receiver<Cmd>) {
    let Some(mut service) = Service::start() else {
        return;
    };
    publish(service.snapshot());
    let mut next_tick = Instant::now() + notein_sync::driver::TICK;
    let mut next_supervise = Instant::now() + notein_sync::driver::SUPERVISE;
    let mut pending_change: Option<Instant> = None;
    loop {
        let now = Instant::now();
        let mut deadline = next_tick.min(next_supervise);
        if let Some(due) = pending_change {
            deadline = deadline.min(due);
        }
        if let Some((due, _)) = service.transport.retry {
            deadline = deadline.min(due);
        }
        let deadline = deadline.max(now);
        let events = service.transport.node.as_ref().map(|n| n.events().clone());
        let mut selector =
            flume::Selector::new().recv(cmds, |cmd| cmd.map_or(Step::CmdsGone, Step::Cmd));
        if let Some(events) = &events {
            selector = selector.recv(events, |event| event.map_or(Step::NetGone, Step::Net));
        }
        match selector.wait_timeout(deadline.duration_since(now)) {
            Ok(Step::Cmd(Cmd::LocalChanged)) => {
                pending_change.get_or_insert_with(|| Instant::now() + DEBOUNCE);
            }
            Ok(Step::Cmd(Cmd::Quit(done))) => {
                // Dropping the node says goodbye on every channel.
                drop(service);
                let _ = done.send(());
                return;
            }
            Ok(Step::Cmd(cmd)) => {
                if let Err(err) = service.on_cmd(cmd) {
                    log::error!("sync: {err}");
                    service.error = Some(SyncProblem::Exchange(Arc::new(err)));
                }
            }
            Ok(Step::Net(NetEvent::Stopped(reason))) => service.transport_lost(&reason),
            Ok(Step::Net(event)) => service.on_net(event),
            Ok(Step::NetGone) => service.transport_lost("transport thread exited"),
            Ok(Step::CmdsGone) => break,
            Err(_) => {}
        }
        let now = Instant::now();
        if pending_change.is_some_and(|due| due <= now) {
            pending_change = None;
            service.local_changed();
        }
        if next_tick <= now {
            next_tick = now + notein_sync::driver::TICK;
            match service.driver.tick() {
                Ok(actions) => service.perform(actions),
                Err(err) => {
                    log::error!("sync tick: {err}");
                    service.error = Some(SyncProblem::Exchange(Arc::new(err)));
                }
            }
        }
        if next_supervise <= now {
            next_supervise = now + notein_sync::driver::SUPERVISE;
            let actions = service.driver.supervise(now);
            service.perform(actions);
        }
        service.revive_transport(now);
        remember_announced(&service);
        publish(service.snapshot());
    }
}

/// Refresh the replica's doc list so statuses cover every document.
fn remember_announced(service: &Service) {
    let docs = notein_sync::Replica::docs(service.driver.engine().replica());
    service.driver.with_replica(|r| r.remember(&docs));
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{RESTART_MAX, Transport};

    #[test]
    fn a_stopped_transport_is_retried_with_growing_waits() {
        let mut transport = Transport {
            node: None,
            retry: None,
            present: true,
        };
        let t0 = Instant::now();
        let mut waits = Vec::new();
        let mut at = t0;
        for _ in 0..8 {
            transport.lost(at);
            let (due, _) = transport.retry.expect("a retry is scheduled");
            assert!(!transport.retry_due(at));
            assert!(transport.retry_due(due));
            waits.push(due.duration_since(at));
            at = due;
        }
        assert_eq!(waits[0], Duration::from_secs(1));
        assert_eq!(waits[1], Duration::from_secs(2));
        assert_eq!(waits[7], RESTART_MAX);
    }
}
