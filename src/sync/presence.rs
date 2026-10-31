//! Who else has a document open, and where they are in it.
//!
//! Every live message about a document says its sender is there; a
//! [`DocPresence`] held while a document is on screen says so for this
//! device, repeating itself so the others know it has not left. A peer
//! that goes quiet for [`GONE_AFTER`] has left.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use notein_plugin::{Peer, Presence};
use notein_sync::DeviceId;

use super::live::LiveMessage;
use crate::library::ItemId;

/// A peer silent for this long has left.
const GONE_AFTER: Duration = Duration::from_secs(8);
/// How often an open document tells the others it is still open.
const HEARTBEAT: Duration = Duration::from_secs(3);
/// Following someone who dropped out of sight resumes if they are back
/// within this long.
const RESUME_FOLLOW: Duration = Duration::from_secs(120);

struct Here {
    name: String,
    state: Vec<u8>,
    seen: Instant,
    /// When they last published a place here: the document they are
    /// working in is the one they moved in last.
    moved: Instant,
}

type Peers = HashMap<ItemId, HashMap<DeviceId, Here>>;

static PEERS: Mutex<Option<Peers>> = Mutex::new(None);
static WATCHERS: Mutex<Vec<(ItemId, flume::Sender<()>)>> = Mutex::new(Vec::new());
/// This device's presence per open document.
static MINE: Mutex<Vec<(ItemId, Weak<DocPresence>)>> = Mutex::new(Vec::new());
/// Whom this device follows: its view goes wherever theirs does.
static FOLLOWING: Mutex<Option<DeviceId>> = Mutex::new(None);
/// Whom this device followed until they dropped out of sight, and
/// since when: following picks up again if they are back in time.
static AWAITING: Mutex<Option<(DeviceId, Instant)>> = Mutex::new(None);
/// Woken on any arrival, move, departure or follow change, anywhere.
static ALL_WATCHERS: Mutex<Vec<flume::Sender<()>>> = Mutex::new(Vec::new());

fn peers() -> MutexGuard<'static, Option<Peers>> {
    PEERS.lock().unwrap_or_else(PoisonError::into_inner)
}

fn notify(item: ItemId) {
    WATCHERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|(id, tx)| !tx.is_disconnected() && (*id != item || tx.send(()).is_ok()));
    notify_all();
}

fn notify_all() {
    ALL_WATCHERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|tx| tx.send(()).is_ok());
}

/// Wake on any change to anyone's presence, or to whom this device
/// follows.
#[must_use]
pub fn watch_all() -> flume::Receiver<()> {
    let (tx, rx) = flume::unbounded();
    ALL_WATCHERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(tx);
    ensure_ticker();
    rx
}

/// Follow `device`, or stop with `None`.
pub fn follow(device: Option<DeviceId>) {
    *awaiting() = None;
    set_following(device);
}

/// The followed peer dropped out of sight: stop moving with it, and
/// start again if it comes back soon.
fn lose_sight(device: DeviceId) {
    set_following(None);
    *awaiting() = Some((device, Instant::now()));
}

fn awaiting() -> MutexGuard<'static, Option<(DeviceId, Instant)>> {
    AWAITING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `from` is back in sight: follow it again if it was followed when it
/// dropped out, not too long ago.
fn resume_following(from: DeviceId) {
    let mut awaiting = awaiting();
    let Some((device, since)) = *awaiting else {
        return;
    };
    if since.elapsed() > RESUME_FOLLOW {
        *awaiting = None;
        return;
    }
    if device != from {
        return;
    }
    *awaiting = None;
    drop(awaiting);
    set_following(Some(from));
}

fn set_following(device: Option<DeviceId>) {
    let mut following = FOLLOWING.lock().unwrap_or_else(PoisonError::into_inner);
    if *following == device {
        return;
    }
    *following = device;
    drop(following);
    // Every open document may have to move.
    let items: Vec<ItemId> = peers()
        .as_ref()
        .map(|all| all.keys().copied().collect())
        .unwrap_or_default();
    for item in items {
        WATCHERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(id, tx)| !tx.is_disconnected() && (*id != item || tx.send(()).is_ok()));
    }
    notify_all();
}

