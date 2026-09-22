//! Freya canvas widget + shared drawing board.
//!
//! [`Board`] owns the process-wide [`Doc`] (brush registry + committed
//! strokes), the in-progress stroke, and the vector eraser session.
//! It is `Arc<Mutex<Board>>` so the Freya render closure and the
//! [`crate::pen_pump`] background thread both touch the same buffer.
//!
//! Input is single-sourced from `istmo-pen` — freya mouse / touch
//! handlers are intentionally NOT wired here. Every sample comes from
//! [`crate::pen_pump`], which drains the platform pen backend (Android
//! `PenCaptureView`, iOS `PenCaptureView`, Linux libinput) and pushes
//! into [`Board`]. Canvas paints in element-local coordinates; the
//! layout keeps the drawing surface fullscreen at `(0, 0)` so pen
//! samples in decor-view / fullscreen coordinates land on the same
//! pixels the finger / stylus touched.
//!
//! Eraser input skips the paint path entirely — every sample runs a
//! circle-vs-polyline hit test against strokes pruned through
//! [`SpatialIndex`], and intersecting strokes are split at the circle
//! boundary. See [`Board::erase_at`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use flume::{Receiver, Sender};
use freya::prelude::*;
use freya_engine::prelude::{
    BlendMode, Color as SkColor, Paint, PaintStyle, Path, PathBuilder, PathDirection,
};

use crate::brush::{BrushKind, BrushPreset, InkPoint, SegmentMode, Stroke, StrokeStyle};
use crate::doc::Doc;
use crate::history::{EraseSession, HistoryOp};
use crate::spatial::SpatialIndex;

const SIZE_SCALE_MIN: f32 = 0.25;
const SIZE_SCALE_MAX: f32 = 4.0;

/// Minimum and maximum canvas zoom. Outside these the viewport clamps —
/// below 5% the strokes become unhittable, above 32× the cached Skia
/// paths' sub-pixel accuracy stops being meaningful.
const VIEWPORT_SCALE_MIN: f32 = 0.05;
const VIEWPORT_SCALE_MAX: f32 = 32.0;

/// 2D affine viewport: translate then uniform scale, mapping world
/// coordinates (what strokes are stored in) to surface-local pixels
/// (what the pen backend and pointer events deliver).
///
/// `screen = world * scale + translation`. Inverse is
/// `world = (screen - translation) / scale`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    pub tx: f32,
    pub ty: f32,
    pub scale: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            tx: 0.0,
            ty: 0.0,
            scale: 1.0,
        }
    }
}

impl Viewport {
    #[must_use]
    pub fn screen_to_world(&self, x: f32, y: f32) -> (f32, f32) {
        ((x - self.tx) / self.scale, (y - self.ty) / self.scale)
    }

    #[must_use]
    pub fn world_to_screen(&self, x: f32, y: f32) -> (f32, f32) {
        (x.mul_add(self.scale, self.tx), y.mul_add(self.scale, self.ty))
    }
}

/// Coalescing wakeup handle.
///
/// Board mutations `ping` it; the root component drains the receiver
/// and asks Freya's platform for a window redraw. Bounded(1) so bursts
/// collapse to one wakeup — the next frame will paint the freshest
/// state anyway.
#[derive(Debug, Clone)]
pub struct RedrawNotifier(Sender<()>);

impl RedrawNotifier {
    #[must_use]
    pub fn new() -> (Self, Receiver<()>) {
        let (tx, rx) = flume::bounded(1);
        (Self(tx), rx)
    }

    fn ping(&self) {
        let _ = self.0.try_send(());
    }
}

/// Immutable, UI-friendly summary of a layer.
///
/// Snapshotting keeps the UI code lock-free between reads — the panel
/// copies these once per render pass instead of holding a `Board`
/// guard across a layout traversal.
#[derive(Clone, Debug)]
pub struct LayerSnapshot {
    pub id: u32,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub opacity: f32,
    pub stroke_count: usize,
}

/// Shared drawing state. Cheap to clone the [`Arc`] into any handler.
///
/// `current_preset` is a **draft** — the palette owns it, the UI can
/// mutate it (size slider, colour picker). Registration into the doc
/// happens lazily at [`Self::begin`] so tunings compose into one
/// `BrushId` per stroke rather than one per keystroke.
///
/// `size_scales` remembers each tool's last-set thickness across
/// palette switches, matching Procreate's per-brush size memory.
#[derive(Debug)]
pub struct Board {
    doc: Doc,
    active: Option<Stroke>,
    /// Skia path holding the "frozen" prefix of the active stroke —
    /// every segment whose four Catmull-Rom control points are known
    /// and no longer subject to mirror extrapolation. The trailing two
    /// segments (last two samples) always live in a fresh per-paint
    /// tail path so their shape never pops between the mirror-based
    /// approximation and the eventual actual-neighbour rendering.
    active_builder: Option<PathBuilder>,
    /// Index of the next segment (`points[k] → points[k+1]`) yet to be
    /// emitted into `active_builder`. Advances by one for every new
    /// sample that pushes `points.len() >= active_next_segment + 4`.
    active_next_segment: usize,
    /// Id of the layer the in-flight stroke will commit into. Captured
    /// at [`Board::begin`] so mid-stroke `set_active_layer` calls don't
    /// redirect the commit to an unexpected layer.
    active_layer_at_begin: Option<u32>,
    current_preset: BrushPreset,
    current_color: [u8; 4],
    size_scales: HashMap<BrushKind, f32>,
    spatial: SpatialIndex,
    /// Pre-tessellated Skia path per committed stroke. Rebuilt only on
    /// commit / erase / load — repainting is a `HashMap` lookup plus a
    /// single `draw_path` call.
    cached_paths: HashMap<u32, Path>,
    /// `stroke_id → (layer_id, within-layer stroke index)`. Kills the
    /// O(N) `position` scan the eraser used to run per candidate hit
    /// and localises index shifting to the affected layer.
    stroke_index: HashMap<u32, (u32, usize)>,
    history: Vec<HistoryOp>,
    erase_session: Option<EraseSession>,
    notifier: Option<RedrawNotifier>,
    viewport: Viewport,
    /// Last surface-pixel cursor observed while a middle-drag pan is
    /// in flight. `Some` gates every `on_global_pointer_move` sample as
    /// a pan step; `None` means no pan currently active.
    pan_anchor: Option<(f32, f32)>,
    /// Live surface-pixel positions of every finger currently in
    /// contact. Populated from `on_touch_start` / `on_touch_move` and
    /// pruned by `on_touch_end` / `on_touch_cancel`. Two entries or
    /// more flips [`Self::gesture_active`] on and drives pinch/pan of
    /// the viewport; single-finger contacts are ignored here so pen
    /// input keeps its usual path.
    finger_positions: HashMap<u64, (f32, f32)>,
    gesture_baseline: Option<GestureBaseline>,
    /// True while two or more fingers are down. Suppresses stroke
    /// input coming through the `_screen` entrypoints so the viewport
    /// gesture doesn't share the pen path.
    gesture_active: bool,
}

