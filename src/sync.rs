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

use notin_sync::{
    Action, BlobKey, BlobKind, DeviceId, DeviceProfile, DocId, DocStatus, DriverEvent, Invite,
    JoinFailure, LanConfig, LanNode, Member, NetEvent, PairFailure, PairingCode, PairingOffer,
    Path as ReachPath, PeerInfo, RelayAddr, RelayFailure, Removal, Role, Share, ShareBook,
    ShareEdit, ShareId, ShareMode, SyncDriver, TrustedDevice,
};

use notin_i18n::tr;

use crate::library::{FolderId, ItemId, Library, bodies};
use crate::session_end::SessionEnd;

pub mod live;
pub mod open_docs;
pub mod presence;
pub mod problem;
pub mod replica;

use live::{LiveEvent, LiveMessage};
pub use problem::SyncProblem;
use problem::{ExchangeError, Recovery};
use replica::LibraryReplica;

/// Coalescing window for local change bursts.
const DEBOUNCE: Duration = Duration::from_millis(800);
const PROFILE_FILE: &str = "device.bin";
const SHARES_FILE: &str = "shares.bin";

/// One person of a share as the UI shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberView {
    pub member: Member,
    /// Connected to this device over the network right now.
    pub online: bool,
    /// How they can be reached, if at all.
    pub path: Option<ReachPath>,
}

/// One share as the UI shows it.
// Independent facts the UI reads one by one; no two of them form a state.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareView {
    pub id: ShareId,
    pub title: String,
    pub owner: Member,
    pub is_owner: bool,
    /// What this device may do here.
    pub my_role: Role,
    pub mode: ShareMode,
    /// The folder this share mirrors, when it is a shared folder.
    pub folder: Option<FolderId>,
    /// The folders travelling with a shared folder, its root included.
    pub folders: Vec<FolderId>,
    /// Shared as a whole desk.
    pub desk: bool,
    pub entry_role: Role,
    pub approve_entry: bool,
    pub editors_add_docs: bool,
    pub docs: Vec<ItemId>,
    /// Members other than the owner.
    pub members: Vec<MemberView>,
    /// People waiting for an organizer to let them in.
    pub pending: Vec<Member>,
    pub owner_online: bool,
    pub owner_path: Option<ReachPath>,
    /// The owner's lease is gone: nothing is shared with the other
    /// members until the owner is reached again.
    pub waiting_for_owner: bool,
    pub invite_link: String,
    /// The invitation names a relay, so it works off the network too.
    pub invite_relayed: bool,
    /// The relay the share meets at is within reach.
    pub relay_online: bool,
    pub can_edit: bool,
    pub can_manage: bool,
    pub can_add_docs: bool,
    pub can_rename: bool,
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
                .any(|m| m.online && Some(m.member.device) != me)
    }

    /// A one-off session whose organizer is away: nothing moves until
    /// they are back, and nothing is lost either.
    #[must_use]
    pub fn paused(&self) -> bool {
        !self.is_owner && self.mode == ShareMode::Session && self.owner_path.is_none()
    }

    /// Whether `device` outranks nobody here, i.e. may not be touched by
    /// this device.
    #[must_use]
    pub fn outranks(&self, device: DeviceId) -> bool {
        let theirs = if device == self.owner.device {
            Role::Owner
        } else {
            match self.members.iter().find(|m| m.member.device == device) {
                Some(m) => m.member.role,
                None => return false,
            }
        };
        self.my_role.can_manage() && self.my_role > theirs
    }
}

/// An invitation accepted and not answered yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingJoinView {
    pub share: ShareId,
    pub title: String,
    pub owner: String,
    /// The owner was reached; an organizer has to let us in.
    pub awaiting_approval: bool,
}

/// A share this device left or ended, kept for a while.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentShare {
    pub id: ShareId,
    pub title: String,
    pub owner: String,
    /// Ended by this device (it can open it again) rather than left
    /// (it can ask back in).
    pub is_owner: bool,
    pub docs: Vec<ItemId>,
    /// Unix milliseconds of the leave or the end.
    pub at: u64,
}

/// Something that happened in a share and deserves a word on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShareNotice {
    Joined {
        share: ShareId,
        title: String,
        who: String,
    },
    Left {
        share: ShareId,
        title: String,
        who: String,
    },
    Renamed {
        share: ShareId,
        title: String,
        who: String,
    },
    /// Someone asked into a share this device organizes.
    EntryRequested {
        share: ShareId,
        title: String,
        who: String,
    },
    /// This device got in.
    Entered { share: ShareId, title: String },
    /// This device was refused, or the invitation is dead.
    Refused { share: ShareId, title: String },
    /// Opening a link for a share this device is already in.
    AlreadyIn { share: ShareId, title: String },
    /// This device was linked with another one, whichever side asked.
    Paired { name: String },
    /// Linking with another device did not go through.
    PairFailed(PairFailure),
    /// The share's server keeps no more of it.
    RelayFull { share: ShareId, title: String },
}

