//! Keeps this library in step with the user's other devices.
//!
//! One background thread owns the device profile, the LAN transport
//! and the exchange driver. The UI talks to it through commands and
//! reads a snapshot that the thread refreshes after every step;
//! subscribers of [`changes`] wake whenever the snapshot moved.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use notein_sync::{
    Action, DeviceId, DeviceProfile, DocStatus, DriverEvent, LanConfig, LanNode, NetEvent,
    PairFailure, PairingCode, PairingOffer, PeerInfo, Scope, SyncDriver, SyncEngine, TrustedDevice,
};

use crate::library::{ItemId, bodies};

pub mod replica;

use replica::LibraryReplica;

/// Coalescing window for local change bursts.
const DEBOUNCE: Duration = Duration::from_millis(800);
const PROFILE_FILE: &str = "device.bin";

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
    pub error: Option<String>,
}

enum Cmd {
    LocalChanged,
    BeginPairing,
    CancelPairing,
    PairWith(PairingOffer),
    PairDiscovered { device: DeviceId, code: PairingCode },
    SetName(String),
    Revoke(DeviceId),
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

/// Where `item` stands against the other devices.
#[must_use]
pub fn doc_status(item: ItemId) -> DocStatus {
    snapshot_slot()
        .as_ref()
        .and_then(|s| s.docs.get(&item).copied())
        .unwrap_or(DocStatus::Unknown)
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

pub fn revoke(device: DeviceId) {
    command(Cmd::Revoke(device));
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
        .spawn(move || run(rx));
    if let Err(err) = spawned {
        publish(SyncSnapshot {
            error: Some(format!("sync thread: {err}")),
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
        .unwrap_or_else(|| "Este dispositivo".to_owned())
}

struct Service {
    driver: SyncDriver<LibraryReplica>,
    node: LanNode,
    profile_path: std::path::PathBuf,
    last_failure: Option<(DeviceId, PairFailure)>,
    error: Option<String>,
}

impl Service {
    fn save_profile(&mut self) {
        if let Err(err) = self.driver.profile().save(&self.profile_path) {
            log::error!("device profile save: {err}");
            self.error = Some(format!("perfil: {err}"));
        }
    }

    fn perform(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Send { device, bytes } => {
                    if let Err(err) = self.node.send(device, bytes) {
                        log::warn!("sync send: {err}");
                    }
                }
                Action::Connect { addr, public } => {
                    if let Err(err) = self.node.connect(addr, public) {
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
            DriverEvent::PairFailed { device, reason } => {
                self.last_failure = Some((device, reason));
            }
            DriverEvent::PeerOnline { .. } => {}
            DriverEvent::Imported(_) | DriverEvent::Merged(_) => {
                crate::space::notify_changed();
            }
        }
    }

    fn on_cmd(&mut self, cmd: Cmd) -> Result<(), String> {
        let actions = match cmd {
            Cmd::LocalChanged => self.driver.announce().map_err(|e| e.to_string())?,
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
                if let Err(err) = self.node.set_name(name) {
                    log::warn!("sync rename: {err}");
                }
                Vec::new()
            }
            Cmd::Revoke(device) => {
                if self.driver.profile_mut().revoke(device) {
                    self.save_profile();
                    let _ = self.node.disconnect(device);
                }
                Vec::new()
            }
        };
        self.perform(actions);
        Ok(())
    }

    fn snapshot(&mut self) -> SyncSnapshot {
        let profile = self.driver.profile();
        let docs = self
            .driver
            .engine()
            .replica()
            .known_docs()
            .iter()
            .map(|doc| (ItemId(doc.0), self.driver.engine().status(*doc)))
            .collect();
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
            error: self.error.clone(),
        }
    }
}

/// What woke the service loop.
enum Step {
    Cmd(Cmd),
    Net(NetEvent),
}

fn run(cmds: flume::Receiver<Cmd>) {
    let profile_path = bodies::root_dir().join(PROFILE_FILE);
    let profile = match DeviceProfile::load_or_create(&profile_path, &default_device_name()) {
        Ok(profile) => profile,
        Err(err) => {
            publish(SyncSnapshot {
                error: Some(format!("perfil del dispositivo: {err}")),
                ..SyncSnapshot::default()
            });
            return;
        }
    };
    let node = match LanNode::start(LanConfig::new(profile.key().clone(), profile.name())) {
        Ok(node) => node,
        Err(err) => {
            publish(SyncSnapshot {
                device: Some(profile.id()),
                name: profile.name().to_owned(),
                trusted: profile.trusted().to_vec(),
                error: Some(format!("red: {err}")),
                ..SyncSnapshot::default()
            });
            return;
        }
    };
    let engine = SyncEngine::new(LibraryReplica::default(), Scope::All);
    let mut service = Service {
        driver: SyncDriver::new(profile, engine),
        node,
        profile_path,
        last_failure: None,
        error: None,
    };
    publish(service.snapshot());

    let mut next_tick = Instant::now() + notein_sync::driver::TICK;
    let mut pending_change: Option<Instant> = None;
    loop {
        let now = Instant::now();
        let deadline = pending_change
            .map_or(next_tick, |due| due.min(next_tick))
            .max(now);
        let wait = deadline.duration_since(now);
        let events = service.node.events().clone();
        let step = flume::Selector::new()
            .recv(&cmds, |cmd| cmd.map(Step::Cmd))
            .recv(&events, |event| event.map(Step::Net))
            .wait_timeout(wait);
        match step {
            Ok(Ok(Step::Cmd(Cmd::LocalChanged))) => {
                pending_change.get_or_insert(Instant::now() + DEBOUNCE);
            }
            Ok(Ok(Step::Cmd(cmd))) => {
                if let Err(err) = service.on_cmd(cmd) {
                    log::error!("sync: {err}");
                    service.error = Some(err);
                }
            }
            Ok(Ok(Step::Net(event))) => match service.driver.on_event(event) {
                Ok(actions) => service.perform(actions),
                Err(err) => {
                    log::error!("sync: {err}");
                    service.error = Some(err.to_string());
                }
            },
            Ok(Err(_)) => break,
            Err(_) => {}
        }
        let now = Instant::now();
        if pending_change.is_some_and(|due| due <= now) {
            pending_change = None;
            if let Err(err) = service.on_cmd(Cmd::LocalChanged) {
                log::error!("sync: {err}");
                service.error = Some(err);
            }
        }
        if next_tick <= now {
            next_tick = now + notein_sync::driver::TICK;
            match service.driver.tick() {
                Ok(actions) => service.perform(actions),
                Err(err) => {
                    log::error!("sync tick: {err}");
                    service.error = Some(err.to_string());
                }
            }
        }
        remember_announced(&mut service);
        publish(service.snapshot());
    }
}

/// Refresh the replica's doc list so statuses cover every document.
fn remember_announced(service: &mut Service) {
    let docs = notein_sync::Replica::docs(service.driver.engine().replica());
    service.driver.engine_mut().replica_mut().remember(&docs);
}