/// Whom this device follows.
#[must_use]
pub fn following() -> Option<DeviceId> {
    *FOLLOWING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The document `device` was most recently in, with its name and color.
#[must_use]
pub fn whereabouts(device: DeviceId) -> Option<(ItemId, Peer)> {
    let guard = peers();
    guard
        .as_ref()?
        .iter()
        .filter_map(|(item, here)| {
            let seen = here
                .get(&device)
                .filter(|here| here.seen.elapsed() < GONE_AFTER)?;
            Some((*item, seen))
        })
        .max_by_key(|(_, here)| here.moved)
        .map(|(item, here)| {
            (
                item,
                Peer {
                    id: device.0,
                    name: here.name.clone(),
                    color: color(device),
                    state: here.state.clone(),
                },
            )
        })
}

/// The color everything of `device` is drawn in.
#[must_use]
pub fn color(device: DeviceId) -> [u8; 3] {
    let hue = f32::from(device.0[0]) / 255.0 * 360.0;
    let color = freya::prelude::Color::from_hsv(hue, 0.62, 0.92);
    [color.r(), color.g(), color.b()]
}

/// `from` sent something about `item`: it is there. `state` replaces
/// what it last published, when given. Returns whether that changed
/// anything.
pub(super) fn seen(item: ItemId, from: DeviceId, name: &str, state: Option<Vec<u8>>) -> bool {
    let mut guard = peers();
    let here = guard
        .get_or_insert_with(HashMap::new)
        .entry(item)
        .or_default();
    let arrived = !here.contains_key(&from);
    let entry = here.entry(from).or_insert_with(|| Here {
        name: name.to_owned(),
        state: Vec::new(),
        seen: Instant::now(),
        moved: Instant::now(),
    });
    entry.seen = Instant::now();
    name.clone_into(&mut entry.name);
    let moved = state.is_some_and(|state| {
        let changed = entry.state != state;
        if changed {
            entry.moved = Instant::now();
        }
        entry.state = state;
        changed
    });
    drop(guard);
    if arrived {
        resume_following(from);
    }
    if arrived || moved {
        notify(item);
    }
    arrived || moved
}

/// `from` closed `item`.
pub(super) fn left(item: ItemId, from: DeviceId) {
    let removed = peers()
        .as_mut()
        .and_then(|all| all.get_mut(&item))
        .is_some_and(|here| here.remove(&from).is_some());
    if removed {
        notify(item);
    }
}

/// `from` left the network: it is in no document any more. Returns the
/// documents it was in.
pub(super) fn gone(from: DeviceId) -> Vec<ItemId> {
    let items: Vec<ItemId> = peers()
        .as_mut()
        .map(|all| {
            all.iter_mut()
                .filter_map(|(item, here)| here.remove(&from).map(|_| *item))
                .collect()
        })
        .unwrap_or_default();
    for item in &items {
        notify(*item);
    }
    if following() == Some(from) {
        lose_sight(from);
    }
    items
}

/// Everyone else with `item` open.
#[must_use]
pub fn here(item: ItemId) -> Vec<Peer> {
    let guard = peers();
    let Some(here) = guard.as_ref().and_then(|all| all.get(&item)) else {
        return Vec::new();
    };
    let mut people: Vec<Peer> = here
        .iter()
        .filter(|(_, here)| here.seen.elapsed() < GONE_AFTER)
        .map(|(device, here)| Peer {
            id: device.0,
            name: here.name.clone(),
            color: color(*device),
            state: here.state.clone(),
        })
        .collect();
    drop(guard);
    people.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    people
}

/// Fires when someone arrives at, moves in or leaves `item`.
#[must_use]
pub fn watch(item: ItemId) -> flume::Receiver<()> {
    let (tx, rx) = flume::unbounded();
    WATCHERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((item, tx));
    ensure_ticker();
    rx
}

/// This device's presence in `item`: shared by everything showing it, so
/// the header and the face publish through the same one.
#[must_use]
pub fn open(item: ItemId) -> Arc<DocPresence> {
    let mut mine = MINE.lock().unwrap_or_else(PoisonError::into_inner);
    mine.retain(|(_, held)| held.strong_count() > 0);
    if let Some(held) = mine
        .iter()
        .find(|(id, _)| *id == item)
        .and_then(|(_, held)| held.upgrade())
    {
        return held;
    }
    let presence = Arc::new(DocPresence {
        item,
        state: Mutex::new(Vec::new()),
    });
    mine.push((item, Arc::downgrade(&presence)));
    drop(mine);
    super::live(item, &LiveMessage::Presence { state: Vec::new() });
    ensure_ticker();
    presence
}

/// This device's place in one document.
pub struct DocPresence {
    item: ItemId,
    state: Mutex<Vec<u8>>,
}

impl std::fmt::Debug for DocPresence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocPresence")
            .field("item", &self.item)
            .finish_non_exhaustive()
    }
}

impl DocPresence {
    fn repeat(&self) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        super::live(self.item, &LiveMessage::Presence { state });
    }
}

impl Presence for DocPresence {
    fn publish(&self, state: Vec<u8>) {
        let mut held = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if *held == state {
            return;
        }
        held.clone_from(&state);
        drop(held);
        super::live(self.item, &LiveMessage::Presence { state });
    }

    fn peers(&self) -> Vec<Peer> {
        here(self.item)
    }