/// What the UI reads.
#[derive(Clone, Debug, Default)]
pub struct SyncSnapshot {
    /// `None` until the service is up.
    pub device: Option<DeviceId>,
    pub name: String,
    pub trusted: Vec<TrustedDevice>,
    pub online: Vec<DeviceId>,
    /// How each trusted device in `online` is reached.
    pub paths: HashMap<DeviceId, ReachPath>,
    pub discovered: Vec<PeerInfo>,
    /// Offer currently shown, if the user opened pairing.
    pub offer: Option<PairingOffer>,
    /// Name of the device we are trying to pair with.
    pub pairing_with: Option<String>,
    pub last_pair_failure: Option<(DeviceId, PairFailure)>,
    pub docs: HashMap<ItemId, DocStatus>,
    /// A file another device has (an attachment, an image) is still on
    /// its way here.
    pub files_pending: bool,
    pub shares: Vec<ShareView>,
    /// Shares we asked into and are still waiting for.
    pub pending_joins: Vec<PendingJoinView>,
    /// Shares left or ended lately, newest first.
    pub recents: Vec<RecentShare>,
    /// The name chosen for shares, when one is set.
    pub user_name: Option<String>,
    pub last_join_failure: Option<JoinFailure>,
    /// Relay in use, as configured.
    pub relay: Option<String>,
    pub relay_online: bool,
    /// Why the relay is out of reach, while it is.
    pub relay_failure: Option<RelayFailure>,
    pub error: Option<SyncProblem>,
}

/// What a new share should cover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareRequest {
    pub title: String,
    pub docs: Vec<ItemId>,
    pub mode: ShareMode,
    /// The folder this share mirrors; its documents are `docs`.
    pub folder: Option<FolderId>,
    /// Whether `docs` are a whole desk.
    pub desk: bool,
    pub entry_role: Role,
    pub approve_entry: bool,
    pub editors_add_docs: bool,
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
    Edit(ShareId, ShareEdit),
    /// Documents to bring into a share, by library id; the records that
    /// travel with them are worked out on the sync thread.
    AddDocs(ShareId, Vec<ItemId>),
    Rejoin(ShareId),
    CancelJoin(ShareId),
    Reopen(ShareId),
    ForgetRecent(ShareId),
    /// Drop recents older than this many milliseconds.
    PruneRecents(u64),
    SetUserName(Option<String>),
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
static NOTICE_LISTENERS: Mutex<Vec<flume::Sender<ShareNotice>>> = Mutex::new(Vec::new());

/// A receiver of share notices (people coming and going, requests to
/// enter, the result of a join) for the UI to turn into toasts.
#[must_use]
pub fn notices() -> flume::Receiver<ShareNotice> {
    let (tx, rx) = flume::unbounded();
    NOTICE_LISTENERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(tx);
    rx
}

#[sismo::function("sync::notify")]
fn notify(notice: ShareNotice) {
    sismo::counter!("sync.notices", 1);
    NOTICE_LISTENERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|tx| tx.send(notice.clone()).is_ok());
}

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

/// The first share `item` belongs to, if any; a document may be in
/// several, see [`shares_of`].
#[must_use]
pub fn share_of(item: ItemId) -> Option<ShareView> {
    snapshot_slot()
        .as_ref()
        .and_then(|s| s.shares.iter().find(|v| v.docs.contains(&item)).cloned())
}

