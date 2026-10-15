//! Background pump that wires the istmo-pen [`PenClient`] stream into
//! the shared [`crate::canvas::Board`].
//!
//! Called once at app startup. Acquires the client for `WINDOW_ID`,
//! subscribes to the event + hover streams, and dispatches each sample
//! to the board on a dedicated thread. Failure to acquire logs and
//! returns — the app still runs, driven by mouse events.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
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

/// On-screen rect occupied by a mounted canvas, in surface-logical
/// pixels. Published by `drawing_surface` from its `on_sized` handler so
/// the pump can route pen samples (which arrive in window coordinates)
/// to the canvas they land on and translate them into canvas-local
/// space before dispatch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceRect {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

impl SurfaceRect {
    #[must_use]
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.min_x && x < self.max_x && y >= self.min_y && y < self.max_y
    }
}

/// Opaque per-surface identifier. Each `drawing_surface` mount
/// allocates one via [`allocate_canvas_surface_id`] and uses it as the
/// key for register / unregister calls — multiple surfaces can share
/// the same `Board` (DocLinked split view) without clobbering each
/// other's rect on update.
pub type CanvasSurfaceId = u64;

static NEXT_SURFACE_ID: AtomicU64 = AtomicU64::new(1);

/// Monotonically increasing id generator. The id space is global, so
/// surfaces coming from different windows / crates never collide.
#[must_use]
pub fn allocate_canvas_surface_id() -> CanvasSurfaceId {
    NEXT_SURFACE_ID.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone)]
struct SurfaceEntry {
    id: CanvasSurfaceId,
    board: Weak<Mutex<Board>>,
    rect: SurfaceRect,
}

/// Every mounted canvas surface. Pen samples arrive in window-wide
/// coordinates (`PenCaptureView` + iOS `UIEvent` both report
/// decor-view / window space), so the pump hit-tests the sample against
/// every registered rect and routes to the owning board. Split-pane
/// layouts hold several entries at once — otherwise only the pane the
/// pen is physically over receives the stroke. Entries are keyed by
/// [`CanvasSurfaceId`] rather than board identity so two surfaces
/// sharing a single `Board` (shared-canvas split) can coexist with
/// distinct rects.
static CANVAS_SURFACES: Mutex<Vec<SurfaceEntry>> = Mutex::new(Vec::new());

/// Board that latched the current stroke on `Down`. Move / Up / Cancel
/// use this instead of re-hit-testing — a mid-stroke sample that
/// crosses the pane divider should still land on the pane the stroke
/// started on. Cleared on Up / Cancel.
static ACTIVE_STROKE: Mutex<Option<(Weak<Mutex<Board>>, SurfaceRect)>> = Mutex::new(None);

/// Register or update the on-screen rect for the surface identified by
/// `id`. Called from the surface's `on_sized` handler every time the
/// layout engine resolves a new area — mount, resize, split-pane
/// divider drag.
pub fn register_canvas_surface(id: CanvasSurfaceId, board: &Arc<Mutex<Board>>, rect: SurfaceRect) {
    let Ok(mut g) = CANVAS_SURFACES.lock() else {
        return;
    };
    g.retain(|e| e.board.upgrade().is_some());
    for entry in g.iter_mut() {
        if entry.id == id {
            entry.board = Arc::downgrade(board);
            entry.rect = rect;
            return;
        }
    }
    g.push(SurfaceEntry {
        id,
        board: Arc::downgrade(board),
        rect,
    });
}

/// Drop the registration for `id` if present. Called on unmount so
/// the hit-test registry does not grow without bound across route
/// changes. Siblings sharing the same `Board` retain their own entries.
pub fn unregister_canvas_surface(id: CanvasSurfaceId) {
    if let Ok(mut g) = CANVAS_SURFACES.lock() {
        g.retain(|e| e.id != id);
    }
}