/// Anchor snapshot for a pinch gesture — centroid + pair-distance of
/// the two lowest finger ids. Rebuilt after every touch sample so pan
/// and zoom accumulate incrementally.
#[derive(Debug, Clone, Copy)]
struct GestureBaseline {
    centroid: (f32, f32),
    distance: f32,
}

impl Default for Board {
    fn default() -> Self {
        let default_preset = BrushPreset::pen();
        Self {
            doc: Doc::default(),
            active: None,
            active_builder: None,
            active_next_segment: 0,
            active_layer_at_begin: None,
            current_preset: default_preset,
            current_color: default_preset.color,
            size_scales: HashMap::new(),
            spatial: SpatialIndex::new(),
            cached_paths: HashMap::new(),
            stroke_index: HashMap::new(),
            history: Vec::new(),
            erase_session: None,
            notifier: None,
            viewport: Viewport::default(),
            pan_anchor: None,
            finger_positions: HashMap::new(),
            gesture_baseline: None,
            gesture_active: false,
        }
    }
}

static SHARED: OnceLock<Arc<Mutex<Board>>> = OnceLock::new();

impl Board {
    /// Process-wide board. Freya's root component and [`crate::pen_pump`]
    /// share this same instance so pen input and the render callback
    /// touch the same stroke buffer.
    #[must_use]
    pub fn shared() -> Arc<Mutex<Self>> {
        Arc::clone(SHARED.get_or_init(|| Arc::new(Mutex::new(Self::default()))))
    }

    /// Install the wakeup handle used to request repaints after each
    /// mutation. Called once at startup by the root component. Safe to
    /// re-install (replaces the previous notifier).
    pub fn set_notifier(&mut self, notifier: RedrawNotifier) {
        self.notifier = Some(notifier);
    }

    fn notify(&self) {
        if let Some(n) = &self.notifier {
            n.ping();
        }
    }

    /// Adopt `preset` as the current tool. The remembered per-kind
    /// [`BrushPreset::size_scale`] is folded in, so switching to a tool
    /// the user has previously resized returns to that thickness.
    /// Stroke colour resets to the preset default.
    pub fn set_current_preset(&mut self, mut preset: BrushPreset) {
        if let Some(scale) = self.size_scales.get(&preset.kind).copied() {
            preset.size_scale = scale;
        }
        self.current_color = preset.color;
        self.current_preset = preset;
    }

    pub const fn set_current_color(&mut self, color: [u8; 4]) {
        self.current_color = color;
    }

    /// Update the active tool's size multiplier (clamped to a sane
    /// range). Persisted per-kind so the value survives palette
    /// switches.
    pub fn set_current_size(&mut self, scale: f32) {
        let clamped = scale.clamp(SIZE_SCALE_MIN, SIZE_SCALE_MAX);
        self.current_preset.size_scale = clamped;
        self.size_scales.insert(self.current_preset.kind, clamped);
    }

    #[must_use]
    pub const fn current_size(&self) -> f32 {
        self.current_preset.size_scale
    }

    #[must_use]
    pub const fn current_preset(&self) -> &BrushPreset {
        &self.current_preset
    }

    #[must_use]
    pub const fn current_kind(&self) -> BrushKind {
        self.current_preset.kind
    }

    #[must_use]
    pub const fn current_color(&self) -> [u8; 4] {
        self.current_color
    }

    #[must_use]
    pub const fn doc(&self) -> &Doc {
        &self.doc
    }

    #[must_use]
    pub const fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Adopt `viewport` wholesale. Scale is clamped to
    /// `[VIEWPORT_SCALE_MIN, VIEWPORT_SCALE_MAX]`; translation is left
    /// unrestricted (canvas is truly infinite).
    pub fn set_viewport(&mut self, mut viewport: Viewport) {
        viewport.scale = viewport.scale.clamp(VIEWPORT_SCALE_MIN, VIEWPORT_SCALE_MAX);
        if !viewport.scale.is_finite() {
            viewport.scale = 1.0;
        }
        if !viewport.tx.is_finite() {
            viewport.tx = 0.0;
        }
        if !viewport.ty.is_finite() {
            viewport.ty = 0.0;
        }
        self.viewport = viewport;
        self.notify();
    }

    pub fn reset_viewport(&mut self) {
        self.set_viewport(Viewport::default());
    }

    /// Translate the viewport by `(dx, dy)` in surface pixels.
    pub fn viewport_pan(&mut self, dx: f32, dy: f32) {
        if !dx.is_finite() || !dy.is_finite() {
            return;
        }
        self.viewport.tx += dx;
        self.viewport.ty += dy;
        self.notify();
    }

    /// Multiply the current zoom by `factor`, keeping the world point
    /// currently under the surface-pixel cursor `(cx, cy)` pinned.
    /// Scale clamps to `[VIEWPORT_SCALE_MIN, VIEWPORT_SCALE_MAX]`; a
    /// no-op factor (already at the clamp boundary) skips the notify.
    pub fn viewport_zoom_at(&mut self, cx: f32, cy: f32, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let old_scale = self.viewport.scale;
        let raw_new = (old_scale * factor).clamp(VIEWPORT_SCALE_MIN, VIEWPORT_SCALE_MAX);
        // Snap to exactly 1.0 whenever the target lands within a 2%
        // band — gives wheel + pinch a satisfying detent at native
        // scale and keeps text rendering from drifting off pixel grid.
        let new_scale = if (raw_new - 1.0).abs() < 0.02 {
            1.0
        } else {
            raw_new
        };
        if (new_scale - old_scale).abs() < f32::EPSILON {
            return;
        }
        let world_x = (cx - self.viewport.tx) / old_scale;
        let world_y = (cy - self.viewport.ty) / old_scale;
        self.viewport.scale = new_scale;
        self.viewport.tx = world_x.mul_add(-new_scale, cx);
        self.viewport.ty = world_y.mul_add(-new_scale, cy);
        self.notify();
    }

    /// Latch the pan anchor at the current surface cursor. Called on
    /// middle-button press; the matching `pan_move` / `pan_end` pair
    /// unpacks the drag.
    pub const fn pan_begin(&mut self, sx: f32, sy: f32) {
        self.pan_anchor = Some((sx, sy));
    }

    /// Consume one drag sample: pan the viewport by the delta from the
    /// last anchor, then rebase the anchor. No-op when no pan is active.
    pub fn pan_move(&mut self, sx: f32, sy: f32) {
        let Some((ax, ay)) = self.pan_anchor else {
            return;
        };
        self.pan_anchor = Some((sx, sy));
        self.viewport_pan(sx - ax, sy - ay);
    }

