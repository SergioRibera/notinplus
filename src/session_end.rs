//! A shared session is over for this device: the owner ended it, threw
//! us out, or we walked away.
//!
//! The documents it brought stay on screen until the person acknowledges, then
//! the tabs close, the copies go and the shell returns home. Until then
//! the end is kept on disk, so a restart asks again instead of leaving
//! the copies behind without a word.

use std::path::PathBuf;
use std::sync::Mutex;

use notein_sync::Removal;

use crate::library::ItemId;

const FILE: &str = "session_ends.bin";

/// One session that ended for a guest.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Eq, Debug)]
pub struct SessionEnd {
    pub title: String,
    pub docs: Vec<ItemId>,
    pub reason: Removal,
}

/// Ends waiting for the UI to pick them up.
static PENDING: Mutex<Vec<SessionEnd>> = Mutex::new(Vec::new());
/// Ends announced and not resolved yet, as on disk; `None` until read.
static UNRESOLVED: Mutex<Option<Vec<SessionEnd>>> = Mutex::new(None);
static WAKERS: Mutex<Vec<flume::Sender<()>>> = Mutex::new(Vec::new());

fn locked<T>(slot: &'static Mutex<T>) -> std::sync::MutexGuard<'static, T> {
    slot.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Run `f` on the unresolved ends, reading them from disk the first
/// time; those left by an earlier run are queued for the UI again.
fn with_unresolved<T>(f: impl FnOnce(&mut Vec<SessionEnd>) -> T) -> T {
    let mut slot = locked(&UNRESOLVED);
    let ends = slot.get_or_insert_with(|| {
        let earlier = load();
        locked(&PENDING).extend(earlier.iter().cloned());
        earlier
    });
    let out = f(ends);
    drop(slot);
    out
}

fn path() -> PathBuf {
    crate::library::bodies::root_dir().join(FILE)
}

fn load() -> Vec<SessionEnd> {
    std::fs::read(path())
        .ok()
        .and_then(|bytes| {
            bincode::decode_from_slice(&bytes, bincode::config::standard())
                .ok()
                .map(|(ends, _)| ends)
        })
        .unwrap_or_default()
}

fn store(ends: &[SessionEnd]) {
    let path = path();
    let written = if ends.is_empty() {
        match std::fs::remove_file(&path) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    } else {
        match bincode::encode_to_vec(ends, bincode::config::standard()) {
            Ok(bytes) => std::fs::write(&path, bytes),
            Err(err) => {
                log::warn!("session end: encode: {err}");
                return;
            }
        }
    };
    if let Err(err) = written {
        log::warn!("session end: save {}: {err}", path.display());
    }
}

/// Queue a session end for the UI. Callable from any thread.
pub(crate) fn announce(end: SessionEnd) {
    with_unresolved(|ends| {
        ends.push(end.clone());
        store(ends);
    });
    locked(&PENDING).push(end);
    wake();
}

fn wake() {
    locked(&WAKERS).retain(|tx| tx.send(()).is_ok());
}

/// Watch for session ends from the app root. A session we left
/// ourselves resolves at once; one closed on us asks first. Ends left
/// unanswered by an earlier run ask again.
pub fn listen() {
    with_unresolved(|_| ());
    let (tx, rx) = flume::unbounded();
    locked(&WAKERS).push(tx);
    freya::prelude::spawn_in_window(async move {
        loop {
            let ends: Vec<SessionEnd> = locked(&PENDING).drain(..).collect();
            for end in ends {
                match end.reason {
                    Removal::Left => resolve(end),
                    Removal::Ended | Removal::Expelled => {
                        crate::screens::collab::session_ended(&end);
                    }
                }
            }
            if rx.recv_async().await.is_err() {
                break;
            }
        }
    });
}

/// Close the session's tabs, drop its copies and go home.
pub fn resolve(end: SessionEnd) {
    with_unresolved(|ends| {
        if let Some(at) = ends.iter().position(|e| *e == end) {
            ends.remove(at);
            store(ends);
        }
    });
    freya::prelude::spawn_in_window(async move {
        let Ok(handle) = crate::library::shared().await else {
            log::warn!("session end: library unavailable");
            return;
        };
        {
            let mut lib = handle.lock().await;
            if let Err(err) = lib.close_item_tabs(&end.docs).await {
                log::warn!("session end: close tabs: {err}");
            }
            for item in &end.docs {
                if let Err(err) = lib.forget_item(*item).await {
                    log::warn!("session end: drop {item}: {err}");
                }
            }
        }
        crate::space::notify_changed();
        let _ = crate::app::global_router().push(crate::route::Route::Home);
    });
}