/// Window-coord hit-test. Walks the registry in reverse (last-mounted
/// first) so an overlay canvas (future PiP) wins over the base layer.
fn hit_canvas(x: f32, y: f32) -> Option<(Arc<Mutex<Board>>, SurfaceRect)> {
    let g = CANVAS_SURFACES.lock().ok()?;
    for entry in g.iter().rev() {
        if let Some(b) = entry.board.upgrade()
            && entry.rect.contains(x, y)
        {
            return Some((b, entry.rect));
        }
    }
    None
}

fn active_stroke() -> Option<(Arc<Mutex<Board>>, SurfaceRect)> {
    let g = ACTIVE_STROKE.lock().ok()?;
    let (w, r) = g.as_ref()?;
    Some((w.upgrade()?, *r))
}

fn set_active_stroke(board: &Arc<Mutex<Board>>, rect: SurfaceRect) {
    if let Ok(mut g) = ACTIVE_STROKE.lock() {
        *g = Some((Arc::downgrade(board), rect));
    }
}

fn clear_active_stroke() {
    if let Ok(mut g) = ACTIVE_STROKE.lock() {
        *g = None;
    }
}

/// Resolve the board whose rect the hover sample is over. Used by
/// `pump_hover` so the pointer-overlay disc shows up in the correct
/// pane in split-pane layouts.
fn hover_target(x: f32, y: f32) -> Option<(Arc<Mutex<Board>>, SurfaceRect)> {
    hit_canvas(x, y)
}

/// Kick off the pen pump for `window_id`.
///
/// Spawns a thread that acquires the client and, on success, drains
/// the event and hover streams. Each `Down` sample hit-tests against
/// every canvas registered via [`register_canvas_surface`] and
/// dispatches the stroke to whichever one it lands on; subsequent
/// `Move` / `Up` / `Cancel` events follow the latched target. Client
/// ownership stays on that thread so its `Drop` runs only when the
/// streams close.
pub fn spawn(window_id: u64) {
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
        pump_events(&events);
        drop(client);
    });
}