    pub const fn pan_end(&mut self) {
        self.pan_anchor = None;
    }

    #[must_use]
    pub const fn is_panning(&self) -> bool {
        self.pan_anchor.is_some()
    }

    #[must_use]
    pub const fn is_gesture_active(&self) -> bool {
        self.gesture_active
    }

    /// Register a new touch point. Entering multi-finger mode (`>= 2`
    /// fingers) rolls back any in-flight stroke so a two-finger gesture
    /// never lands as ink on the doc.
    pub fn touch_down(&mut self, id: u64, sx: f32, sy: f32) {
        self.finger_positions.insert(id, (sx, sy));
        if self.finger_positions.len() >= 2 && !self.gesture_active {
            self.gesture_active = true;
            self.cancel();
        }
        self.gesture_baseline = self.compute_gesture_baseline();
    }

    /// Update a tracked finger. Pans + zooms the viewport by the delta
    /// against the previous baseline whenever a gesture is active.
    pub fn touch_move(&mut self, id: u64, sx: f32, sy: f32) {
        if !self.finger_positions.contains_key(&id) {
            return;
        }
        self.finger_positions.insert(id, (sx, sy));
        if !self.gesture_active {
            return;
        }
        let Some(new) = self.compute_gesture_baseline() else {
            return;
        };
        if let Some(prev) = self.gesture_baseline {
            let dx = new.centroid.0 - prev.centroid.0;
            let dy = new.centroid.1 - prev.centroid.1;
            if dx != 0.0 || dy != 0.0 {
                self.viewport_pan(dx, dy);
            }
            // Distances below one surface pixel are numerically noisy
            // — a pair that near-collides would produce factor blowups.
            if prev.distance > 1.0 && new.distance > 1.0 {
                let factor = new.distance / prev.distance;
                if (factor - 1.0).abs() > 1e-4 {
                    self.viewport_zoom_at(new.centroid.0, new.centroid.1, factor);
                }
            }
        }
        self.gesture_baseline = Some(new);
    }

    /// Drop a finger from the tracker. Leaving multi-finger mode clears
    /// gesture state so the next single-finger contact resumes the
    /// normal pen/input path.
    pub fn touch_up(&mut self, id: u64) {
        self.finger_positions.remove(&id);
        if self.finger_positions.len() < 2 {
            self.gesture_active = false;
            self.gesture_baseline = None;
        } else {
            self.gesture_baseline = self.compute_gesture_baseline();
        }
    }

    fn compute_gesture_baseline(&self) -> Option<GestureBaseline> {
        if self.finger_positions.len() < 2 {
            return None;
        }
        // Deterministic pair choice keeps the baseline stable across
        // frames — freya doesn't order finger ids for us.
        let mut ids: Vec<u64> = self.finger_positions.keys().copied().collect();
        ids.sort_unstable();
        let (ax, ay) = *self.finger_positions.get(&ids[0])?;
        let (bx, by) = *self.finger_positions.get(&ids[1])?;
        let centroid = ((ax + bx) * 0.5, (ay + by) * 0.5);
        let distance = (bx - ax).hypot(by - ay);
        Some(GestureBaseline { centroid, distance })
    }

    /// Project a surface-local point (as delivered by the pen backend
    /// and by freya pointer events) into world coordinates — the space
    /// strokes are stored in and the spatial index is keyed by.
    #[must_use]
    pub fn screen_to_world(&self, x: f32, y: f32) -> (f32, f32) {
        self.viewport.screen_to_world(x, y)
    }

    #[must_use]
    pub fn world_to_screen(&self, x: f32, y: f32) -> (f32, f32) {
        self.viewport.world_to_screen(x, y)
    }

    /// Replace the doc wholesale — used by load-from-disk. Rebuilds the
    /// spatial index, the `stroke_id → index` table and every cached
    /// stroke path, and drops any in-flight session.
    pub fn replace_doc(&mut self, doc: Doc) {
        self.doc = doc;
        self.active = None;
        self.active_builder = None;
        self.active_next_segment = 0;
        self.active_layer_at_begin = None;
        self.erase_session = None;
        self.history.clear();
        self.spatial.clear();
        self.cached_paths.clear();
        self.stroke_index.clear();
        for layer in &self.doc.layers {
            for (idx, stroke) in layer.strokes.iter().enumerate() {
                self.spatial.insert(stroke.id, &stroke.points);
                self.stroke_index.insert(stroke.id, (layer.id, idx));
                if let Some(preset) = self.doc.preset(stroke.brush) {
                    self.cached_paths
                        .insert(stroke.id, build_stroke_path(preset, stroke));
                }
            }
        }
        self.notify();
    }

    pub fn begin(&mut self, point: InkPoint) {
        if let Some(active) = self.active.take() {
            self.commit_stroke(active);
        }
        self.active_builder = None;
        self.active_next_segment = 0;
        self.active_layer_at_begin = None;
        if self.current_kind() == BrushKind::Eraser {
            // Erase gestures don't own a target layer — they always
            // fan out across every visible+unlocked layer via the
            // spatial index.
            self.erase_session = Some(EraseSession::default());
            self.apply_erase(point);
            self.notify();
            return;
        }
        // Refuse to start a paint stroke when the active layer is
        // missing or locked. Silent — the UI already renders the lock
        // affordance, so nothing further to communicate here.
        let target = self.doc.active_layer;
        match self.doc.layer(target) {
            Some(layer) if !layer.locked => {}
            _ => return,
        }
        self.active_layer_at_begin = Some(target);
        let brush = self.doc.register_brush(self.current_preset);
        let id = self.doc.allocate_stroke_id();
        // Geometry for the first sample is drawn by the per-paint tail
        // path (a single circle at n=1). Stable builder starts empty
        // and only grows once `points.len() >= active_next_segment + 4`.
        self.active_builder = Some(PathBuilder::new());
        self.active = Some(Stroke::new(id, brush, self.current_color, point));
        self.notify();
    }

    pub fn extend(&mut self, point: InkPoint) {
        if self.erase_session.is_some() {
            self.apply_erase(point);
            self.notify();
            return;
        }
        let Some(active) = self.active.as_mut() else {
            return;
        };
        active.points.push(point);
        let brush = active.brush;
        let n = active.points.len();
        let Some(preset) = self.doc.preset(brush).copied() else {
            self.notify();
            return;
        };
        let Some(builder) = self.active_builder.as_mut() else {
            self.notify();
            return;
        };
        let points: &[InkPoint] = &self.active.as_ref().expect("just pushed to active").points;
        // Freeze a segment only after we have BOTH its neighbouring
        // control points and one more sample past that — the tail
        // always keeps the last two segments so `p3` never has to
        // transition from mirror to actual once a segment is stable.
        while self.active_next_segment + 4 <= n {
            let k = self.active_next_segment;
            emit_segment(builder, &preset, points, k);
            self.active_next_segment += 1;
        }
        self.notify();
    }