    fn changes(&self) -> flume::Receiver<()> {
        watch(self.item)
    }

    fn followed(&self) -> Option<Peer> {
        let device = following()?;
        here(self.item).into_iter().find(|peer| peer.id == device.0)
    }

    fn stop_following(&self) {
        follow(None);
    }
}

impl Drop for DocPresence {
    fn drop(&mut self) {
        super::live(self.item, &LiveMessage::Left);
    }
}

/// One thread repeats this device's presence and notices who left.
fn ensure_ticker() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("notein-presence".to_owned())
            .spawn(|| {
                loop {
                    std::thread::sleep(HEARTBEAT);
                    let open: Vec<Arc<DocPresence>> = MINE
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .iter()
                        .filter_map(|(_, held)| held.upgrade())
                        .collect();
                    for presence in open {
                        presence.repeat();
                    }
                    sweep();
                    // A peer that died mid-stroke sends nothing more; its
                    // ghost must not wait for someone else to draw.
                    for board in crate::active_boards::all() {
                        crate::canvas::lock(&board).expire_hands();
                    }
                }
            });
        if let Err(err) = spawned {
            log::error!("presence thread: {err}");
        }
    });
}

/// Forget peers gone quiet, telling whoever watches their documents.
fn sweep() {
    let mut gone = Vec::new();
    if let Some(all) = peers().as_mut() {
        for (item, here) in all.iter_mut() {
            let before = here.len();
            here.retain(|_, here| here.seen.elapsed() < GONE_AFTER);
            if here.len() != before {
                gone.push(*item);
            }
        }
        all.retain(|_, here| !here.is_empty());
    }
    for item in gone {
        notify(item);
    }
    // Following someone who left everywhere ends.
    if let Some(device) = following()
        && whereabouts(device).is_none()
    {
        lose_sight(device);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that follow someone share one global; take turns.
    fn serial() -> MutexGuard<'static, ()> {
        static SERIAL: Mutex<()> = Mutex::new(());
        SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[test]
    fn following_resumes_when_the_followed_peer_comes_back() {
        let _serial = serial();
        let notes = ItemId([10; 16]);
        let dani = DeviceId([4; 32]);
        seen(notes, dani, "Dani", Some(vec![1]));
        follow(Some(dani));
        gone(dani);
        assert_eq!(following(), None);
        seen(notes, dani, "Dani", Some(vec![1]));
        assert_eq!(following(), Some(dani));
        // Stopping by hand is not undone by a return.
        follow(None);
        gone(dani);
        seen(notes, dani, "Dani", Some(vec![1]));
        assert_eq!(following(), None);
        left(notes, dani);
    }

    #[test]
    fn peers_arrive_move_and_leave() {
        let item = ItemId([9; 16]);
        let ana = DeviceId([1; 32]);
        let changes = watch(item);
        seen(item, ana, "Ana", None);
        assert!(changes.try_recv().is_ok());
        assert_eq!(here(item)[0].name, "Ana");
        seen(item, ana, "Ana", Some(vec![4]));
        assert!(changes.try_recv().is_ok());
        assert_eq!(here(item)[0].state, vec![4]);
        // Saying the same again changes nothing.
        assert!(!seen(item, ana, "Ana", Some(vec![4])));
        left(item, ana);
        assert!(here(item).is_empty());
    }

    #[test]
    fn following_goes_where_they_last_moved() {
        let _serial = serial();
        let (notes, bible) = (ItemId([5; 16]), ItemId([6; 16]));
        let beto = DeviceId([2; 32]);
        seen(notes, beto, "Beto", Some(vec![1]));
        std::thread::sleep(Duration::from_millis(5));
        seen(bible, beto, "Beto", Some(vec![2]));
        follow(Some(beto));
        assert_eq!(following(), Some(beto));
        assert_eq!(whereabouts(beto).map(|(item, _)| item), Some(bible));
        let mine = open(bible);
        assert_eq!(mine.followed().map(|peer| peer.state), Some(vec![2]));
        assert!(open(notes).followed().is_some());
        left(notes, beto);
        left(bible, beto);
        sweep();
        assert_eq!(following(), None);
    }

    #[test]
    fn a_peer_off_the_network_leaves_every_document_at_once() {
        let (notes, board) = (ItemId([7; 16]), ItemId([8; 16]));
        let caro = DeviceId([3; 32]);
        seen(notes, caro, "Caro", Some(vec![1]));
        seen(board, caro, "Caro", Some(vec![2]));
        let changes = watch(board);
        let mut was_in = gone(caro);
        was_in.sort_by_key(|item| item.0);
        assert_eq!(was_in, vec![notes, board]);
        assert!(here(notes).is_empty() && here(board).is_empty());
        assert!(changes.try_recv().is_ok());
        assert!(gone(caro).is_empty());
    }
}