fn pump_events(stream: &TypedStream<PenEvent, ()>) {
    let mut dt = DtTracker::default();
    loop {
        match stream.recv() {
            Ok(StreamItem::Event(event)) => apply(event, &mut dt),
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
        let (payload, pointer) = match event {
            PenHoverEvent::ProximityEnter(sample) | PenHoverEvent::Move(sample) => {
                // Hover often precedes contact — record so the first
                // touch_down after proximity enters already has a
                // fresh pen position to compare against.
                record_pen_pos(sample.x, sample.y);
                (
                    Some(HoverPoint {
                        x: sample.x,
                        y: sample.y,
                        z_offset: sample.z_offset,
                    }),
                    Some((sample.x, sample.y)),
                )
            }
            PenHoverEvent::ProximityLeave => {
                clear_pen_pos();
                (None, None)
            }
        };
        // Pointer overlay follows hover regardless of whether the UI
        // task on the other side of `hover_bridge` picked up this
        // sample — pump_hover is the one place every proximity event
        // flows through. In split-pane layouts hover lands on
        // whichever canvas the pen is physically over; a mid-canvas
        // ProximityLeave clears the pointer on both candidates so a
        // stale disc never lingers on the pane the pen just left.
        match pointer {
            Some((sx, sy)) => {
                if let Some((board, rect)) = hover_target(sx, sy) {
                    lock_board(&board).set_pointer(Some((sx - rect.min_x, sy - rect.min_y)));
                }
            }
            None => {
                if let Ok(g) = CANVAS_SURFACES.lock() {
                    for entry in g.iter() {
                        if let Some(b) = entry.board.upgrade() {
                            lock_board(&b).set_pointer(None);
                        }
                    }
                }
            }
        }
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

fn apply(event: PenEvent, dt: &mut DtTracker) {
    // Pen samples arrive in window coordinates. `Down` hit-tests the
    // sample against every registered canvas surface and latches the
    // owning board for the stroke. Follow-up samples (`Move` / `Up` /
    // `Cancel`) read that latch rather than re-hit-testing — a stroke
    // that happens to cross a split-pane divider still belongs to the
    // pane it started on. `dt` clears on skip so the next real stroke
    // starts with a fresh baseline.
    match event {
        PenEvent::Down(sample) => {
            record_pen_pos(sample.x, sample.y);
            // Mask gate: a `Down` landing inside any chrome overlay
            // (palette, zoom HUD, bookmark card, back pill) must not
            // start a stroke on the canvas underneath. Rects are
            // published in window coords, same as `sample.x/y`.
            if crate::ui_mask::contains(sample.x, sample.y) {
                dt.clear();
                clear_active_stroke();
                return;
            }
            let Some((board, rect)) = hit_canvas(sample.x, sample.y) else {
                dt.clear();
                clear_active_stroke();
                return;
            };
            set_active_stroke(&board, rect);
            let (lx, ly) = (sample.x - rect.min_x, sample.y - rect.min_y);
            let d = dt.reset(sample.timestamp_us);
            let mut point = point_from_sample(&sample, d);
            point.x = lx;
            point.y = ly;
            let mut guard = lock_board(&board);
            guard.set_pointer(Some((lx, ly)));
            match guard.input_mode() {
                InputMode::Draw => guard.begin_screen(point),
                InputMode::Pan => guard.pan_begin(lx, ly),
                // Pen is deferred to the sibling overlay; pump is inert.
                InputMode::Select => {}
            }
        }
        PenEvent::Move(m) => {
            record_pen_pos(m.sample.x, m.sample.y);
            let Some((board, rect)) = active_stroke() else {
                dt.clear();
                return;
            };
            let (lx, ly) = (m.sample.x - rect.min_x, m.sample.y - rect.min_y);
            let mut guard = lock_board(&board);
            guard.set_pointer(Some((lx, ly)));
            match guard.input_mode() {
                InputMode::Draw => {
                    let mut points = Vec::with_capacity(m.coalesced.len() + 1);
                    for c in &m.coalesced {
                        let d = dt.advance(c.timestamp_us);
                        let mut p = point_from_sample(c, d);
                        p.x = c.x - rect.min_x;
                        p.y = c.y - rect.min_y;
                        points.push(p);
                    }
                    let d = dt.advance(m.sample.timestamp_us);
                    let mut p = point_from_sample(&m.sample, d);
                    p.x = lx;
                    p.y = ly;
                    points.push(p);
                    for p in points {
                        guard.extend_screen(p);
                    }
                }
                InputMode::Pan => {
                    // Only the final sample's screen position matters —
                    // intermediate coalesced samples would thrash the
                    // anchor without changing the net viewport delta.
                    let _ = dt.advance(m.sample.timestamp_us);
                    guard.pan_move(lx, ly);
                }
                InputMode::Select => {
                    let _ = dt.advance(m.sample.timestamp_us);
                }
            }
        }
        PenEvent::Up(sample) => {
            record_pen_pos(sample.x, sample.y);
            let Some((board, rect)) = active_stroke() else {
                dt.clear();
                return;
            };
            let (lx, ly) = (sample.x - rect.min_x, sample.y - rect.min_y);
            let d = dt.advance(sample.timestamp_us);
            {
                let mut guard = lock_board(&board);
                guard.set_pointer(Some((lx, ly)));
                match guard.input_mode() {
                    InputMode::Draw => {
                        let mut point = point_from_sample(&sample, d);
                        point.x = lx;
                        point.y = ly;
                        guard.extend_screen(point);
                        guard.end();
                    }
                    InputMode::Pan => {
                        guard.pan_move(lx, ly);
                        guard.pan_end();
                    }
                    InputMode::Select => {}
                }
            }
            dt.clear();
            clear_active_stroke();
        }
        PenEvent::Cancel(_) => {
            clear_pen_pos();
            if let Some((board, _)) = active_stroke() {
                let mut guard = lock_board(&board);
                guard.set_pointer(None);
                match guard.input_mode() {
                    InputMode::Draw => guard.cancel(),
                    InputMode::Pan => guard.pan_end(),
                    InputMode::Select => {}
                }
            }
            dt.clear();
            clear_active_stroke();
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