    /// Screen-space entrypoint mirroring [`Board::begin`]. Projects the
    /// incoming surface-pixel sample through the current [`Viewport`]
    /// so pen and pointer inputs land in the same world coordinates the
    /// stroke buffer stores.
    pub fn begin_screen(&mut self, mut point: InkPoint) {
        if self.gesture_active {
            return;
        }
        let (wx, wy) = self.viewport.screen_to_world(point.x, point.y);
        point.x = wx;
        point.y = wy;
        self.begin(point);
    }

    /// Screen-space entrypoint mirroring [`Board::extend`]. See
    /// [`Board::begin_screen`] for the projection rationale.
    pub fn extend_screen(&mut self, mut point: InkPoint) {
        if self.gesture_active {
            return;
        }
        let (wx, wy) = self.viewport.screen_to_world(point.x, point.y);
        point.x = wx;
        point.y = wy;
        self.extend(point);
    }

    pub fn end(&mut self) {
        if let Some(session) = self.erase_session.take() {
            if !session.is_empty() {
                self.history.push(HistoryOp::Erase(session));
            }
            self.notify();
            return;
        }
        if let Some(active) = self.active.take() {
            self.commit_stroke(active);
            self.notify();
        }
    }

    pub fn cancel(&mut self) {
        if let Some(session) = self.erase_session.take() {
            self.rollback_session(&session);
            self.notify();
            return;
        }
        if self.active.take().is_some() {
            self.active_builder = None;
            self.active_next_segment = 0;
            self.active_layer_at_begin = None;
            self.notify();
        }
    }

    pub fn clear(&mut self) {
        self.doc.clear();
        self.active = None;
        self.active_builder = None;
        self.active_next_segment = 0;
        self.active_layer_at_begin = None;
        self.erase_session = None;
        self.history.clear();
        self.spatial.clear();
        self.cached_paths.clear();
        self.stroke_index.clear();
        self.notify();
    }

    /// Undo the most recent recorded op — currently just erase
    /// sessions. Returns `true` if anything was undone.
    pub fn undo(&mut self) -> bool {
        let Some(HistoryOp::Erase(session)) = self.history.pop() else {
            return false;
        };
        self.rollback_session(&session);
        self.notify();
        true
    }

    /// Snapshot every layer's UI-visible state (id, name, visible,
    /// locked, opacity). Returned bottom-to-top; caller reverses for
    /// a top-of-stack-first display.
    #[must_use]
    pub fn layers_snapshot(&self) -> Vec<LayerSnapshot> {
        self.doc
            .layers
            .iter()
            .map(|l| LayerSnapshot {
                id: l.id,
                name: l.name.clone(),
                visible: l.visible,
                locked: l.locked,
                opacity: l.opacity,
                stroke_count: l.strokes.len(),
            })
            .collect()
    }

    #[must_use]
    pub const fn active_layer_id(&self) -> u32 {
        self.doc.active_layer
    }

    /// Add a new empty layer, name it "Layer N" where N is one past
    /// the current count, and adopt it as the active layer. Returns
    /// the new id.
    pub fn add_layer(&mut self) -> u32 {
        let n = self.doc.layers.len() + 1;
        let id = self.doc.add_layer(format!("Layer {n}"));
        self.doc.active_layer = id;
        self.notify();
        id
    }

    /// Remove the currently active layer. No-op if it's the only
    /// layer. Purges every derived index entry for strokes that
    /// belonged to the removed layer, and cancels any in-flight
    /// stroke that was aimed at it.
    pub fn remove_active_layer(&mut self) {
        let id = self.doc.active_layer;
        let Some(removed) = self.doc.remove_layer(id) else {
            return;
        };
        for stroke in &removed.strokes {
            self.stroke_index.remove(&stroke.id);
            self.cached_paths.remove(&stroke.id);
            self.spatial.remove(stroke.id, &stroke.points);
        }
        if self.active_layer_at_begin == Some(id) {
            self.active = None;
            self.active_builder = None;
            self.active_next_segment = 0;
            self.active_layer_at_begin = None;
        }
        self.notify();
    }

    pub fn set_active_layer(&mut self, id: u32) {
        if self.doc.set_active_layer(id) {
            self.notify();
        }
    }

    pub fn set_layer_visible(&mut self, id: u32, visible: bool) {
        if let Some(layer) = self.doc.layer_mut(id) {
            layer.visible = visible;
            self.notify();
        }
    }

    pub fn set_layer_locked(&mut self, id: u32, locked: bool) {
        if let Some(layer) = self.doc.layer_mut(id) {
            layer.locked = locked;
            self.notify();
        }
    }

    pub fn set_layer_opacity(&mut self, id: u32, opacity: f32) {
        if let Some(layer) = self.doc.layer_mut(id) {
            layer.opacity = opacity.clamp(0.0, 1.0);
            self.notify();
        }
    }

    fn commit_stroke(&mut self, stroke: Stroke) {
        self.active_builder = None;
        self.active_next_segment = 0;
        let layer_id = self
            .active_layer_at_begin
            .take()
            .unwrap_or(self.doc.active_layer);
        if stroke.points.is_empty() {
            return;
        }
        let Some(preset) = self.doc.preset(stroke.brush).copied() else {
            return;
        };
        let path = build_stroke_path(&preset, &stroke);
        self.spatial.insert(stroke.id, &stroke.points);
        let id = stroke.id;
        let Some(layer) = self.doc.layer_mut(layer_id) else {
            self.spatial.remove(id, &stroke.points);
            return;
        };
        let within = layer.strokes.len();
        layer.strokes.push(stroke);
        self.stroke_index.insert(id, (layer_id, within));
        self.cached_paths.insert(id, path);
    }

    fn apply_erase(&mut self, sample: InkPoint) {
        let preset = self.current_preset;
        let radius = 0.5 * preset.width(sample.pressure_f32(), sample.tilt_f32());
        if radius <= 0.0 {
            return;
        }
        let candidates = self.spatial.query_circle(sample.x, sample.y, radius);
        for id in candidates {
            // Skip strokes that live in an invisible or locked layer —
            // the user can't see them, and locking is a hard "leave
            // this layer alone" contract.
            let Some(&(layer_id, _)) = self.stroke_index.get(&id) else {
                continue;
            };
            let Some(layer) = self.doc.layer(layer_id) else {
                continue;
            };
            if !layer.visible || layer.locked {
                continue;
            }
            self.erase_stroke(id, layer_id, sample.x, sample.y, radius);
        }
    }

