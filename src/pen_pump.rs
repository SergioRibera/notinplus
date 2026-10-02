//! Background pump that wires the istmo-pen [`PenClient`] stream into
//! the shared [`crate::canvas::Board`].
//!
//! Called once at app startup. Acquires the client for `WINDOW_ID`,
//! subscribes to the event + hover streams, and dispatches each sample
//! to the board on a dedicated thread. Failure to acquire logs and
//! returns — the app still runs, driven by mouse events.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use flume::{Receiver, Sender};
use istmo::{StreamItem, TypedStream};
use istmo_pen::{PenClient, PenConfig, PenEvent, PenHoverEvent, PenSample};

use crate::brush::InkPoint;
use crate::canvas::{Board, InputMode};

/// Latest known pen-hover sample published by the backend.
///
/// `None` on [`PenHoverEvent::ProximityLeave`]; `Some(...)` after any
/// enter / move. Coordinates are in surface-logical units (points /
/// dp) relative to the tracked window origin — the same space palette
/// button areas resolve to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoverPoint {
    pub x: f32,
    pub y: f32,
    /// Distance above the surface reported by the platform. Zero on
    /// backends that do not surface height; growing values indicate
    /// the tool pulling away.
    pub z_offset: f32,
}

/// Broadcast-shaped bridge between [`pump_hover`] and any UI thread
/// that wants a reactive view on pen proximity. Bounded so a stalled
/// consumer cannot back-pressure the pen event pump.
#[derive(Debug)]
struct HoverBridge {
    tx: Sender<Option<HoverPoint>>,
    rx: Receiver<Option<HoverPoint>>,
}

static HOVER_BRIDGE: OnceLock<HoverBridge> = OnceLock::new();

/// Last surface-pixel position the pen was observed at (contact or
/// hover), timestamped. Read by `canvas.rs` to drop freya touch events
/// that are really pen artifacts — some platforms deliver stylus
/// contact as both an istmo-pen sample AND a `TouchEventData` with the
/// same coords, which would otherwise fire the finger-pan path and
/// steal the stroke.
static PEN_SURFACE_POS: Mutex<Option<(f32, f32, Instant)>> = Mutex::new(None);

/// Freshness window for [`pen_screen_pos_fresh`]. Comfortably covers
/// the gap between a pen sample dispatch and the matching freya touch
/// event on the UI thread; short enough that a lifted pen stops
/// swallowing real finger taps within a frame or two.
const PEN_POS_FRESHNESS: Duration = Duration::from_millis(300);

/// Radius (surface px) around a fresh pen position within which an
/// incoming touch_down is treated as a pen artifact and dropped.
pub const PEN_TOUCH_FILTER_RADIUS: f32 = 48.0;

fn record_pen_pos(x: f32, y: f32) {
    if let Ok(mut g) = PEN_SURFACE_POS.lock() {
        *g = Some((x, y, Instant::now()));
    }
}

fn clear_pen_pos() {
    if let Ok(mut g) = PEN_SURFACE_POS.lock() {
        *g = None;
    }
}

/// Most recent pen position, if observed within the freshness window.
/// Returns `None` once the pen has been lifted / idle long enough that
/// a touch at the same coord is unlikely to be a stylus artifact.
#[must_use]
pub fn pen_screen_pos_fresh() -> Option<(f32, f32)> {
    let g = PEN_SURFACE_POS.lock().ok()?;
    let (x, y, t) = (*g)?;
    (t.elapsed() < PEN_POS_FRESHNESS).then_some((x, y))
}

/// `true` when a fresh pen sample sits within `radius` surface pixels
/// of `(sx, sy)` — the heuristic the canvas uses to drop stylus events
/// that leak through as touch on platforms without native tool-type
/// disambiguation.
#[must_use]
pub fn is_pen_near(sx: f32, sy: f32, radius: f32) -> bool {
    pen_screen_pos_fresh().is_some_and(|(px, py)| (px - sx).hypot(py - sy) < radius)
}

fn hover_bridge() -> &'static HoverBridge {
    HOVER_BRIDGE.get_or_init(|| {
        let (tx, rx) = flume::bounded(32);
        HoverBridge { tx, rx }
    })
}

