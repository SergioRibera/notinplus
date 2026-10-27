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

struct Here {
    name: String,
    state: Vec<u8>,
    seen: Instant,
}

type Peers = HashMap<ItemId, HashMap<DeviceId, Here>>;

static PEERS: Mutex<Option<Peers>> = Mutex::new(None);
static WATCHERS: Mutex<Vec<(ItemId, flume::Sender<()>)>> = Mutex::new(Vec::new());
/// This device's presence per open document.
static MINE: Mutex<Vec<(ItemId, Weak<DocPresence>)>> = Mutex::new(Vec::new());

fn peers() -> MutexGuard<'static, Option<Peers>> {
    PEERS.lock().unwrap_or_else(PoisonError::into_inner)
}

fn notify(item: ItemId) {
    WATCHERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|(id, tx)| !tx.is_disconnected() && (*id != item || tx.send(()).is_ok()));
}

/// The color everything of `device` is drawn in.
#[must_use]
pub fn color(device: DeviceId) -> [u8; 3] {
    let hue = f32::from(device.0[0]) / 255.0 * 360.0;
    let color = freya::prelude::Color::from_hsv(hue, 0.62, 0.92);
    [color.r(), color.g(), color.b()]
}

/// `from` sent something about `item`: it is there. `state` replaces
/// what it last published, when given.
pub(super) fn seen(item: ItemId, from: DeviceId, name: &str, state: Option<Vec<u8>>) {
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
    });
    entry.seen = Instant::now();
    name.clone_into(&mut entry.name);
    let moved = state.is_some_and(|state| {
        let changed = entry.state != state;
        entry.state = state;
        changed
    });
    drop(guard);
    if arrived || moved {
        notify(item);
    }
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        seen(item, ana, "Ana", Some(vec![4]));
        assert!(changes.try_recv().is_err());
        left(item, ana);
        assert!(here(item).is_empty());
    }
}