    fn erase_stroke(&mut self, id: u32, layer_id: u32, cx: f32, cy: f32, r: f32) {
        let Some(&(_, idx)) = self.stroke_index.get(&id) else {
            return;
        };
        let Some(layer) = self.doc.layer(layer_id) else {
            return;
        };
        let stroke = &layer.strokes[idx];
        let outcome = split_polyline(&stroke.points, cx, cy, r);
        if !outcome.touched {
            return;
        }

        let Some(original) = self.remove_stroke_indexed(id) else {
            return;
        };
        self.spatial.remove(original.id, &original.points);

        let preset = self.doc.preset(original.brush).copied();
        let mut fragment_ids = Vec::with_capacity(outcome.fragments.len());
        for fragment in outcome.fragments {
            if fragment.len() < 2 {
                // Solitary points are hard to see and easy to
                // accidentally leave behind; drop them so the eraser
                // fully clears where the user gestured.
                continue;
            }
            let frag_id = self.doc.allocate_stroke_id();
            fragment_ids.push(frag_id);
            let new_stroke = Stroke {
                id: frag_id,
                brush: original.brush,
                color: original.color,
                points: fragment,
            };
            if let Some(preset) = preset.as_ref() {
                self.cached_paths
                    .insert(frag_id, build_stroke_path(preset, &new_stroke));
            }
            self.spatial.insert(new_stroke.id, &new_stroke.points);
            let Some(layer) = self.doc.layer_mut(layer_id) else {
                continue;
            };
            let new_idx = layer.strokes.len();
            layer.strokes.push(new_stroke);
            self.stroke_index.insert(frag_id, (layer_id, new_idx));
        }

        if let Some(session) = self.erase_session.as_mut() {
            session.record(layer_id, original, &fragment_ids);
        }
    }

    /// Remove a stroke by id, keeping `stroke_index` and `cached_paths`
    /// in sync. Preserves within-layer z-order and only shifts
    /// indices for strokes that share the affected layer.
    fn remove_stroke_indexed(&mut self, id: u32) -> Option<Stroke> {
        let (layer_id, idx) = self.stroke_index.remove(&id)?;
        let removed = {
            let layer = self
                .doc
                .layer_mut(layer_id)
                .expect("stroke_index points at a layer that no longer exists");
            layer.strokes.remove(idx)
        };
        for entry in self.stroke_index.values_mut() {
            if entry.0 == layer_id && entry.1 > idx {
                entry.1 -= 1;
            }
        }
        self.cached_paths.remove(&id);
        Some(removed)
    }

    fn rollback_session(&mut self, session: &EraseSession) {
        for frag_id in &session.added_fragments {
            if let Some(removed) = self.remove_stroke_indexed(*frag_id) {
                self.spatial.remove(removed.id, &removed.points);
            }
        }
        for entry in &session.originals {
            let layer_id = entry.layer_id;
            let original = &entry.stroke;
            let id = original.id;
            let preset = self.doc.preset(original.brush).copied();
            if let Some(preset) = preset.as_ref() {
                self.cached_paths
                    .insert(id, build_stroke_path(preset, original));
            }
            self.spatial.insert(id, &original.points);
            if let Some(layer) = self.doc.layer_mut(layer_id) {
                let idx = layer.strokes.len();
                layer.strokes.push(original.clone());
                self.stroke_index.insert(id, (layer_id, idx));
            }
        }
    }

    /// Paint every committed stroke plus the active one onto `canvas`.
    ///
    /// Committed strokes: cached Skia path per stroke, one `draw_path`
    /// each. Active stroke: the frozen prefix is snapshotted from
    /// `active_builder` (non-consuming, cheap) and the trailing two
    /// segments are rebuilt every frame into a fresh tail path so
    /// their Catmull-Rom `p3` neighbour stays live (mirror while the
    /// user is still drawing, actual once the next sample arrives).
    pub fn paint(&self, canvas: &freya_engine::prelude::Canvas) {
        // Everything below draws in world coordinates. Wrapping in a
        // save/restore lets viewport pan+zoom compose freely with any
        // future overlay pass that wants to draw in screen space.
        canvas.save();
        canvas.translate((self.viewport.tx, self.viewport.ty));
        canvas.scale((self.viewport.scale, self.viewport.scale));
        for layer in &self.doc.layers {
            if !layer.visible {
                continue;
            }
            let opacity = layer.opacity.clamp(0.0, 1.0);
            for stroke in &layer.strokes {
                let Some(cached) = self.cached_paths.get(&stroke.id) else {
                    continue;
                };
                let Some(preset) = self.doc.preset(stroke.brush) else {
                    continue;
                };
                let mut paint = stroke_paint(preset, stroke.color);
                if opacity < 0.999 {
                    let base = paint.color();
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let scaled = (f32::from(base.a()) * opacity).clamp(0.0, 255.0) as u8;
                    paint.set_color(base.with_a(scaled));
                }
                canvas.draw_path(cached, &paint);
            }
        }
        if let (Some(active), Some(builder)) = (&self.active, &self.active_builder) {
            let Some(preset) = self.doc.preset(active.brush) else {
                return;
            };
            let layer_opacity = self
                .active_layer_at_begin
                .and_then(|id| self.doc.layer(id))
                .map_or(1.0, |l| l.opacity.clamp(0.0, 1.0));
            let mut paint = stroke_paint(preset, active.color);
            if layer_opacity < 0.999 {
                let base = paint.color();
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let scaled = (f32::from(base.a()) * layer_opacity).clamp(0.0, 255.0) as u8;
                paint.set_color(base.with_a(scaled));
            }
            canvas.draw_path(&builder.snapshot(), &paint);
            let tail = build_active_tail(preset, &active.points, self.active_next_segment);
            canvas.draw_path(&tail, &paint);
        }
        canvas.restore();
    }
}

/// Build the full ribbon path for a stroke in one pass.
///
/// Each pair of adjacent samples `points[k], points[k+1]` becomes one
/// Catmull-Rom segment: neighbours `points[k-1]` and `points[k+2]`
/// (mirror-extrapolated at the ends) drive the tangents, and the
/// segment's ribbon is approximated by a chain of straight-line
/// trapezoids over `subdivision_count` interpolated points. A circle
/// caps every sample vertex and the last sample; every contour is CCW
/// so Skia's non-zero fill accumulates overlaps instead of cancelling
/// them (fast strokes produced dotted seams under mixed windings).
fn build_stroke_path(preset: &BrushPreset, stroke: &Stroke) -> Path {
    let mut builder = PathBuilder::new();
    let n = stroke.points.len();
    if n == 0 {
        return builder.detach();
    }
    if n == 1 {
        emit_end_cap(&mut builder, preset, stroke.points[0]);
        return builder.detach();
    }
    for k in 0..(n - 1) {
        emit_segment(&mut builder, preset, &stroke.points, k);
    }
    emit_end_cap(&mut builder, preset, stroke.points[n - 1]);
    builder.detach()
}

