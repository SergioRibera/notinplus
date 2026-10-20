//! Documents open in a face that is not a canvas board (a note, a
//! plugin page). The replica merges remote changes into these live
//! copies, exactly as it does into open boards, and wakes the face.

use std::sync::{Arc, Mutex};

use crate::doc::Doc;
use crate::library::ItemId;

static DOCS: Mutex<Vec<(ItemId, Arc<Mutex<Doc>>)>> = Mutex::new(Vec::new());
static WATCHERS: Mutex<Vec<(ItemId, flume::Sender<()>)>> = Mutex::new(Vec::new());

fn docs() -> std::sync::MutexGuard<'static, Vec<(ItemId, Arc<Mutex<Doc>>)>> {
    DOCS.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn watchers() -> std::sync::MutexGuard<'static, Vec<(ItemId, flume::Sender<()>)>> {
    WATCHERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Lock a shared document, recovering from a poisoned mutex.
pub fn lock(doc: &Arc<Mutex<Doc>>) -> std::sync::MutexGuard<'_, Doc> {
    doc.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Publish `doc` as the live copy of `item`. A later registration for
/// the same item replaces an earlier one.
pub fn register(item: ItemId, doc: Arc<Mutex<Doc>>) {
    let mut list = docs();
    match list.iter_mut().find(|(i, _)| *i == item) {
        Some((_, held)) => *held = doc,
        None => list.push((item, doc)),
    }
}

/// Drop the registration if `doc` is still the live copy of `item`.
pub fn unregister(item: ItemId, doc: &Arc<Mutex<Doc>>) {
    docs().retain(|(i, held)| *i != item || !Arc::ptr_eq(held, doc));
}

/// The live copy of `item`, if a face holds one.
#[must_use]
pub fn get(item: ItemId) -> Option<Arc<Mutex<Doc>>> {
    docs()
        .iter()
        .find(|(i, _)| *i == item)
        .map(|(_, doc)| Arc::clone(doc))
}

/// Wake whenever remote changes land in the live copy of `item`.
#[must_use]
pub fn watch(item: ItemId) -> flume::Receiver<()> {
    let (tx, rx) = flume::unbounded();
    watchers().push((item, tx));
    rx
}

/// Remote changes landed in the live copy of `item`.
pub(crate) fn notify(item: ItemId) {
    watchers().retain(|(i, tx)| *i != item || tx.send(()).is_ok());
}