/// Every share `item` belongs to.
#[must_use]
pub fn shares_of(item: ItemId) -> Vec<ShareView> {
    snapshot_slot()
        .as_ref()
        .map(|s| {
            s.shares
                .iter()
                .filter(|v| v.docs.contains(&item))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// The share mirroring `folder`, if any.
#[must_use]
pub fn share_of_folder(folder: FolderId) -> Option<ShareView> {
    snapshot_slot()
        .as_ref()
        .and_then(|s| s.shares.iter().find(|v| v.folder == Some(folder)).cloned())
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

#[sismo::function("sync::publish")]
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
        sismo::counter!("sync.cmds_queued", tx.len());
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
        role: Role,
    },
    /// An invitation to a share this device is already in.
    AlreadyIn {
        share: ShareId,
        title: String,
    },
    Pair {
        name: String,
    },
}

/// What `text` would do as a link, without acting on it.
#[must_use]
pub fn recognize_link(text: &str) -> Option<LinkTarget> {
    let text = text.trim();
    Invite::parse_link(text).map(link_target).or_else(|| {
        PairingOffer::parse_link(text).map(|offer| LinkTarget::Pair { name: offer.name })
    })
}

fn link_target(invite: Invite) -> LinkTarget {
    let inside = snapshot().shares.into_iter().find(|s| s.id == invite.share);
    match inside {
        Some(share) => LinkTarget::AlreadyIn {
            share: share.id,
            title: share.title,
        },
        None => LinkTarget::Join {
            share: invite.share,
            title: invite.title,
            owner: invite.owner.name,
            mode: invite.mode,
            role: invite.role,
        },
    }
}

/// Act on a pairing or invitation link; `None` when `text` is neither.
#[must_use]
pub fn open_link(text: &str) -> Option<LinkTarget> {
    let text = text.trim();
    if let Some(invite) = Invite::parse_link(text) {
        let target = link_target(invite.clone());
        match &target {
            LinkTarget::Join { .. } => join(invite),
            LinkTarget::AlreadyIn { share, title } => notify(ShareNotice::AlreadyIn {
                share: *share,
                title: title.clone(),
            }),
            LinkTarget::Pair { .. } => {}
        }
        return Some(target);
    }
    let offer = PairingOffer::parse_link(text)?;
    let target = LinkTarget::Pair {
        name: offer.name.clone(),
    };
    pair_with(offer);
    Some(target)
}

pub fn leave_share(id: ShareId) {
    command(Cmd::Leave(id));
}

pub fn end_share(id: ShareId) {
    command(Cmd::End(id));
}

/// Ask for a change in share `id`; applied on the spot when this device
/// organizes it, else sent to the owner.
pub fn edit_share(id: ShareId, edit: ShareEdit) {
    command(Cmd::Edit(id, edit));
}

pub fn expel(id: ShareId, device: DeviceId) {
    edit_share(id, ShareEdit::Expel(device));
}

pub fn rename_share(id: ShareId, title: impl Into<String>) {
    edit_share(id, ShareEdit::Rename(title.into()));
}

pub fn set_role(id: ShareId, device: DeviceId, role: Role) {
    edit_share(id, ShareEdit::SetRole { device, role });
}

/// Let someone waiting in, with `role`.
pub fn approve(id: ShareId, device: DeviceId, role: Role) {
    edit_share(id, ShareEdit::Approve { device, role });
}

pub fn refuse(id: ShareId, device: DeviceId) {
    edit_share(id, ShareEdit::Refuse(device));
}

pub fn set_entry_role(id: ShareId, role: Role) {
    edit_share(id, ShareEdit::SetEntryRole(role));
}

pub fn set_approve_entry(id: ShareId, on: bool) {
    edit_share(id, ShareEdit::SetApproveEntry(on));
}

pub fn set_editors_add_docs(id: ShareId, on: bool) {
    edit_share(id, ShareEdit::SetEditorsAddDocs(on));
}

/// Bring documents into share `id`.
pub fn add_docs(id: ShareId, docs: Vec<ItemId>) {
    command(Cmd::AddDocs(id, docs));
}

/// Take documents out of share `id`; every member keeps what it has.
pub fn remove_docs(id: ShareId, docs: &[ItemId]) {
    edit_share(
        id,
        ShareEdit::RemoveDocs(docs.iter().map(|d| DocId(d.0)).collect()),
    );
}

/// Ask back into a share this device left.
pub fn rejoin(id: ShareId) {
    command(Cmd::Rejoin(id));
}

/// Stop waiting on an invitation accepted earlier.
pub fn cancel_join(id: ShareId) {
    command(Cmd::CancelJoin(id));
}

/// Open a share this device ended again, under a new invitation.
pub fn reopen(id: ShareId) {
    command(Cmd::Reopen(id));
}

pub fn forget_recent(id: ShareId) {
    command(Cmd::ForgetRecent(id));
}

/// Drop recents older than `keep`.
pub fn prune_recents(keep: Duration) {
    command(Cmd::PruneRecents(
        u64::try_from(keep.as_millis()).unwrap_or(u64::MAX),
    ));
}

/// The name other people see in shares; `None` falls back to the
/// device's name.
pub fn set_user_name(name: Option<String>) {
    command(Cmd::SetUserName(name));
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
        .name("notin-sync".into())
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

#[sismo::function("sync::open_library")]
fn open_library() -> Option<async_lock::MutexGuardArc<Library>> {
    match crate::library::shared_blocking() {
        Ok(handle) => {
            sismo::scope!("sync::lock_wait");
            Some(pollster::block_on(handle.lock_arc()))
        }
        Err(err) => {
            log::error!("sync: open library: {err}");
            None
        }
    }
}

/// A shared folder this device may add to, as the share last knew it.
struct SharedFolder {
    share: ShareId,
    root: FolderId,
    docs: Vec<ItemId>,
    folders: Vec<FolderId>,
}

/// What to remember about a share once the driver forgot it, and what
/// to compare against to say who came and went.
#[derive(Clone, Debug)]
struct ShareMemo {
    title: String,
    mode: ShareMode,
    is_owner: bool,
    docs: Vec<ItemId>,
    /// Members other than the owner, by device, with their names.
    members: Vec<(DeviceId, String)>,
}

impl ShareMemo {
    fn of(share: &Share, me: DeviceId) -> Self {
        Self {
            title: share.title.clone(),
            mode: share.mode,
            is_owner: share.is_owner(me),
            docs: share.docs.iter().map(|d| ItemId(d.0)).collect(),
            members: share
                .members
                .iter()
                .map(|m| (m.device, m.name.clone()))
                .collect(),
        }
    }

    /// What changed between `self` and `now`, as notices. `me` is left
    /// out: nobody needs a toast about themselves.
    #[sismo::function("sync::memo_diff")]
    fn diff(&self, now: &Self, id: ShareId, me: DeviceId) -> Vec<ShareNotice> {
        let mut out = Vec::new();
        let title = now.title.clone();
        for (device, who) in &now.members {
            if *device != me && !self.members.iter().any(|(d, _)| d == device) {
                out.push(ShareNotice::Joined {
                    share: id,
                    title: title.clone(),
                    who: who.clone(),
                });
            }
        }
        for (device, who) in &self.members {
            if *device != me && !now.members.iter().any(|(d, _)| d == device) {
                out.push(ShareNotice::Left {
                    share: id,
                    title: title.clone(),
                    who: who.clone(),
                });
            }
        }
        if self.title != now.title {
            out.push(ShareNotice::Renamed {
                share: id,
                title,
                who: String::new(),
            });
        }
        out
    }
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
    fn start(profile: &DeviceProfile) -> Result<LanNode, notin_sync::NetError> {
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
    /// Why our relay was last out of reach, until it answers.
    relay_failure: Option<RelayFailure>,
    /// Shares whose relay said it keeps no more; each is reported once.
    relay_full: HashSet<ShareId>,
}

impl Service {
    /// A peer runs a build with another sync protocol. Said once per
    /// device, and shown in settings so the user knows what to update.
    #[sismo::function("sync::report_outdated")]
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
                notin_sync::PROTOCOL_VERSION
            );
        }
        self.error = Some(SyncProblem::Outdated { name, version });
    }

    /// Forget the last problem when `recovery` is what it waited for.
    fn recovered(&mut self, recovery: Recovery) {
        if self.error.as_ref().is_some_and(|e| e.cleared_by(recovery)) {
            self.error = None;
        }
    }

    #[sismo::function("sync::save_profile")]
    fn save_profile(&mut self) {
        if let Err(err) = self.driver.profile().save(&self.profile_path) {
            log::error!("device profile save: {err}");
            self.error = Some(SyncProblem::ProfileSave(Arc::new(err)));
        } else {
            self.recovered(Recovery::ProfileSaved);
        }
    }

    #[sismo::function("sync::save_shares")]
    /// Save how long each lease was held, so a restart resumes it.
    fn keep_lease_clocks(&mut self) {
        let _ = self.driver.keep_lease_clocks();
        self.save_shares();
    }

    fn save_shares(&mut self) {
        let book = self.driver.book();
        if let Err(err) = std::fs::write(&self.shares_path, book.encode()) {
            log::error!("share book save: {err}");
            self.error = Some(SyncProblem::SharesSave(Arc::new(err)));
        } else {
            self.recovered(Recovery::SharesSaved);
        }
    }

    /// Refresh the memos and say what changed in `changed`, if given.
    #[sismo::function("sync::remember_shares")]
    fn remember_shares(&mut self, changed: Option<ShareId>) {
        let me = self.driver.profile().id();
        let fresh: HashMap<ShareId, ShareMemo> = self
            .driver
            .shares()
            .map(|s| (s.id, ShareMemo::of(s, me)))
            .collect();
        if let Some(id) = changed
            && let Some(now) = fresh.get(&id)
        {
            match self.memos.get(&id) {
                Some(before) => {
                    for notice in before.diff(now, id, me) {
                        notify(notice);
                    }
                }
                None if !now.is_owner => notify(ShareNotice::Entered {
                    share: id,
                    title: now.title.clone(),
                }),
                None => {}
            }
        }
        self.memos = fresh;
    }

    #[sismo::function("sync::perform")]
    fn perform(&mut self, actions: Vec<Action>) {
        sismo::counter!("sync.out.actions", actions.len());
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
                    sismo::scope!("sync::perform::send");
                    sismo::counter!("sync.out.bytes", bytes.len());
                    if let Some(Err(err)) = node.map(|n| n.send(device, lane, bytes)) {
                        log::warn!("sync send: {err}");
                    }
                }
                Action::Connect { addr, public } => {
                    sismo::scope!("sync::perform::connect");
                    if let Some(Err(err)) = node.map(|n| n.connect(addr, public)) {
                        log::warn!("sync connect: {err}");
                    }
                }
                Action::ConnectHost { host, public } => {
                    sismo::scope!("sync::perform::connect_host");
                    if let Some(Err(err)) = node.map(|n| n.connect_host(host, public)) {
                        log::warn!("sync connect: {err}");
                    }
                }
                Action::ConnectVia { relay, public } => {
                    sismo::scope!("sync::perform::connect_via");
                    if let Some(Err(err)) = node.map(|n| n.connect_via(relay, public)) {
                        log::warn!("sync connect: {err}");
                    }
                }
                Action::Punch { device } => {
                    if let Some(Err(err)) = node.map(|n| n.punch(device)) {
                        log::warn!("sync punch: {err}");
                    }
                }
                Action::Disconnect { device } => {
                    if let Some(Err(err)) = node.map(|n| n.disconnect(device)) {
                        log::warn!("sync disconnect: {err}");
                    }
                }
                Action::Bulk { device, allowed } => {
                    if let Some(Err(err)) = node.map(|n| n.allow_bulk(device, allowed)) {
                        log::warn!("sync bulk: {err}");
                    }
                }
                Action::Event(event) => self.on_driver_event(event),
            }
        }
    }

    #[sismo::function("sync::driver_event")]
    fn on_driver_event(&mut self, event: DriverEvent) {
        match event {
            DriverEvent::Paired(device) => {
                log::info!("sync: paired with {}", device.name);
                self.last_failure = None;
                self.save_profile();
                notify(ShareNotice::Paired { name: device.name });
            }
            DriverEvent::Renamed { device, name } => {
                log::info!("sync: {device:?} is now {name}");
                self.save_profile();
            }
            DriverEvent::PairFailed { device, reason } => {
                self.last_failure = Some((device, reason));
                notify(ShareNotice::PairFailed(reason));
            }
            DriverEvent::Imported(_) | DriverEvent::Merged(_) => {
                sismo::scope!("sync::event::imported");
                crate::space::notify_changed();
            }
            DriverEvent::ShareChanged(id) => {
                sismo::scope!("sync::event::share_changed");
                self.last_join_failure = None;
                self.save_shares();
                self.remember_shares(Some(id));
                crate::space::notify_changed();
            }
            DriverEvent::LeaseKept(_) => self.save_shares(),
            DriverEvent::JoinPending(_) => {
                sismo::scope!("sync::event::join_pending");
                self.save_shares();
            }
            DriverEvent::EntryRequested { share, member } => {
                sismo::scope!("sync::event::entry_requested");
                self.save_shares();
                let title = self
                    .driver
                    .share(share)
                    .map(|s| s.title.clone())
                    .unwrap_or_default();
                notify(ShareNotice::EntryRequested {
                    share,
                    title,
                    who: member.name,
                });
            }
            DriverEvent::ShareRemoved { share, reason } => {
                sismo::scope!("sync::event::share_removed");
                self.save_shares();
                match self.memos.remove(&share) {
                    Some(memo) if memo.is_owner => {}
                    Some(memo) if memo.mode == ShareMode::Session => {
                        crate::session_end::announce(SessionEnd {
                            title: memo.title,
                            docs: memo.docs,
                            reason,
                        });
                    }
                    // A permanent share leaves every member its copy: the
                    // guest only hears that it is no longer shared.
                    Some(memo) if reason != Removal::Left => {
                        crate::session_end::announce_unshared(memo.title, reason);
                    }
                    Some(_) | None => {}
                }
                crate::space::notify_changed();
            }
            DriverEvent::JoinFailed { share, reason } => {
                self.last_join_failure = Some(reason);
                self.save_shares();
                if reason == JoinFailure::Refused {
                    let title = self
                        .driver
                        .left()
                        .find(|l| l.invite.share == share)
                        .map(|l| l.title.clone())
                        .unwrap_or_default();
                    notify(ShareNotice::Refused { share, title });
                }
            }
            DriverEvent::PeerOnline {
                device,
                online: false,
            } => {
                sismo::scope!("sync::event::peer_gone");
                // Gone from the network is gone from every document at
                // once, not only after its presence goes quiet.
                for item in presence::gone(device) {
                    on_live(item, device, String::new(), LiveMessage::Left);
                }
                // Ink it was drawing on a board it never announced
                // presence on, or whose presence already went quiet.
                for board in crate::active_boards::all() {
                    crate::canvas::lock(&board).forget_hand(device);
                }
            }
            DriverEvent::TrustedRelay(device) => {
                log::info!("sync: {device:?} waits at another relay");
                self.save_profile();
            }
            DriverEvent::RelayAdopted(relay) => {
                self.driver.profile_mut().set_relay(Some(relay.to_string()));
                self.save_profile();
            }
            DriverEvent::RelayUnreachable { relay, why } => {
                // Ours is the one shown; others' are retried quietly.
                if self.driver.relay().is_some_and(|r| r.device() == relay) {
                    log::warn!("sync: relay {relay:?} out of reach: {why:?}");
                    self.relay_failure = Some(why);
                } else {
                    log::debug!("sync: relay {relay:?} out of reach: {why:?}");
                }
            }
            DriverEvent::RelayOnline(online) => {
                if online {
                    self.relay_failure = None;
                }
            }
            DriverEvent::RelayFull(share) => {
                if self.relay_full.insert(share)
                    && let Some(title) = self.driver.share(share).map(|s| s.title.clone())
                {
                    notify(ShareNotice::RelayFull { share, title });
                }
            }
            // The next snapshot shows the new path, and the invitation
            // made again with the share's new key.
            DriverEvent::InviteRenewed(_)
            | DriverEvent::LeaseExpired(_)
            | DriverEvent::LeaseRenewed(_)
            | DriverEvent::PeerOnline { online: true, .. }
            | DriverEvent::PathChanged { .. } => {}
            DriverEvent::Live {
                from,
                name,
                doc,
                bytes,
            } => {
                sismo::scope!("sync::event::live");
                sismo::counter!("sync.in.live_message_bytes", bytes.len());
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

    #[sismo::function("sync::create_share")]
    fn create_share(&mut self, request: ShareRequest) -> Vec<Action> {
        let mut share = Share::new(self.driver.me(), request.title, request.mode, now_ms());
        share.entry_role = request.entry_role.for_member();
        share.approve_entry = request.approve_entry;
        share.editors_add_docs = request.editors_add_docs;
        share.folder = request.folder.map(|f| f.0);
        share.desk = request.desk;
        share.docs = request.docs.iter().map(|id| DocId(id.0)).collect();
        share.blobs = blobs_for(&request.docs);
        if let Some(folder) = request.folder
            && let Some(lib) = open_library()
        {
            share.blobs.extend(folder_blobs(&lib, folder));
        }
        self.driver.create_share(share)
    }

    /// Bring library documents into a share, with their records.
    #[sismo::function("sync::add_docs")]
    fn add_docs(&mut self, id: ShareId, docs: &[ItemId]) -> Vec<Action> {
        let blobs = blobs_for(docs);
        self.driver.edit_share(
            id,
            ShareEdit::AddDocs {
                docs: docs.iter().map(|d| DocId(d.0)).collect(),
                blobs,
            },
        )
    }

    /// Keep every shared folder this device may add to in step with the
    /// library: documents that moved in travel, documents that moved
    /// out stop travelling.
    #[sismo::function("sync::reconcile_folders")]
    fn reconcile_folders(&mut self) -> Vec<Action> {
        let me = self.driver.profile().id();
        let folders: Vec<SharedFolder> = self
            .driver
            .shares()
            .filter(|s| s.can_add_docs(me))
            .filter_map(|s| {
                s.folder.map(|f| SharedFolder {
                    share: s.id,
                    root: FolderId(f),
                    docs: s.docs.iter().map(|d| ItemId(d.0)).collect(),
                    folders: s.folders().map(FolderId).collect(),
                })
            })
            .collect();
        if folders.is_empty() {
            return Vec::new();
        }
        let Some(lib) = open_library() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for shared in folders {
            if lib.folder(shared.root).is_none() {
                continue;
            }
            let inside: Vec<ItemId> = lib
                .live_items()
                .filter(|i| folder_contains(&lib, shared.root, i.folder))
                .map(|i| i.id)
                .collect();
            let added: Vec<ItemId> = inside
                .iter()
                .copied()
                .filter(|i| !shared.docs.contains(i))
                .collect();
            let removed: Vec<ItemId> = shared
                .docs
                .iter()
                .copied()
                .filter(|i| !inside.contains(i) && lib.item(*i).is_some())
                .collect();
            let below = folders_under(&lib, shared.root);
            let new_folders: Vec<FolderId> = below
                .iter()
                .copied()
                .filter(|f| !shared.folders.contains(f))
                .collect();
            let gone_folders: Vec<[u8; 16]> = shared
                .folders
                .iter()
                .filter(|f| !below.contains(f) && lib.folder(**f).is_some())
                .map(|f| f.0)
                .collect();
            if !added.is_empty() || !new_folders.is_empty() {
                sismo::counter!("sync.folder.docs_added", added.len());
                sismo::counter!("sync.folder.folders_added", new_folders.len());
                let mut blobs = blobs_for_in(&lib, &added);
                blobs.extend(new_folders.iter().map(|f| BlobKey {
                    kind: BlobKind::Folder,
                    id: f.0,
                }));
                out.extend(self.driver.edit_share(
                    shared.share,
                    ShareEdit::AddDocs {
                        docs: added.iter().map(|d| DocId(d.0)).collect(),
                        blobs,
                    },
                ));
            }
            if !removed.is_empty() {
                sismo::counter!("sync.folder.docs_removed", removed.len());
                out.extend(self.driver.edit_share(
                    shared.share,
                    ShareEdit::RemoveDocs(removed.iter().map(|d| DocId(d.0)).collect()),
                ));
            }
            if !gone_folders.is_empty() {
                sismo::counter!("sync.folder.folders_removed", gone_folders.len());
                out.extend(
                    self.driver
                        .edit_share(shared.share, ShareEdit::RemoveFolders(gone_folders)),
                );
            }
        }
        out
    }

    #[sismo::function("sync::on_cmd")]
    fn on_cmd(&mut self, cmd: Cmd) -> Result<(), ExchangeError> {
        let actions = match cmd {
            Cmd::LocalChanged => {
                sismo::scope!("sync::cmd::announce");
                self.driver.announce()?
            }
            Cmd::DocChanged(doc) => {
                sismo::scope!("sync::cmd::doc_changed");
                self.driver.push_doc(doc)?
            }
            Cmd::BlobChanged(key) => {
                sismo::scope!("sync::cmd::blob_changed");
                self.driver.push_blob(key)?
            }
            Cmd::Live(doc, bytes) => {
                sismo::scope!("sync::cmd::live");
                sismo::counter!("sync.out.live_message_bytes", bytes.len());
                self.driver.live(doc, &bytes)
            }
            Cmd::BeginPairing => {
                self.last_failure = None;
                self.driver.begin_pairing().1
            }
            Cmd::CancelPairing => self.driver.cancel_pairing(),
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
                    // Typed codes only pair on the same network.
                    secret: None,
                    relay: None,
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
            Cmd::Revoke(device) => match self.driver.revoke(device) {
                Some(actions) => {
                    self.save_profile();
                    if let Some(node) = &self.transport.node {
                        let _ = node.disconnect(device);
                    }
                    actions
                }
                None => Vec::new(),
            },
            Cmd::CreateShare(request) => {
                sismo::scope!("sync::cmd::create_share");
                self.create_share(request)
            }
            Cmd::Join(invite) => {
                sismo::scope!("sync::cmd::join");
                self.last_join_failure = None;
                let actions = self.driver.join(invite);
                // A join outlives the app until the owner answers.
                self.save_shares();
                actions
            }
            Cmd::Leave(id) => {
                let actions = self.driver.leave_share(id);
                self.save_shares();
                actions
            }
            Cmd::End(id) => {
                let actions = self.driver.end_share(id);
                self.save_shares();
                actions
            }
            Cmd::Edit(id, edit) => {
                sismo::scope!("sync::cmd::edit_share");
                self.driver.edit_share(id, edit)
            }
            Cmd::AddDocs(id, docs) => {
                sismo::scope!("sync::cmd::add_docs");
                self.add_docs(id, &docs)
            }
            Cmd::Rejoin(id) => {
                sismo::scope!("sync::cmd::rejoin");
                self.last_join_failure = None;
                let actions = self.driver.rejoin(id);
                self.save_shares();
                actions
            }
            Cmd::CancelJoin(id) => {
                if self.driver.cancel_join(id) {
                    self.last_join_failure = None;
                    self.save_shares();
                }
                Vec::new()
            }
            Cmd::Reopen(id) => {
                sismo::scope!("sync::cmd::reopen");
                let actions = self
                    .driver
                    .reopen(id)
                    .map(|(_, actions)| actions)
                    .unwrap_or_default();
                self.save_shares();
                actions
            }
            Cmd::ForgetRecent(id) => {
                if self.driver.forget_recent(id) {
                    self.save_shares();
                }
                Vec::new()
            }
            Cmd::PruneRecents(keep) => {
                if self.driver.prune_recent(now_ms().saturating_sub(keep)) {
                    self.save_shares();
                }
                Vec::new()
            }
            Cmd::SetUserName(name) => {
                self.driver.profile_mut().set_user_name(name);
                self.save_profile();
                Vec::new()
            }
            Cmd::Present(present) => {
                sismo::scope!("sync::cmd::present");
                // A phone may end an app in the background without a word.
                if !present {
                    self.keep_lease_clocks();
                }
                self.set_present(present)?
            }
            // Handled by the service loop before it gets here.
            Cmd::Quit(_) => Vec::new(),
            Cmd::SetRelay(relay) => {
                sismo::scope!("sync::cmd::set_relay");
                self.relay_failure = None;
                let parsed = relay.as_deref().map(str::parse::<RelayAddr>);
                match parsed {
                    Some(Err(err)) => {
                        log::warn!("sync relay: {err}");
                        self.error = Some(SyncProblem::BadRelay);
                        Vec::new()
                    }
                    Some(Ok(addr)) => {
                        self.recovered(Recovery::RelaySet);
                        self.driver.profile_mut().set_relay(relay);
                        self.save_profile();
                        self.driver.set_relay(Some(addr))
                    }
                    None => {
                        self.recovered(Recovery::RelaySet);
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
    #[sismo::function("sync::set_present")]
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

    #[sismo::function("sync::share_views")]
    fn share_views(&self) -> Vec<ShareView> {
        let me = self.driver.profile().id();
        let online: Vec<DeviceId> = self.driver.connected().map(|p| p.device).collect();
        self.driver
            .shares()
            .map(|share| {
                let invite = self.driver.invite_for(share.id);
                (invite.unwrap_or_else(|| share.invite()), share)
            })
            .map(|(invite, share)| ShareView {
                id: share.id,
                title: share.title.clone(),
                owner: share.owner.clone(),
                is_owner: share.is_owner(me),
                my_role: share.role_of(me).unwrap_or(Role::Viewer),
                mode: share.mode,
                folder: share.folder.map(FolderId),
                folders: share.folders().map(FolderId).collect(),
                desk: share.desk,
                entry_role: share.entry_role,
                approve_entry: share.approve_entry,
                editors_add_docs: share.editors_add_docs,
                docs: share.docs.iter().map(|d| ItemId(d.0)).collect(),
                members: share
                    .members
                    .iter()
                    .map(|m| MemberView {
                        member: m.clone(),
                        online: online.contains(&m.device),
                        path: self.driver.path_to(m.device),
                    })
                    .collect(),
                pending: share.pending.clone(),
                owner_online: online.contains(&share.owner.device),
                owner_path: self.driver.path_to(share.owner.device),
                waiting_for_owner: self.driver.waiting_for_owner(share.id),
                invite_link: invite.to_link(),
                invite_relayed: invite.server.is_some(),
                relay_online: self.driver.share_relay_online(share.id),
                can_edit: share.can_edit(me),
                can_manage: share.can_manage(me),
                can_add_docs: share.can_add_docs(me),
                can_rename: share.can_rename(me),
            })
            .collect()
    }

    #[sismo::function("sync::recents")]
    fn recents(&self) -> Vec<RecentShare> {
        let me = self.driver.profile().display_name().to_owned();
        let mut out: Vec<RecentShare> = self
            .driver
            .ended()
            .map(|e| RecentShare {
                id: e.id,
                title: e.title.clone(),
                owner: me.clone(),
                is_owner: true,
                docs: e.docs.iter().map(|d| ItemId(d.0)).collect(),
                at: e.ended_at,
            })
            .chain(self.driver.left().map(|l| RecentShare {
                id: l.invite.share,
                title: l.title.clone(),
                owner: l.invite.owner.name.clone(),
                is_owner: false,
                docs: l.docs.iter().map(|d| ItemId(d.0)).collect(),
                at: l.left_at,
            }))
            .collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.at));
        out
    }

    #[sismo::function("sync::snapshot")]
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
            paths: self
                .driver
                .online()
                .filter_map(|p| Some((p.device, self.driver.path_to(p.device)?)))
                .collect(),
            discovered: self.driver.discovered().cloned().collect(),
            offer: self.driver.current_offer(),
            pairing_with: self.driver.pairing_target().map(str::to_owned),
            last_pair_failure: self.last_failure,
            docs,
            shares: self.share_views(),
            pending_joins: self
                .driver
                .pending_joins()
                .map(|j| PendingJoinView {
                    share: j.invite.share,
                    title: j.invite.title.clone(),
                    owner: j.invite.owner.name.clone(),
                    awaiting_approval: j.awaiting_approval,
                })
                .collect(),
            recents: self.recents(),
            user_name: profile.user_name().map(str::to_owned),
            last_join_failure: self.last_join_failure,
            relay: self.driver.relay().map(ToString::to_string),
            relay_online: self.driver.relay_online(),
            relay_failure: self.relay_failure,
            files_pending: self.driver.files_pending(),
            error: self.error.clone(),
        }
    }
}

/// Hand a live message about `item` to its open boards and listeners.
#[sismo::function("sync::on_live")]
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

/// The records that travel with `docs`: their item entries and the
/// links whose both ends are among them.
#[sismo::function("sync::blobs_for")]
fn blobs_for(docs: &[ItemId]) -> Vec<BlobKey> {
    open_library().map_or_else(|| item_blobs(docs), |lib| blobs_for_in(&lib, docs))
}

/// Whether `inner` is `folder` or sits somewhere below it. The walk is
/// bounded so a corrupted parent chain cannot spin.
#[sismo::function("sync::folder_contains")]
fn folder_contains(lib: &Library, folder: FolderId, inner: FolderId) -> bool {
    let mut cursor = inner;
    for _ in 0..=lib
        .folders_of(crate::library::ROOT_FOLDER)
        .len()
        .saturating_add(64)
    {
        if cursor == folder {
            return true;
        }
        if cursor == crate::library::ROOT_FOLDER {
            return false;
        }
        match lib.folder(cursor) {
            Some(f) => cursor = f.parent,
            None => return false,
        }
    }
    false
}

fn item_blobs(docs: &[ItemId]) -> Vec<BlobKey> {
    docs.iter()
        .map(|id| BlobKey {
            kind: BlobKind::Item,
            id: id.0,
        })
        .collect()
}

fn blobs_for_in(lib: &Library, docs: &[ItemId]) -> Vec<BlobKey> {
    let mut blobs = item_blobs(docs);
    for link in &lib.links().links {
        if docs.contains(&link.from) && docs.contains(&link.to) {
            blobs.push(BlobKey {
                kind: BlobKind::Link,
                id: link.id.0,
            });
        }
    }
    blobs
}

/// The records of `folder` and every folder below it.
#[sismo::function("sync::folder_blobs")]
fn folder_blobs(lib: &Library, folder: FolderId) -> Vec<BlobKey> {
    folders_under(lib, folder)
        .into_iter()
        .map(|f| BlobKey {
            kind: BlobKind::Folder,
            id: f.0,
        })
        .collect()
}

/// `folder` and every live folder below it.
fn folders_under(lib: &Library, folder: FolderId) -> Vec<FolderId> {
    let mut out = vec![folder];
    out.extend(
        lib.live_folders()
            .filter(|f| f.id != folder && folder_contains(lib, folder, f.parent))
            .map(|f| f.id),
    );
    out
}

#[sismo::function("sync::load_shares")]
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
    #[sismo::function("sync::service_start")]
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
            relay_failure: None,
            relay_full: HashSet::new(),
        };
        service.remember_shares(None);
        let relay = service
            .driver
            .profile()
            .relay()
            .and_then(|text| text.parse::<RelayAddr>().ok());
        let actions = service.driver.set_relay(relay);
        service.perform(actions);
        let actions = service.driver.resume_joins();
        service.perform(actions);
        Some(service)
    }

    /// Something arrived from the network.
    #[sismo::function("sync::on_net")]
    fn on_net(&mut self, event: NetEvent) {
        let from = match &event {
            NetEvent::Message { from, .. } => Some(*from),
            _ => None,
        };
        match self.driver.on_event(event) {
            Ok(actions) => {
                self.recovered(Recovery::Exchanged);
                self.perform(actions);
            }
            Err(notin_sync::SyncError::Version(version)) => self.report_outdated(from, version),
            Err(err) => {
                log::error!("sync: {err}");
                self.error = Some(SyncProblem::Exchange(Arc::new(err)));
            }
        }
    }

    /// The transport stopped: everyone is offline until it is back.
    #[sismo::function("sync::transport_lost")]
    fn transport_lost(&mut self, reason: &str) {
        log::warn!("sync network stopped: {reason}");
        self.on_net(NetEvent::Stopped(reason.to_owned()));
        self.transport.lost(Instant::now());
        self.error = Some(SyncProblem::Network(Arc::new(
            notin_sync::NetError::Stopped,
        )));
    }

    /// Start the transport again once its wait is over.
    #[sismo::function("sync::revive_transport")]
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
                self.recovered(Recovery::TransportUp);
            }
            Err(err) => {
                log::warn!("sync network: {err}");
                self.error = Some(SyncProblem::Network(Arc::new(err)));
                self.transport.lost(now);
            }
        }
    }

    #[sismo::function("sync::local_changed")]
    fn local_changed(&mut self) {
        let folder_actions = self.reconcile_folders();
        self.perform(folder_actions);
        match self.on_cmd(Cmd::LocalChanged) {
            Ok(()) => self.recovered(Recovery::Exchanged),
            Err(err) => {
                log::error!("sync: {err}");
                self.error = Some(SyncProblem::Exchange(Arc::new(err)));
            }
        }
    }
}

