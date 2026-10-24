//! A shared session is over for this device: the owner ended it, threw
//! us out, or we walked away.
//!
//! The documents it brought stay on screen until the person acknowledges, then
//! the tabs close, the copies go and the shell returns home.

use std::sync::Mutex;

use notein_sync::Removal;

use crate::library::ItemId;

/// One session that ended for a guest.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SessionEnd {
    pub title: String,
    pub docs: Vec<ItemId>,
    pub reason: Removal,
}

static PENDING: Mutex<Vec<SessionEnd>> = Mutex::new(Vec::new());
static WAKERS: Mutex<Vec<flume::Sender<()>>> = Mutex::new(Vec::new());

fn pending() -> std::sync::MutexGuard<'static, Vec<SessionEnd>> {
    PENDING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Queue a session end for the UI. Callable from any thread.
pub(crate) fn announce(end: SessionEnd) {
    pending().push(end);
    WAKERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .retain(|tx| tx.send(()).is_ok());
}

/// Watch for session ends from the app root. A session we left
/// ourselves resolves at once; one closed on us asks first.
pub fn listen() {
    let (tx, rx) = flume::unbounded();
    WAKERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(tx);
    freya::prelude::spawn_forever(async move {
        loop {
            let ends: Vec<SessionEnd> = pending().drain(..).collect();
            for end in ends {
                match end.reason {
                    Removal::Left => resolve(end),
                    Removal::Ended | Removal::Expelled => crate::screens::collab::session_ended(&end),
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
    freya::prelude::spawn_forever(async move {
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
