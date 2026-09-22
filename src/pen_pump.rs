//! Background pump that wires the istmo-pen [`PenClient`] stream into
//! the shared [`crate::canvas::Board`].
//!
//! Called once at app startup. Acquires the client for `WINDOW_ID`,
//! subscribes to the event + hover streams, and dispatches each sample
//! to the board on a dedicated thread. Failure to acquire logs and
//! returns — the app still runs, driven by mouse events.

use std::sync::{Arc, Mutex};

use istmo::{StreamItem, TypedStream};
use istmo_pen::{PenClient, PenConfig, PenEvent, PenHoverEvent, PenSample};

use crate::brush::{InkPoint, Pressure};
use crate::canvas::Board;

/// Kick off the pen pump for `window_id`.
///
/// Spawns a thread that acquires the client and, on success, drains
/// the event and hover streams into `board`. Client ownership stays on
/// that thread so its `Drop` runs only when the streams close.
pub fn spawn(window_id: u64, board: Arc<Mutex<Board>>) {
    std::thread::spawn(move || {
        let client = match pollster::block_on(PenClient::acquire_with(PenConfig::new(window_id))) {
            Ok(c) => c,
            Err(err) => {
                log::warn!("pen client unavailable: {err:?}");
                return;
            }
        };

        let (events, hover) = match (client.events(), client.hover()) {
            (Ok(e), Ok(h)) => (e, h),
            (Err(err), _) | (_, Err(err)) => {
                log::warn!("pen streams unavailable: {err:?}");
                return;
            }
        };

        // Drain hover on a helper thread; the event pump keeps this
        // thread (and therefore the client) alive until the runtime
        // closes the stream.
        std::thread::spawn(move || pump_hover(&hover));
        pump_events(&events, &board);
        drop(client);
    });
}

fn pump_events(stream: &TypedStream<PenEvent, ()>, board: &Arc<Mutex<Board>>) {
    loop {
        match stream.recv() {
            Ok(StreamItem::Event(event)) => apply(event, board),
            Ok(StreamItem::Completed | StreamItem::Cancelled) => break,
            Ok(StreamItem::Failed(err)) => {
                log::warn!("pen event stream failed: {err:?}");
                break;
            }
            Err(err) => {
                log::warn!("pen event recv error: {err:?}");
                break;
            }
        }
    }
}

fn pump_hover(stream: &TypedStream<PenHoverEvent, ()>) {
    // Hover isn't drawn yet, but the receiver must be drained so the
    // publisher's channel doesn't back up on platforms that emit hover
    // frequently.
    while let Ok(StreamItem::Event(_)) = stream.recv() {}
}

fn apply(event: PenEvent, board: &Arc<Mutex<Board>>) {
    let mut guard = match board.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    match event {
        PenEvent::Down(sample) => guard.begin(point_from_sample(&sample)),
        PenEvent::Move(m) => {
            for c in &m.coalesced {
                guard.extend(point_from_sample(c));
            }
            guard.extend(point_from_sample(&m.sample));
        }
        PenEvent::Up(sample) => {
            guard.extend(point_from_sample(&sample));
            guard.end();
        }
        PenEvent::Cancel(_) => guard.cancel(),
        PenEvent::ButtonChanged(_) => {}
    }
}

fn point_from_sample(sample: &PenSample) -> InkPoint {
    let tilt = sample.tilt_x.hypot(sample.tilt_y);
    InkPoint::new(sample.x, sample.y, Pressure::new(sample.pressure), tilt)
}