/// The sync service: commands from the app and network events, with
/// local changes debounced, a periodic tick, and lost devices dialed
/// again every [`notin_sync::driver::SUPERVISE`].
fn run(cmds: &flume::Receiver<Cmd>) {
    let Some(mut service) = Service::start() else {
        return;
    };
    publish(service.snapshot());
    let mut next_tick = Instant::now() + notin_sync::driver::TICK;
    let mut next_supervise = Instant::now() + notin_sync::driver::SUPERVISE;
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
        let step = {
            sismo::scope!("sync::wait");
            selector.wait_timeout(deadline.duration_since(now))
        };
        sismo::counter!("sync.cmds_queued", cmds.len());
        if let Some(events) = &events {
            sismo::counter!("sync.net_queued", events.len());
        }
        match step {
            Ok(Step::Cmd(Cmd::LocalChanged)) => {
                pending_change.get_or_insert_with(|| Instant::now() + DEBOUNCE);
            }
            Ok(Step::Cmd(Cmd::Quit(done))) => {
                service.keep_lease_clocks();
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
            sismo::scope!("sync::debounced_announce");
            pending_change = None;
            service.local_changed();
        }
        if next_tick <= now {
            sismo::scope!("sync::tick");
            next_tick = now + notin_sync::driver::TICK;
            // A member's request to bring documents in is lost when the
            // owner is out of reach; asking again each tick gets them in
            // once it is back.
            let folder_actions = service.reconcile_folders();
            service.perform(folder_actions);
            match service.driver.tick() {
                Ok(actions) => {
                    service.recovered(Recovery::Exchanged);
                    service.perform(actions);
                }
                Err(err) => {
                    log::error!("sync tick: {err}");
                    service.error = Some(SyncProblem::Exchange(Arc::new(err)));
                }
            }
        }
        if next_supervise <= now {
            sismo::scope!("sync::supervise");
            next_supervise = now + notin_sync::driver::SUPERVISE;
            let actions = service.driver.supervise(now);
            service.perform(actions);
        }
        service.revive_transport(now);
        remember_announced(&service);
        let snapshot = service.snapshot();
        publish(snapshot);
    }
}

/// Refresh the replica's doc list so statuses cover every document.
#[sismo::function("sync::remember_announced")]
fn remember_announced(service: &Service) {
    let docs = notin_sync::Replica::docs(service.driver.engine().replica());
    service.driver.with_replica(|r| r.remember(&docs));
}

/// Show `snapshot` as the current state, for headless previews.
#[cfg(test)]
pub(crate) fn publish_preview(snapshot: SyncSnapshot) {
    publish(snapshot);
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