/// Rebuild the trailing tail of the active stroke each frame: every
/// segment from `next_segment` to the last sample, plus the end cap.
/// Segment shape is a live function of the current control points, so
/// the tail can freely use `p3 = mirror(p1, p2)` while the user is
/// still drawing and swap to the actual sample as soon as one more
/// point arrives — the pop that would otherwise appear at freeze time
/// is avoided by keeping the last two segments here rather than in
/// `active_builder`.
fn build_active_tail(preset: &BrushPreset, points: &[InkPoint], next_segment: usize) -> Path {
    let mut builder = PathBuilder::new();
    let n = points.len();
    if n == 0 {
        return builder.detach();
    }
    if n == 1 {
        emit_end_cap(&mut builder, preset, points[0]);
        return builder.detach();
    }
    for k in next_segment..(n - 1) {
        emit_segment(&mut builder, preset, points, k);
    }
    emit_end_cap(&mut builder, preset, points[n - 1]);
    builder.detach()
}

/// Emit one Catmull-Rom segment (`points[k] → points[k+1]`) into
/// `builder`: a CCW circle at the segment's start sample and a chain
/// of trapezoids along the curve. The final sample of the stroke is
/// capped separately by [`emit_end_cap`] — every internal sample gets
/// its cap here as the `p1` circle of the segment it starts.
fn emit_segment(builder: &mut PathBuilder, preset: &BrushPreset, points: &[InkPoint], k: usize) {
    let n = points.len();
    let p1 = points[k];
    let p2 = points[k + 1];
    // Mirror the missing neighbour at the endpoints (`p0 = 2·p1 - p2`
    // at the start, `p3 = 2·p2 - p1` at the end). This is the standard
    // linear extrapolation used to give the boundary segments a natural
    // tangent without asking the user for phantom control points.
    let p0 = if k > 0 {
        (points[k - 1].x, points[k - 1].y)
    } else {
        (2.0f32.mul_add(p1.x, -p2.x), 2.0f32.mul_add(p1.y, -p2.y))
    };
    let p3 = if k + 2 < n {
        (points[k + 2].x, points[k + 2].y)
    } else {
        (2.0f32.mul_add(p2.x, -p1.x), 2.0f32.mul_add(p2.y, -p1.y))
    };
    let p1_xy = (p1.x, p1.y);
    let p2_xy = (p2.x, p2.y);
    let half_1 = 0.5 * preset.width(p1.pressure_f32(), p1.tilt_f32());
    let half_2 = 0.5 * preset.width(p2.pressure_f32(), p2.tilt_f32());

    if half_1 > 0.0 {
        builder.add_circle(p1_xy, half_1, PathDirection::CCW);
    }

    let steps = subdivision_count(p1_xy, p2_xy);
    let mut prev: Option<((f32, f32), f32)> = None;
    let steps_f = f32_from_usize(steps);
    for j in 0..=steps {
        let t = f32_from_usize(j) / steps_f;
        let q = catmull_rom_centripetal(p0, p1_xy, p2_xy, p3, t);
        let half = lerp(half_1, half_2, t);
        if let Some((prev_q, prev_half)) = prev {
            append_trapezoid(builder, prev_q, q, prev_half, half);
        }
        prev = Some((q, half));
    }
}

fn emit_end_cap(builder: &mut PathBuilder, preset: &BrushPreset, p: InkPoint) {
    let half = 0.5 * preset.width(p.pressure_f32(), p.tilt_f32());
    if half > 0.0 {
        builder.add_circle((p.x, p.y), half, PathDirection::CCW);
    }
}

/// Centripetal Catmull-Rom: parameterised so knot spacing goes as the
/// square root of chord length. Suppresses the self-intersections and
/// cusps the uniform (`α = 0`) variant produces on tight loops or
/// non-uniform sample spacing — exactly the failure modes a
/// pen-capture stream feeds into an interpolator.
fn catmull_rom_centripetal(
    p0: (f32, f32),
    p1: (f32, f32),
    p2: (f32, f32),
    p3: (f32, f32),
    t: f32,
) -> (f32, f32) {
    let t0 = 0.0f32;
    let t1 = t0 + dist_sq(p0, p1).sqrt().sqrt().max(1e-4);
    let t2 = t1 + dist_sq(p1, p2).sqrt().sqrt().max(1e-4);
    let t3 = t2 + dist_sq(p2, p3).sqrt().sqrt().max(1e-4);
    let tt = t.mul_add(t2 - t1, t1);
    // Barry-Goldman recursion (three levels of linear interp — the
    // canonical evaluation form that stays numerically stable when
    // consecutive knots are near-equal).
    let a1 = lerp2d(p0, p1, (tt - t0) / (t1 - t0));
    let a2 = lerp2d(p1, p2, (tt - t1) / (t2 - t1));
    let a3 = lerp2d(p2, p3, (tt - t2) / (t3 - t2));
    let b1 = lerp2d(a1, a2, (tt - t0) / (t2 - t0));
    let b2 = lerp2d(a2, a3, (tt - t1) / (t3 - t1));
    lerp2d(b1, b2, (tt - t1) / (t2 - t1))
}

fn dist_sq(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    dx.mul_add(dx, dy * dy)
}

fn lerp2d(a: (f32, f32), b: (f32, f32), t: f32) -> (f32, f32) {
    (t.mul_add(b.0 - a.0, a.0), t.mul_add(b.1 - a.1, a.1))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    t.mul_add(b - a, a)
}

// `subdivision_count` returns tiny usizes (< 128 in practice — see the
// clamp inside it), so an f32 cast can't actually lose precision.
// Explicit helper keeps the cast localised and lets clippy see the
// suppression at one site instead of scattering `allow` on every use.
#[allow(clippy::cast_precision_loss)]
const fn f32_from_usize(n: usize) -> f32 {
    n as f32
}

/// Subdivisions per Catmull-Rom segment. Roughly one step per 6 dp of
/// chord length, clamped so tight-sample slow strokes still stay
/// visibly smooth and fast wide strokes don't blow up the vertex
/// count — the trapezoid + circle count per stroke stays bounded by
/// `samples × 12` in the worst case.
fn subdivision_count(a: (f32, f32), b: (f32, f32)) -> usize {
    let d = dist_sq(a, b).sqrt();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let raw = (d / 6.0).ceil() as usize;
    raw.clamp(3, 12)
}

