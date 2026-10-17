//! Shell-side registry for pending Bible-chip jumps.
//!
//! A `SourceRef::Plugin { plugin_id = "io.notein.bible", anchor }` chip
//! click decodes the anchor, queues a jump keyed on the target
//! [`ItemId`] here, and asks the router to open that item's
//! [`crate::route::Route::BibleView`]. The viewer polls the registry
//! on mount, applies the jump, and clears the entry — mirrors the
//! `active_pdf_sources::pulse` shape used for PDF chips.
//!
//! Lives shell-side (not inside the Bible plugin) because routing +
//! library lookup are host concerns; the plugin only knows how to
//! decode its own anchor payload.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use crate::library::ItemId;

/// Notification payload broadcast after a jump is queued. Carries the
/// target [`ItemId`] so subscribers filter cheaply without hitting the
/// registry mutex on every unrelated chip tap.
type Notification = ItemId;

type Subscribers = Mutex<Vec<flume::Sender<Notification>>>;

fn subscribers() -> &'static Subscribers {
    static S: OnceLock<Subscribers> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Vec::new()))
}

fn lock_subs() -> MutexGuard<'static, Vec<flume::Sender<Notification>>> {
    match subscribers().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Pending jump a BibleView mount should apply once its state hooks
/// have run. `bible_id` is the translation the anchor originated
/// from — the viewer uses it to switch translations before jumping
/// when the open item carries a different one.
#[derive(Clone, Debug)]
pub struct PendingJump {
    pub bible_id: String,
    pub book_id: String,
    pub chapter: u16,
    pub verse: u16,
}

type Registry = Mutex<HashMap<ItemId, PendingJump>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock() -> MutexGuard<'static, HashMap<ItemId, PendingJump>> {
    match registry().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Queue a jump for `item`. Overwrites any prior pending jump — the
/// most recent chip-tap wins. Fans out a notification to every
/// active subscriber so a live Bible viewer on the same item can
/// pick the jump up without waiting for a remount. Dead subscribers
/// (receiver dropped) are reaped lazily on the next broadcast.
pub fn queue(item: ItemId, jump: PendingJump) {
    lock().insert(item, jump);
    let mut subs = lock_subs();
    subs.retain(|tx| tx.try_send(item).is_ok() || !tx.is_disconnected());
}

/// Register a subscriber that fires once per [`queue`] call with the
/// target item id. Callers filter the stream by comparing the
/// payload against the item they own. Dropping the returned receiver
/// cleans the sender slot on the next broadcast.
#[must_use]
pub fn subscribe() -> flume::Receiver<Notification> {
    let (tx, rx) = flume::bounded(8);
    lock_subs().push(tx);
    rx
}

/// Pop the pending jump for `item`, if any. Caller is expected to
/// apply it; a drop without applying loses the jump by design — the
/// user can re-click the chip.
#[must_use]
pub fn take(item: ItemId) -> Option<PendingJump> {
    lock().remove(&item)
}

/// Non-destructive peek — handy for callers that want to decide
/// whether to call [`take`] based on current state.
#[must_use]
pub fn peek(item: ItemId) -> Option<PendingJump> {
    lock().get(&item).cloned()
}