/// Subscribe to pen-hover updates. Every call returns a clone of the
/// same receiver — the UI drains this from a single async task.
#[must_use]
pub fn hover_receiver() -> Receiver<Option<HoverPoint>> {
    hover_bridge().rx.clone()
}

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
    let mut dt = DtTracker::default();
    loop {
        match stream.recv() {
            Ok(StreamItem::Event(event)) => apply(event, board, &mut dt),
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
    let bridge = hover_bridge();
    while let Ok(StreamItem::Event(event)) = stream.recv() {
        let payload = match event {
            PenHoverEvent::ProximityEnter(sample) | PenHoverEvent::Move(sample) => {
                // Hover often precedes contact — record so the first
                // touch_down after proximity enters already has a
                // fresh pen position to compare against.
                record_pen_pos(sample.x, sample.y);
                Some(HoverPoint {
                    x: sample.x,
                    y: sample.y,
                    z_offset: sample.z_offset,
                })
            }
            PenHoverEvent::ProximityLeave => {
                clear_pen_pos();
                None
            }
        };
        // Bounded channel: drop-oldest on full so a stalled consumer
        // never back-pressures the pen backend. Hover samples are
        // idempotent (only the latest matters), so a dropped
        // intermediate is invisible in the UI.
        if bridge.tx.try_send(payload).is_err() {
            let _ = bridge.rx.try_recv();
            let _ = bridge.tx.try_send(payload);
        }
    }
}

/// Per-stroke previous-sample timestamp. `Down` resets it; each
/// subsequent sample computes `dt_us = ts - prev` and advances `prev`.
/// Deltas exceeding `u16::MAX` (65 ms) clamp — a gap that large is
/// almost always a lift-off recovery, and downstream velocity code
/// treats saturated deltas as "no meaningful continuity" anyway.
#[derive(Default)]
struct DtTracker {
    prev_us: Option<u64>,
}

impl DtTracker {
    const fn reset(&mut self, ts_us: u64) -> u16 {
        self.prev_us = Some(ts_us);
        0
    }

    fn advance(&mut self, ts_us: u64) -> u16 {
        let dt = self.prev_us.map_or(0, |p| ts_us.saturating_sub(p));
        self.prev_us = Some(ts_us);
        u16::try_from(dt).unwrap_or(u16::MAX)
    }

    const fn clear(&mut self) {
        self.prev_us = None;
    }
}

fn apply(event: PenEvent, board: &Arc<Mutex<Board>>, dt: &mut DtTracker) {
    // Compute the InkPoint(s) before touching the mutex, then take the
    // lock only for the actual board mutation. Keeps the guard's live
    // scope tight so the pump thread doesn't hold it during quantise +
    // trig work. Dispatch flips at the top on `InputMode` so Pan reuses
    // the same event stream to drive the viewport instead of a stroke.
    match event {
        PenEvent::Down(sample) => {
            record_pen_pos(sample.x, sample.y);
            let d = dt.reset(sample.timestamp_us);
            let point = point_from_sample(&sample, d);
            let mut guard = lock_board(board);
            match guard.input_mode() {
                InputMode::Draw => guard.begin_screen(point),
                InputMode::Pan => guard.pan_begin(sample.x, sample.y),
            }
        }
        PenEvent::Move(m) => {
            record_pen_pos(m.sample.x, m.sample.y);
            let mut guard = lock_board(board);
            match guard.input_mode() {
                InputMode::Draw => {
                    let mut points = Vec::with_capacity(m.coalesced.len() + 1);
                    for c in &m.coalesced {
                        let d = dt.advance(c.timestamp_us);
                        points.push(point_from_sample(c, d));
                    }
                    let d = dt.advance(m.sample.timestamp_us);
                    points.push(point_from_sample(&m.sample, d));
                    for p in points {
                        guard.extend_screen(p);
                    }
                }
                InputMode::Pan => {
                    // Only the final sample's screen position matters —
                    // intermediate coalesced samples would thrash the
                    // anchor without changing the net viewport delta.
                    let _ = dt.advance(m.sample.timestamp_us);
                    guard.pan_move(m.sample.x, m.sample.y);
                }
            }
        }
        PenEvent::Up(sample) => {
            record_pen_pos(sample.x, sample.y);
            let d = dt.advance(sample.timestamp_us);
            {
                let mut guard = lock_board(board);
                match guard.input_mode() {
                    InputMode::Draw => {
                        let point = point_from_sample(&sample, d);
                        guard.extend_screen(point);
                        guard.end();
                    }
                    InputMode::Pan => {
                        guard.pan_move(sample.x, sample.y);
                        guard.pan_end();
                    }
                }
            }
            dt.clear();
        }
        PenEvent::Cancel(_) => {
            clear_pen_pos();
            {
                let mut guard = lock_board(board);
                match guard.input_mode() {
                    InputMode::Draw => guard.cancel(),
                    InputMode::Pan => guard.pan_end(),
                }
            }
            dt.clear();
        }
        PenEvent::ButtonChanged(_) => {}
    }
}

fn lock_board(board: &Arc<Mutex<Board>>) -> std::sync::MutexGuard<'_, Board> {
    match board.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn point_from_sample(sample: &PenSample, dt_us: u16) -> InkPoint {
    // Tilt vector magnitude — already normalised on the backend side to
    // 0.0..=1.0, but clamp defensively before quantising.
    let tilt = sample.tilt_x.hypot(sample.tilt_y);
    InkPoint::from_normalized(sample.x, sample.y, sample.pressure, tilt, dt_us)
}