fn append_trapezoid(
    builder: &mut PathBuilder,
    from: (f32, f32),
    to: (f32, f32),
    half_from: f32,
    half_to: f32,
) {
    let dx = to.0 - from.0;
    let dy = to.1 - from.1;
    let len = dx.hypot(dy);
    if len < 1e-3 {
        return;
    }
    // Left-hand perpendicular unit vector.
    let nx = -dy / len;
    let ny = dx / len;
    builder
        .move_to((nx.mul_add(half_from, from.0), ny.mul_add(half_from, from.1)))
        .line_to((nx.mul_add(half_to, to.0), ny.mul_add(half_to, to.1)))
        .line_to((nx.mul_add(-half_to, to.0), ny.mul_add(-half_to, to.1)))
        .line_to((
            nx.mul_add(-half_from, from.0),
            ny.mul_add(-half_from, from.1),
        ))
        .close();
}

fn stroke_paint(preset: &BrushPreset, color: [u8; 4]) -> Paint {
    let style = preset.stroke_style(color);
    let mut paint = Paint::default();
    configure_fill_paint(&mut paint, &style);
    paint
}

fn configure_fill_paint(paint: &mut Paint, style: &StrokeStyle) {
    paint.set_anti_alias(true);
    paint.set_style(PaintStyle::Fill);

    let sk_color: SkColor = style.color.into();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let alpha = (style.opacity.clamp(0.0, 1.0) * 255.0) as u8;
    paint.set_color(sk_color.with_a(alpha));

    paint.set_blend_mode(match style.mode {
        SegmentMode::Draw => BlendMode::SrcOver,
        SegmentMode::Multiply => BlendMode::Multiply,
    });
}

/// Freya canvas element wired to a shared [`Board`].
///
/// The outer rect owns viewport gestures (wheel zoom + middle-button
/// drag pan). Pen and stylus input still land on the [`Board`] through
/// [`crate::pen_pump`] — the freya handlers here operate on the
/// [`Viewport`] only, so simultaneous pen strokes are untouched.
pub fn drawing_surface(board: &Arc<Mutex<Board>>) -> impl IntoElement {
    let render_board = Arc::clone(board);

    let inner = canvas(RenderCallback::new(move |ctx| {
        let guard = match render_board.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.paint(ctx.canvas);
    }))
    .width(Size::fill())
    .height(Size::fill());

    let wheel_board = Arc::clone(board);
    let down_board = Arc::clone(board);
    let move_board = Arc::clone(board);
    let up_board = Arc::clone(board);
    let touch_start_board = Arc::clone(board);
    let touch_move_board = Arc::clone(board);
    let touch_end_board = Arc::clone(board);
    let touch_cancel_board = Arc::clone(board);

    rect()
        .width(Size::fill())
        .height(Size::fill())
        .background(Color::from_rgb(250, 250, 248))
        .on_wheel(move |e: Event<WheelEventData>| {
            #[allow(clippy::cast_possible_truncation)]
            let x = e.element_location.x as f32;
            #[allow(clippy::cast_possible_truncation)]
            let y = e.element_location.y as f32;
            // Scroll up (dy < 0) zooms in. Empirical exponent — good
            // feel on a standard mouse wheel and on a laptop trackpad;
            // revisit if pinch-zoom-emulation trackpads land here too.
            #[allow(clippy::cast_possible_truncation)]
            let dy = e.delta_y as f32;
            let factor = (-dy * 0.0015).exp();
            lock(&wheel_board).viewport_zoom_at(x, y, factor);
        })
        .on_mouse_down(move |e: Event<MouseEventData>| {
            if e.button != Some(MouseButton::Middle) {
                return;
            }
            #[allow(clippy::cast_possible_truncation)]
            let x = e.global_location.x as f32;
            #[allow(clippy::cast_possible_truncation)]
            let y = e.global_location.y as f32;
            lock(&down_board).pan_begin(x, y);
        })
        .on_global_pointer_move(move |e: Event<PointerEventData>| {
            let mut guard = lock(&move_board);
            if !guard.is_panning() {
                return;
            }
            let loc = e.global_location();
            #[allow(clippy::cast_possible_truncation)]
            let x = loc.x as f32;
            #[allow(clippy::cast_possible_truncation)]
            let y = loc.y as f32;
            guard.pan_move(x, y);
        })
        .on_global_pointer_press(move |_: Event<PointerEventData>| {
            lock(&up_board).pan_end();
        })
        .on_touch_start(move |e: Event<TouchEventData>| {
            #[allow(clippy::cast_possible_truncation)]
            let x = e.element_location.x as f32;
            #[allow(clippy::cast_possible_truncation)]
            let y = e.element_location.y as f32;
            lock(&touch_start_board).touch_down(e.finger_id, x, y);
        })
        .on_touch_move(move |e: Event<TouchEventData>| {
            #[allow(clippy::cast_possible_truncation)]
            let x = e.element_location.x as f32;
            #[allow(clippy::cast_possible_truncation)]
            let y = e.element_location.y as f32;
            lock(&touch_move_board).touch_move(e.finger_id, x, y);
        })
        .on_touch_end(move |e: Event<TouchEventData>| {
            lock(&touch_end_board).touch_up(e.finger_id);
        })
        .on_touch_cancel(move |e: Event<TouchEventData>| {
            lock(&touch_cancel_board).touch_up(e.finger_id);
        })
        .child(inner)
}

/// Lock the shared board, recovering transparently from mutex
/// poisoning. Every mutation site is a short vec push with no
/// panic-prone code — forward progress beats an unactionable error.
pub fn lock(board: &Arc<Mutex<Board>>) -> std::sync::MutexGuard<'_, Board> {
    match board.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[derive(Debug)]
struct SplitOutcome {
    touched: bool,
    fragments: Vec<Vec<InkPoint>>,
}

/// Clip `points` against a circle. Fragments outside the circle are
/// preserved; parts inside are dropped, with fresh interpolated
/// vertices inserted at every circle-boundary crossing.
fn split_polyline(points: &[InkPoint], cx: f32, cy: f32, r: f32) -> SplitOutcome {
    let mut out = SplitOutcome {
        touched: false,
        fragments: Vec::new(),
    };
    if points.is_empty() {
        return out;
    }
    let r2 = r * r;
    let dist2 = |p: &InkPoint| {
        let dx = p.x - cx;
        let dy = p.y - cy;
        dx.mul_add(dx, dy * dy)
    };
    let inside = |p: &InkPoint| dist2(p) <= r2;

    let mut cur: Vec<InkPoint> = Vec::new();
    if inside(&points[0]) {
        out.touched = true;
    } else {
        cur.push(points[0]);
    }

    for i in 1..points.len() {
        let a = points[i - 1];
        let b = points[i];
        let a_in = inside(&a);
        let b_in = inside(&b);

        match (a_in, b_in) {
            (false, false) => {
                if let Some((t0, t1)) = seg_circle_intersections(&a, &b, cx, cy, r) {
                    out.touched = true;
                    let clip0 = t0.clamp(0.0, 1.0);
                    let clip1 = t1.clamp(0.0, 1.0);
                    let p0 = interp(&a, &b, clip0);
                    let p1 = interp(&a, &b, clip1);
                    cur.push(p0);
                    if !cur.is_empty() {
                        out.fragments.push(std::mem::take(&mut cur));
                    }
                    cur.push(p1);
                }
                cur.push(b);
            }
            (false, true) => {
                out.touched = true;
                if let Some(t) = enter_t(&a, &b, cx, cy, r) {
                    cur.push(interp(&a, &b, t));
                }
                if !cur.is_empty() {
                    out.fragments.push(std::mem::take(&mut cur));
                }
            }
            (true, false) => {
                out.touched = true;
                if let Some(t) = exit_t(&a, &b, cx, cy, r) {
                    cur.push(interp(&a, &b, t));
                }
                cur.push(b);
            }
            (true, true) => {
                out.touched = true;
            }
        }
    }

    if !cur.is_empty() {
        out.fragments.push(cur);
    }
    out
}

/// Ordered `(t0, t1)` roots where the parameterised segment
/// `a + t(b - a)` crosses the circle. Both must lie in `0.0..=1.0` for
/// a chord-style clip to apply.
fn seg_circle_intersections(
    a: &InkPoint,
    b: &InkPoint,
    cx: f32,
    cy: f32,
    r: f32,
) -> Option<(f32, f32)> {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let fx = a.x - cx;
    let fy = a.y - cy;
    let aa = dx.mul_add(dx, dy * dy);
    if aa == 0.0 {
        return None;
    }
    let bb = 2.0 * dx.mul_add(fx, dy * fy);
    let cc = r.mul_add(-r, fx.mul_add(fx, fy * fy));
    let disc = bb.mul_add(bb, -4.0 * aa * cc);
    if disc < 0.0 {
        return None;
    }
    let sqrt_d = disc.sqrt();
    let inv = 0.5 / aa;
    let t0 = (-bb - sqrt_d) * inv;
    let t1 = (-bb + sqrt_d) * inv;
    if t1 < 0.0 || t0 > 1.0 {
        return None;
    }
    Some((t0, t1))
}

fn enter_t(a: &InkPoint, b: &InkPoint, cx: f32, cy: f32, r: f32) -> Option<f32> {
    seg_circle_intersections(a, b, cx, cy, r).and_then(|(t0, t1)| {
        if (0.0..=1.0).contains(&t0) {
            Some(t0)
        } else if (0.0..=1.0).contains(&t1) {
            Some(t1)
        } else {
            None
        }
    })
}

fn exit_t(a: &InkPoint, b: &InkPoint, cx: f32, cy: f32, r: f32) -> Option<f32> {
    seg_circle_intersections(a, b, cx, cy, r).and_then(|(t0, t1)| {
        if (0.0..=1.0).contains(&t1) {
            Some(t1)
        } else if (0.0..=1.0).contains(&t0) {
            Some(t0)
        } else {
            None
        }
    })
}

fn interp(from: &InkPoint, to: &InkPoint, t: f32) -> InkPoint {
    let x = (to.x - from.x).mul_add(t, from.x);
    let y = (to.y - from.y).mul_add(t, from.y);
    let pressure = lerp_u8(from.pressure, to.pressure, t);
    let tilt = lerp_u8(from.tilt, to.tilt, t);
    // Time deltas are per-segment budgets; the clipped-in point takes
    // the destination sample's share since it visually replaces `to`.
    let dt_us = to.dt_us;
    InkPoint::new(x, y, pressure, tilt, dt_us)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn lerp_u8(from: u8, to: u8, t: f32) -> u8 {
    let av = f32::from(from);
    let bv = f32::from(to);
    (bv - av).mul_add(t, av).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: f32, y: f32) -> InkPoint {
        InkPoint::new(x, y, 128, 0, 0)
    }

    #[test]
    fn split_hits_middle_of_line() {
        let pts = vec![pt(0.0, 0.0), pt(100.0, 0.0)];
        let out = split_polyline(&pts, 50.0, 0.0, 10.0);
        assert!(out.touched);
        assert_eq!(out.fragments.len(), 2);
        assert!(out.fragments[0].last().unwrap().x <= 40.0 + 1e-3);
        assert!(out.fragments[1].first().unwrap().x >= 60.0 - 1e-3);
    }

    #[test]
    fn split_leaves_untouched_alone() {
        let pts = vec![pt(0.0, 0.0), pt(100.0, 0.0)];
        let out = split_polyline(&pts, 50.0, 500.0, 10.0);
        assert!(!out.touched);
    }

    #[test]
    fn erase_and_undo_roundtrip() {
        let mut b = Board::default();
        // Register a paint preset + stroke.
        b.set_current_preset(BrushPreset::pen());
        b.begin(pt(0.0, 0.0));
        b.extend(pt(100.0, 0.0));
        b.end();
        let doc_before = b.doc().clone();

        b.set_current_preset(BrushPreset::eraser());
        b.begin(pt(50.0, 0.0));
        b.end();
        let stroke_count = |d: &Doc| d.layers.iter().map(|l| l.strokes.len()).sum::<usize>();
        assert_ne!(stroke_count(b.doc()), 1, "erase should have split");

        assert!(b.undo());
        assert_eq!(stroke_count(b.doc()), 1);
        let restored = b.doc().layers[0].strokes[0].points.clone();
        assert_eq!(restored, doc_before.layers[0].strokes[0].points);
    }

    #[test]
    fn stroke_commits_into_active_layer() {
        let mut b = Board::default();
        let l2 = b.add_layer();
        b.set_active_layer(l2);
        b.set_current_preset(BrushPreset::pen());
        b.begin(pt(0.0, 0.0));
        b.extend(pt(10.0, 0.0));
        b.end();
        assert_eq!(b.doc().layer(l2).unwrap().strokes.len(), 1);
        assert_eq!(b.doc().layer(0).unwrap().strokes.len(), 0);
    }

    #[test]
    fn locked_layer_refuses_new_stroke() {
        let mut b = Board::default();
        b.set_layer_locked(0, true);
        b.set_current_preset(BrushPreset::pen());
        b.begin(pt(0.0, 0.0));
        b.extend(pt(10.0, 0.0));
        b.end();
        assert!(b.doc().layer(0).unwrap().strokes.is_empty());
    }

    #[test]
    fn erase_skips_invisible_layer() {
        let mut b = Board::default();
        b.set_current_preset(BrushPreset::pen());
        b.begin(pt(0.0, 0.0));
        b.extend(pt(100.0, 0.0));
        b.end();
        b.set_layer_visible(0, false);
        b.set_current_preset(BrushPreset::eraser());
        b.begin(pt(50.0, 0.0));
        b.end();
        // Layer hidden → eraser is a no-op there; original survives.
        assert_eq!(b.doc().layer(0).unwrap().strokes.len(), 1);
    }
}
