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

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use flume::{Receiver, Sender};
use freya::prelude::*;
use freya_canvas_bg::{
    BgPaintCtx, CanvasBackground, Rect as BgRect, RedrawHandle, SolidColorBackground,
    clamp_translation,
};
use freya_engine::prelude::{BlendMode, Color as SkColor, Paint, Path, SaveLayerRec};

use crate::brush::{
    BrushConfig, BrushKind, BrushPreset, CapStyle, EraserMode, InkPoint, ShapeMode, Stroke,
};
use crate::doc::Doc;
use crate::history::{EraseOriginal, EraseSession, HistoryOp};
use crate::render::{BrushRegistry, HighlighterBrush, HighlighterState};
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
        (
            x.mul_add(self.scale, self.tx),
            y.mul_add(self.scale, self.ty),
        )
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
    /// In-flight stroke. Its Skia path is rebuilt from scratch every
    /// paint tick via [`build_stroke_path`] — the ribbon renderer emits
    /// a single closed polygon per stroke, which cannot be composed
    /// incrementally without reintroducing the between-slab seams that
    /// used to show at high zoom.
    active: Option<Stroke>,
    /// Id of the layer the in-flight stroke will commit into. Captured
    /// at [`Board::begin`] so mid-stroke `set_active_layer` calls don't
    /// redirect the commit to an unexpected layer.
    active_layer_at_begin: Option<u32>,
    current_preset: BrushPreset,
    current_color: [u8; 4],
    size_scales: HashMap<BrushKind, f32>,
    /// Per-kind popup-configurable settings. Lookups fall back to
    /// [`BrushConfig::default_for`] so callers never juggle an "unset"
    /// state. Not persisted with the doc — see [`BrushConfig`].
    brush_configs: HashMap<BrushKind, BrushConfig>,
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
    /// Active [`EraserMode::SelectionRect`] drag, expressed as
    /// `(anchor_x, anchor_y, cursor_x, cursor_y)` in world coords.
    /// `Some` while the pen is down; `None` between drags. The paint
    /// pass reads this to overlay the marquee preview.
    selection_rect: Option<(f32, f32, f32, f32)>,
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
    /// Brush + cap renderer lookup. Built-in kinds resolve to
    /// [`crate::render::RibbonBrush`]; external crates install custom
    /// renderers here (see [`Board::brush_registry_mut`]) so
    /// `BrushKind::Custom(id)` / `CapStyle::Custom(id)` strokes route
    /// through user code without patching the canvas.
    brush_registry: Arc<BrushRegistry>,
    /// Highlighter tip state shared with the
    /// [`crate::render::HighlighterBrush`] renderer registered against
    /// [`BrushKind::Highlighter`]. Mutating the config via
    /// [`Board::set_brush_config`] mirrors the tip into this handle so
    /// subsequent repaints pick up the change without touching the
    /// stroke buffer.
    highlighter_state: Arc<HighlighterState>,
    /// `true` between [`Board::begin`] and [`Board::end`] when the
    /// active brush uses a two-anchor rubber-band gesture (highlighter
    /// `straight = true` or any [`BrushKind::Shape`] variant). Flips
    /// [`Board::extend`] into "replace last point" mode so the live
    /// preview follows the cursor.
    two_point_active: bool,
    /// Background layer painted underneath every stroke. Default is a
    /// [`SolidColorBackground`] matching the historical off-white fill;
    /// swap via [`Board::set_background`] to plug in PDF pages, image
    /// stacks, blank paper, etc.
    background: Arc<dyn CanvasBackground>,
    /// Redraw wakeup handed to async background pipelines so a
    /// freshly-rendered page bitmap can request a repaint from any
    /// thread. Rebuilt whenever [`Board::set_notifier`] runs; noop
    /// until the root component installs the notifier.
    redraw_handle: RedrawHandle,
    /// Last `(width, height)` of the paint surface. Updated once per
    /// frame at the top of [`Board::paint`] so viewport-clamping code
    /// (which runs off the paint path, e.g. from `pan_move`) has a
    /// stable reference — a zero pair means "no paint has landed yet"
    /// and disables clamping entirely.
    last_surface_size: (f32, f32),
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
        let highlighter_state = Arc::new(HighlighterState::default());
        let mut brush_registry = BrushRegistry::new();
        brush_registry.set_brush(
            BrushKind::Highlighter,
            Arc::new(HighlighterBrush::new(Arc::clone(&highlighter_state))),
        );
        Self {
            doc: Doc::default(),
            active: None,
            active_layer_at_begin: None,
            current_preset: default_preset,
            current_color: default_preset.color,
            size_scales: HashMap::new(),
            brush_configs: HashMap::new(),
            spatial: SpatialIndex::new(),
            cached_paths: HashMap::new(),
            stroke_index: HashMap::new(),
            history: Vec::new(),
            erase_session: None,
            selection_rect: None,
            notifier: None,
            viewport: Viewport::default(),
            pan_anchor: None,
            finger_positions: HashMap::new(),
            gesture_baseline: None,
            gesture_active: false,
            brush_registry: Arc::new(brush_registry),
            highlighter_state,
            two_point_active: false,
            background: Arc::new(SolidColorBackground::new(SkColor::from_rgb(250, 250, 248))),
            redraw_handle: RedrawHandle::noop(),
            last_surface_size: (0.0, 0.0),
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
    /// re-install (replaces the previous notifier). Also rebuilds
    /// [`Self::redraw_handle`] so async background pipelines route
    /// their wakeups through the same channel.
    pub fn set_notifier(&mut self, notifier: RedrawNotifier) {
        let mirror = notifier.clone();
        self.redraw_handle = RedrawHandle::new(move || mirror.ping());
        self.notifier = Some(notifier);
    }

    /// Swap the background layer painted underneath every stroke.
    /// Triggers a redraw so the new backdrop / pages appear on the
    /// next frame.
    pub fn set_background(&mut self, background: Arc<dyn CanvasBackground>) {
        self.background = background;
        self.notify();
    }

    /// Currently installed background. Callers (e.g. the freya
    /// [`drawing_surface`]) read this to source the viewport backdrop
    /// color and page metadata.
    #[must_use]
    pub fn background(&self) -> &Arc<dyn CanvasBackground> {
        &self.background
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

    /// Overwrite the per-kind popup config. Silently drops a config
    /// whose variant does not match `kind` — the type-level guarantee
    /// makes this a bug, not a runtime error worth surfacing.
    pub fn set_brush_config(&mut self, kind: BrushKind, config: BrushConfig) {
        if let Some(cfg_kind) = config.kind()
            && cfg_kind != kind
        {
            return;
        }
        self.brush_configs.insert(kind, config);
        if let BrushConfig::Highlighter { tip, .. } = config {
            // Mirror into the shared handle the renderer reads at
            // every `build_path`. Committed strokes carry the tip
            // they were built with (cached path); toggling `tip`
            // affects the in-flight active stroke plus every
            // subsequently committed highlighter.
            self.highlighter_state.set(tip);
        }
        self.notify();
    }

    /// Read the popup config for `kind`, falling back to
    /// [`BrushConfig::default_for`] when unset.
    #[must_use]
    pub fn brush_config(&self, kind: BrushKind) -> BrushConfig {
        self.brush_configs
            .get(&kind)
            .copied()
            .unwrap_or_else(|| BrushConfig::default_for(kind))
    }

    /// Shorthand for `brush_config(current_kind())`. Useful in the
    /// paint / erase entry points so a call site does not have to
    /// re-thread the kind.
    #[must_use]
    pub fn current_config(&self) -> BrushConfig {
        self.brush_config(self.current_preset.kind)
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

    /// Shared registry driving [`crate::render::BrushRenderer`] +
    /// [`crate::render::CapRenderer`] dispatch. External code that
    /// wants to plug in a custom brush or cap grabs this before
    /// touching the canvas.
    #[must_use]
    pub fn brush_registry(&self) -> &BrushRegistry {
        &self.brush_registry
    }

    /// Mutable handle to the brush registry. Rewires every cached
    /// path? No — cached paths stay as-is until a stroke is committed
    /// again or the doc is reloaded. Register renderers before
    /// painting starts to keep the cache consistent.
    pub fn brush_registry_mut(&mut self) -> &mut BrushRegistry {
        Arc::make_mut(&mut self.brush_registry)
    }

    #[must_use]
    pub const fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Adopt `viewport` wholesale. Scale is clamped to
    /// `[VIEWPORT_SCALE_MIN, VIEWPORT_SCALE_MAX]`; translation is
    /// additionally clamped against the current background's content
    /// bounds when one is exposed (see [`Self::clamp_viewport_to_background`]).
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
        self.clamp_viewport_to_background();
        self.notify();
    }

    /// Pin viewport translation to the current background's content
    /// bounds. No-op when the background is unbounded
    /// ([`SolidColorBackground`] and any custom `content_bounds()`-of-
    /// `None` impl) or when no paint has landed yet — the surface
    /// dimensions are only known post-first-frame.
    ///
    /// Called at the top of [`Self::paint`] and at the end of every
    /// viewport-mutating method so drift correction happens both under
    /// user gestures and under background swaps.
    pub fn clamp_viewport_to_background(&mut self) {
        let Some(bounds) = self.background.content_bounds() else {
            return;
        };
        let (sw, sh) = self.last_surface_size;
        if sw <= 0.0 || sh <= 0.0 {
            return;
        }
        let bg_bounds = bounds;
        let (tx, ty) = clamp_translation(
            self.viewport.tx,
            self.viewport.ty,
            self.viewport.scale,
            bg_bounds,
            sw,
            sh,
        );
        self.viewport.tx = tx;
        self.viewport.ty = ty;
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
        self.clamp_viewport_to_background();
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
        self.clamp_viewport_to_background();
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
        self.active_layer_at_begin = None;
        self.erase_session = None;
        self.history.clear();
        self.spatial.clear();
        self.cached_paths.clear();
        self.stroke_index.clear();
        // Clone the Arc so the loop body can call renderer methods
        // without holding a borrow to `self.brush_registry` — the mut
        // borrows on `self.cached_paths` below would otherwise clash.
        let registry = Arc::clone(&self.brush_registry);
        for layer in &self.doc.layers {
            for (idx, stroke) in layer.strokes.iter().enumerate() {
                self.spatial.insert(stroke.id, &stroke.points);
                self.stroke_index.insert(stroke.id, (layer.id, idx));
                if let Some(preset) = self.doc.preset(stroke.brush) {
                    let path =
                        registry
                            .brush(preset.kind)
                            .build_path(preset, stroke, registry.caps());
                    self.cached_paths.insert(stroke.id, path);
                }
            }
        }
        self.notify();
    }

    pub fn begin(&mut self, point: InkPoint) {
        let point = self.shape_sample(point);
        if let Some(active) = self.active.take() {
            self.commit_stroke(active);
        }
        self.active_layer_at_begin = None;
        if self.current_kind() == BrushKind::Eraser {
            // Erase gestures target the active layer only. Strokes on
            // other layers stay untouched even if the eraser sweeps
            // over them — see `apply_erase` for the active-layer
            // filter.
            match self.eraser_mode() {
                EraserMode::Point => {
                    self.erase_session = Some(EraseSession::default());
                    self.apply_erase(point);
                }
                EraserMode::Stroke => {
                    self.erase_stroke_at(point.x, point.y);
                }
                EraserMode::SelectionRect => {
                    self.selection_rect = Some((point.x, point.y, point.x, point.y));
                }
            }
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
        self.active = Some(Stroke::new(id, brush, self.current_color, point));
        // Latch straight-mode at gesture start so an in-flight tip
        // toggle does not switch a live stroke between rubber-band and
        // freehand halfway through.
        self.two_point_active = self.is_two_point_stroke_mode();
        self.notify();
    }

    pub fn extend(&mut self, point: InkPoint) {
        let point = self.shape_sample(point);
        if self.erase_session.is_some() {
            self.apply_erase(point);
            self.notify();
            return;
        }
        if let Some((ax, ay, _, _)) = self.selection_rect {
            self.selection_rect = Some((ax, ay, point.x, point.y));
            self.notify();
            return;
        }
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if self.two_point_active {
            // Rubber-band: keep exactly the anchor + cursor pair so
            // the ribbon renderer paints a live A→B line.
            if active.points.len() < 2 {
                active.points.push(point);
            } else {
                let last = active.points.len() - 1;
                active.points[last] = point;
            }
        } else {
            active.points.push(point);
        }
        self.notify();
    }

    /// Screen-space entrypoint mirroring [`Board::begin`]. Projects the
    /// incoming surface-pixel sample through the current [`Viewport`]
    /// so pen and pointer inputs land in the same world coordinates the
    /// stroke buffer stores.
    ///
    /// Silently drops samples that land inside any overlay rect
    /// published to [`crate::ui_mask`] — a pen tap on the palette
    /// should trigger the button, not the canvas. Gate lives at
    /// `begin` only: once a stroke starts on the canvas, subsequent
    /// samples belong to it even if the pen sweeps over an overlay.
    pub fn begin_screen(&mut self, mut point: InkPoint) {
        if self.gesture_active {
            return;
        }
        if crate::ui_mask::contains(point.x, point.y) {
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
        if let Some(mut session) = self.erase_session.take() {
            self.finalize_erase_session(&mut session);
            if !session.is_empty() {
                self.history.push(HistoryOp::Erase(session));
            }
            self.notify();
            return;
        }
        if let Some((ax, ay, cx, cy)) = self.selection_rect.take() {
            self.erase_strokes_in_rect(ax.min(cx), ay.min(cy), ax.max(cx), ay.max(cy));
            self.notify();
            return;
        }
        if let Some(active) = self.active.take() {
            self.commit_stroke(active);
            self.two_point_active = false;
            self.notify();
        }
    }

    pub fn cancel(&mut self) {
        if self.erase_session.take().is_some() {
            // Deferred model: accumulation never mutates the doc, so
            // dropping the session is a full rollback.
            self.notify();
            return;
        }
        if self.selection_rect.take().is_some() {
            self.notify();
            return;
        }
        if self.active.take().is_some() {
            self.active_layer_at_begin = None;
            self.two_point_active = false;
            self.notify();
        }
    }

    pub fn clear(&mut self) {
        self.doc.clear();
        self.active = None;
        self.active_layer_at_begin = None;
        self.erase_session = None;
        self.selection_rect = None;
        self.two_point_active = false;
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
        let path = self.brush_registry.brush(preset.kind).build_path(
            &preset,
            &stroke,
            self.brush_registry.caps(),
        );
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

    /// Accumulate one eraser sample. Snapshots any touched original
    /// stroke into the active session and records the clip circle —
    /// the doc itself is not mutated here. The final split runs once
    /// at pen-up in [`Board::finalize_erase_session`], keeping the
    /// stroke count minimal and letting cancel be a true no-op.
    fn apply_erase(&mut self, sample: InkPoint) {
        if self.erase_session.is_none() {
            return;
        }
        let preset = self.current_preset;
        let radius = 0.5 * preset.width(sample.pressure_f32(), sample.tilt_f32());
        if radius <= 0.0 {
            return;
        }
        let candidates = self.spatial.query_circle(sample.x, sample.y, radius);
        // Erase gestures are scoped to the active layer only. Strokes
        // on other layers (even overlapping the eraser circle) stay
        // untouched — layer isolation is what users expect from a
        // layered raster tool.
        let active_layer = self.doc.active_layer;

        // Two-phase: collect fresh snapshots under immutable self
        // borrows, then hand them to the session under a mutable one.
        let mut fresh: Vec<(u32, Stroke)> = Vec::new();
        {
            let session = self
                .erase_session
                .as_ref()
                .expect("erase_session presence checked at fn entry");
            for id in candidates {
                if session.contains(id) {
                    continue;
                }
                let Some(&(layer_id, idx)) = self.stroke_index.get(&id) else {
                    continue;
                };
                if layer_id != active_layer {
                    continue;
                }
                let Some(layer) = self.doc.layer(layer_id) else {
                    continue;
                };
                if !layer.visible || layer.locked {
                    continue;
                }
                let stroke = &layer.strokes[idx];
                // Spatial query is bbox-only — confirm actual
                // intersection before snapshotting.
                if !split_polyline(
                    &stroke.points,
                    stroke.cap_start,
                    stroke.cap_end,
                    sample.x,
                    sample.y,
                    radius,
                )
                .touched
                {
                    continue;
                }
                fresh.push((layer_id, stroke.clone()));
            }
        }

        let session = self
            .erase_session
            .as_mut()
            .expect("erase_session presence checked at fn entry");
        session.push_circle(sample.x, sample.y, radius);
        for (layer_id, stroke) in fresh {
            session.snapshot(layer_id, stroke);
        }
    }

    /// Materialize the deferred split for every snapshotted original:
    /// remove the original from the doc and insert the final fragments
    /// derived from cumulatively clipping against every collected
    /// circle. Fragment ids are recorded on the session so
    /// [`Board::rollback_session`] can undo the commit.
    fn finalize_erase_session(&mut self, session: &mut EraseSession) {
        if session.originals.is_empty() {
            return;
        }
        // Circles are Copy-typed tuples, clone is trivial. Owning them
        // locally frees `session` for `added_fragments` pushes below.
        let circles = session.circles.clone();
        // Take originals so we can push to `added_fragments` while
        // iterating. We reinstate the vec at the end — undo needs it.
        let originals = std::mem::take(&mut session.originals);
        let mut restored: Vec<EraseOriginal> = Vec::with_capacity(originals.len());
        for entry in originals {
            let layer_id = entry.layer_id;
            let original = entry.stroke;
            let fragments = cumulative_split(
                &original.points,
                original.cap_start,
                original.cap_end,
                &circles,
            );

            let Some(removed) = self.remove_stroke_indexed(original.id) else {
                // Original vanished between snapshot and commit
                // (shouldn't happen — snapshot is under doc borrow).
                // Still track it for undo symmetry.
                restored.push(EraseOriginal {
                    layer_id,
                    stroke: original,
                });
                continue;
            };
            self.spatial.remove(removed.id, &removed.points);

            let preset = self.doc.preset(original.brush).copied();
            for fragment in fragments {
                if fragment.points.len() < 2 {
                    // Solitary points are hard to see and easy to
                    // accidentally leave behind; drop them so the
                    // eraser fully clears where the user gestured.
                    continue;
                }
                let frag_id = self.doc.allocate_stroke_id();
                session.added_fragments.push(frag_id);
                let new_stroke = Stroke {
                    id: frag_id,
                    brush: original.brush,
                    color: original.color,
                    cap_start: fragment.cap_start,
                    cap_end: fragment.cap_end,
                    points: fragment.points,
                };
                if let Some(preset) = preset.as_ref() {
                    let path = self.brush_registry.brush(preset.kind).build_path(
                        preset,
                        &new_stroke,
                        self.brush_registry.caps(),
                    );
                    self.cached_paths.insert(frag_id, path);
                }
                self.spatial.insert(new_stroke.id, &new_stroke.points);
                if let Some(layer) = self.doc.layer_mut(layer_id) {
                    let new_idx = layer.strokes.len();
                    layer.strokes.push(new_stroke);
                    self.stroke_index.insert(frag_id, (layer_id, new_idx));
                }
            }
            restored.push(EraseOriginal {
                layer_id,
                stroke: original,
            });
        }
        session.originals = restored;
    }

    /// Current eraser mode. Reads through [`Board::brush_config`] so an
    /// unset config resolves to [`EraserMode::Point`] — matches the
    /// pre-config default.
    fn eraser_mode(&self) -> EraserMode {
        match self.current_config() {
            BrushConfig::Eraser { mode } => mode,
            _ => EraserMode::Point,
        }
    }

    /// True when the current brush uses a two-anchor rubber-band
    /// gesture rather than continuous freehand sampling. Covers the
    /// highlighter `straight = true` config and every geometric
    /// [`BrushKind::Shape`] variant. [`Board::begin`] anchors,
    /// [`Board::extend`] replaces the second endpoint, [`Board::end`]
    /// commits the two-point stroke.
    fn is_two_point_stroke_mode(&self) -> bool {
        if matches!(self.current_kind(), BrushKind::Shape(_)) {
            return true;
        }
        matches!(
            self.current_config(),
            BrushConfig::Highlighter { straight: true, .. }
        )
    }

    /// Swap the mode of the currently-active shape brush. No-op when
    /// the current brush is not a shape — callers should only invoke
    /// this from a UI path that already scoped the brush.
    pub fn set_current_shape_mode(&mut self, mode: ShapeMode) {
        if !matches!(self.current_preset.kind, BrushKind::Shape(_)) {
            return;
        }
        self.current_preset.kind = BrushKind::Shape(mode);
        self.notify();
    }

    /// Fold the current per-kind [`BrushConfig`] into a raw pen sample
    /// so downstream renderers stay unaware of user-facing knobs.
    /// Currently only [`BrushConfig::Pen`] transforms samples (via
    /// [`PressureCurve`]); every other kind returns `point` unchanged.
    fn shape_sample(&self, point: InkPoint) -> InkPoint {
        match self.current_config() {
            BrushConfig::Pen { curve } => {
                point.with_pressure_f32(curve.apply(point.pressure_f32()))
            }
            _ => point,
        }
    }

    /// Half-width of the eraser tip at the currently-active preset's
    /// mid-pressure sample. Used as the tap radius for the `Stroke`
    /// eraser mode so the hit test scales with the size slider.
    fn eraser_tap_radius(&self) -> f32 {
        let r = 0.5 * self.current_preset.width(0.5, 0.0);
        r.max(4.0)
    }

    /// Delete the topmost stroke intersecting a tap at `(x, y)` in the
    /// active layer. No-op when no stroke is hit. Records the removal
    /// as a single-original [`EraseSession`] so [`Board::undo`] restores
    /// it exactly.
    fn erase_stroke_at(&mut self, x: f32, y: f32) {
        let radius = self.eraser_tap_radius();
        let active_layer = self.doc.active_layer;
        let candidates = self.spatial.query_circle(x, y, radius);
        let mut best: Option<(usize, u32)> = None;
        for id in candidates {
            let Some(&(layer_id, idx)) = self.stroke_index.get(&id) else {
                continue;
            };
            if layer_id != active_layer {
                continue;
            }
            let Some(layer) = self.doc.layer(layer_id) else {
                continue;
            };
            if !layer.visible || layer.locked {
                continue;
            }
            let stroke = &layer.strokes[idx];
            if !split_polyline(
                &stroke.points,
                stroke.cap_start,
                stroke.cap_end,
                x,
                y,
                radius,
            )
            .touched
            {
                continue;
            }
            if best.is_none_or(|(bidx, _)| idx > bidx) {
                best = Some((idx, id));
            }
        }
        let Some((_, id)) = best else {
            return;
        };
        self.snapshot_and_remove_whole(id, active_layer);
    }

    /// Delete every stroke on the active layer whose polyline
    /// intersects the world-space rect. Empty selection is a no-op —
    /// no history entry is pushed.
    fn erase_strokes_in_rect(&mut self, min_x: f32, min_y: f32, max_x: f32, max_y: f32) {
        if (max_x - min_x).abs() < f32::EPSILON || (max_y - min_y).abs() < f32::EPSILON {
            return;
        }
        let active_layer = self.doc.active_layer;
        let candidates = self.spatial.query_rect(min_x, min_y, max_x, max_y);
        let mut targets: Vec<u32> = Vec::new();
        for id in candidates {
            let Some(&(layer_id, idx)) = self.stroke_index.get(&id) else {
                continue;
            };
            if layer_id != active_layer {
                continue;
            }
            let Some(layer) = self.doc.layer(layer_id) else {
                continue;
            };
            if !layer.visible || layer.locked {
                continue;
            }
            let stroke = &layer.strokes[idx];
            if polyline_intersects_rect(&stroke.points, min_x, min_y, max_x, max_y) {
                targets.push(id);
            }
        }
        if targets.is_empty() {
            return;
        }
        let mut session = EraseSession::default();
        for id in targets {
            let Some(&(layer_id, _)) = self.stroke_index.get(&id) else {
                continue;
            };
            let Some(layer) = self.doc.layer(layer_id) else {
                continue;
            };
            let Some(&(_, idx)) = self.stroke_index.get(&id) else {
                continue;
            };
            let snapshot = layer.strokes[idx].clone();
            session.snapshot(layer_id, snapshot);
            if let Some(removed) = self.remove_stroke_indexed(id) {
                self.spatial.remove(removed.id, &removed.points);
            }
        }
        if !session.is_empty() {
            self.history.push(HistoryOp::Erase(session));
        }
    }

    /// Snapshot a single stroke into a fresh session, remove it from
    /// the doc, and push the session onto the history. Shared by the
    /// `Stroke` mode tap path.
    fn snapshot_and_remove_whole(&mut self, id: u32, layer_id: u32) {
        let Some(layer) = self.doc.layer(layer_id) else {
            return;
        };
        let Some(&(_, idx)) = self.stroke_index.get(&id) else {
            return;
        };
        let snapshot = layer.strokes[idx].clone();
        let mut session = EraseSession::default();
        session.snapshot(layer_id, snapshot);
        if let Some(removed) = self.remove_stroke_indexed(id) {
            self.spatial.remove(removed.id, &removed.points);
        }
        if !session.is_empty() {
            self.history.push(HistoryOp::Erase(session));
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
                let path = self.brush_registry.brush(preset.kind).build_path(
                    preset,
                    original,
                    self.brush_registry.caps(),
                );
                self.cached_paths.insert(id, path);
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
    /// each. Active stroke: rebuilt from scratch every frame via
    /// [`build_stroke_path`] — the unified-polygon ribbon renderer
    /// cannot be composed incrementally without reintroducing seams,
    /// and the per-frame cost is negligible for realistic sample
    /// counts.
    pub fn paint(&mut self, canvas: &freya_engine::prelude::Canvas, surface: SurfaceBounds) {
        // Cache surface size so off-paint mutators (`pan_move`,
        // `viewport_zoom_at`, `viewport_zoom_by`) can consult it when
        // clamping against the current background's content bounds.
        self.last_surface_size = (surface.max_x - surface.min_x, surface.max_y - surface.min_y);
        // Correct any drift accumulated since the last frame — the
        // background may have swapped, the surface may have resized, or
        // an off-path mutator may have pushed the viewport past bounds
        // before the surface size was known.
        self.clamp_viewport_to_background();

        // Everything below draws in world coordinates. Wrapping in a
        // save/restore lets viewport pan+zoom compose freely with any
        // future overlay pass that wants to draw in screen space.
        canvas.save();
        canvas.translate((self.viewport.tx, self.viewport.ty));
        canvas.scale((self.viewport.scale, self.viewport.scale));

        // Background layer (PDF pages, blank paper, image, solid
        // color, etc.) paints first — under the strokes but inside the
        // world transform so page rectangles live in the same
        // coordinate space as the ink.
        let (v_min_x, v_min_y) = self.viewport.screen_to_world(surface.min_x, surface.min_y);
        let (v_max_x, v_max_y) = self.viewport.screen_to_world(surface.max_x, surface.max_y);
        let bg_visible = BgRect {
            min_x: v_min_x,
            min_y: v_min_y,
            max_x: v_max_x,
            max_y: v_max_y,
        };
        let mut bg_ctx = BgPaintCtx {
            canvas,
            visible: bg_visible,
            scale: self.viewport.scale,
            redraw: self.redraw_handle.clone(),
        };
        self.background.paint(&mut bg_ctx);
        self.background.tick(bg_visible, self.viewport.scale);

        // Cull committed strokes to the visible world rect via the
        // spatial index. Skia culls per primitive anyway, but skipping
        // the Paint construction + `draw_path` bookkeeping for
        // off-screen strokes is what actually saves cycles at high
        // zoom — that's the "500 strokes, only 20 visible" regime.
        let visible = self.visible_stroke_set(surface);

        // Live eraser preview. During an in-flight erase gesture the
        // doc is not mutated (see `finalize_erase_session`) — instead
        // we composite a `DstOut` mask over the active layer so the
        // user sees the cut immediately. The mask is scoped to that
        // one layer via `save_layer`: other layers keep their strokes
        // intact, matching the active-layer-only erase semantics.
        let mask_circles = self
            .erase_session
            .as_ref()
            .map(|s| s.circles.as_slice())
            .filter(|c| !c.is_empty());
        let active_layer = self.doc.active_layer;

        for layer in &self.doc.layers {
            if !layer.visible {
                continue;
            }
            let mask_this = mask_circles.is_some() && layer.id == active_layer;
            if mask_this {
                canvas.save_layer(&SaveLayerRec::default());
            }
            let opacity = layer.opacity.clamp(0.0, 1.0);
            for stroke in &layer.strokes {
                if let Some(set) = visible.as_ref() {
                    if !set.contains(&stroke.id) {
                        continue;
                    }
                }
                let Some(cached) = self.cached_paths.get(&stroke.id) else {
                    continue;
                };
                let Some(preset) = self.doc.preset(stroke.brush) else {
                    continue;
                };
                let mut paint = self
                    .brush_registry
                    .brush(preset.kind)
                    .paint(preset, stroke.color);
                if opacity < 0.999 {
                    let base = paint.color();
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let scaled = (f32::from(base.a()) * opacity).clamp(0.0, 255.0) as u8;
                    paint.set_color(base.with_a(scaled));
                }
                canvas.draw_path(cached, &paint);
            }
            if mask_this {
                if let Some(circles) = mask_circles {
                    let mut mask = Paint::default();
                    mask.set_blend_mode(BlendMode::DstOut);
                    mask.set_color(SkColor::from_argb(255, 0, 0, 0));
                    mask.set_anti_alias(true);
                    for &(cx, cy, r) in circles {
                        canvas.draw_circle((cx, cy), r, &mask);
                    }
                }
                canvas.restore();
            }
        }
        if let Some(active) = &self.active {
            if let Some(preset) = self.doc.preset(active.brush) {
                let layer_opacity = self
                    .active_layer_at_begin
                    .and_then(|id| self.doc.layer(id))
                    .map_or(1.0, |l| l.opacity.clamp(0.0, 1.0));
                let renderer = self.brush_registry.brush(preset.kind);
                let mut paint = renderer.paint(preset, active.color);
                if layer_opacity < 0.999 {
                    let base = paint.color();
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let scaled = (f32::from(base.a()) * layer_opacity).clamp(0.0, 255.0) as u8;
                    paint.set_color(base.with_a(scaled));
                }
                let path = renderer.build_path(preset, active, self.brush_registry.caps());
                canvas.draw_path(&path, &paint);
            }
        }
        if let Some((ax, ay, cx, cy)) = self.selection_rect {
            draw_selection_rect(canvas, ax, ay, cx, cy);
        }
        canvas.restore();
    }

    /// Compute the visible stroke set for the current viewport. Returns
    /// `None` when culling is not beneficial (very small doc or 100%
    /// zoom showing "most of it") — the caller then paints every
    /// stroke, matching pre-culling behaviour and skipping the
    /// per-frame hash-set build.
    fn visible_stroke_set(&self, surface: SurfaceBounds) -> Option<HashSet<u32>> {
        // Total committed strokes across every layer. Below this
        // threshold the per-frame `HashSet` build costs more than the
        // draws it saves — full-doc paint is faster.
        //
        // `PAD` widens the query by one spatial bucket on every side so
        // strokes whose sample points lie just outside the viewport
        // still register when their ribbon geometry bleeds in.
        const CULL_MIN_STROKES: usize = 64;
        const PAD: f32 = 128.0;
        let total: usize = self.doc.layers.iter().map(|l| l.strokes.len()).sum();
        if total < CULL_MIN_STROKES {
            return None;
        }
        let (min_x, min_y) = self.viewport.screen_to_world(surface.min_x, surface.min_y);
        let (max_x, max_y) = self.viewport.screen_to_world(surface.max_x, surface.max_y);
        let ids = self
            .spatial
            .query_rect(min_x - PAD, min_y - PAD, max_x + PAD, max_y + PAD);
        Some(ids.into_iter().collect())
    }
}

/// Axis-aligned surface-pixel bounds handed to [`Board::paint`]. The
/// rect is projected into world coordinates via the current
/// [`Viewport`] to drive stroke culling.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceBounds {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

/// Freya canvas element wired to a shared [`Board`].
///
/// The outer rect owns viewport gestures (wheel zoom + middle-button
/// drag pan). Pen and stylus input still land on the [`Board`] through
/// [`crate::pen_pump`] — the freya handlers here operate on the
/// [`Viewport`] only, so simultaneous pen strokes are untouched.
pub fn drawing_surface(board: &Arc<Mutex<Board>>) -> impl IntoElement {
    let render_board = Arc::clone(board);
    // Read the current background's surface backdrop once at element
    // construction. Swapping backgrounds at runtime requires re-mounting
    // this element for the backdrop color to refresh — acceptable while
    // background swaps happen from an app-level open/close action that
    // already forces a top-level re-render. The trait returns a Skia
    // color (paints inside `Board::paint` use that space); freya's
    // element attribute takes freya-core's own `Color`, so we round-trip
    // via ARGB components at the boundary.
    let backdrop_sk = lock(board)
        .background()
        .viewport_backdrop()
        .unwrap_or_else(|| SkColor::from_rgb(250, 250, 248));
    let backdrop = Color::from_argb(
        backdrop_sk.a(),
        backdrop_sk.r(),
        backdrop_sk.g(),
        backdrop_sk.b(),
    );

    let inner = canvas(RenderCallback::new(move |ctx| {
        let mut guard = match render_board.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        // Canvas paints in element-local coordinates — origin at
        // (0, 0), extent given by `ctx.size` in logical pixels. This
        // matches the surface space our pen samples arrive in, so the
        // culling rect projects cleanly through the viewport.
        let bounds = SurfaceBounds {
            min_x: 0.0,
            min_y: 0.0,
            max_x: ctx.size.width,
            max_y: ctx.size.height,
        };
        guard.paint(ctx.canvas, bounds);
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
        .background(backdrop)
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
struct Fragment {
    points: Vec<InkPoint>,
    cap_start: CapStyle,
    cap_end: CapStyle,
}

#[derive(Debug)]
struct SplitOutcome {
    touched: bool,
    fragments: Vec<Fragment>,
}

/// Marquee preview for an in-flight [`EraserMode::SelectionRect`]
/// drag. Fills the rect with a low-alpha wash and outlines it with a
/// solid stroke so the user can see exactly which strokes will be
/// erased on pen-up.
fn draw_selection_rect(canvas: &freya_engine::prelude::Canvas, ax: f32, ay: f32, cx: f32, cy: f32) {
    use freya_engine::prelude::{PaintStyle, Rect};
    let min_x = ax.min(cx);
    let min_y = ay.min(cy);
    let max_x = ax.max(cx);
    let max_y = ay.max(cy);
    let rect = Rect::from_ltrb(min_x, min_y, max_x, max_y);
    let mut fill = Paint::default();
    fill.set_color(SkColor::from_argb(40, 90, 130, 220));
    fill.set_anti_alias(true);
    canvas.draw_rect(rect, &fill);
    let mut outline = Paint::default();
    outline.set_color(SkColor::from_argb(200, 90, 130, 220));
    outline.set_style(PaintStyle::Stroke);
    outline.set_stroke_width(1.5);
    outline.set_anti_alias(true);
    canvas.draw_rect(rect, &outline);
}

/// Does any sample or connecting segment of `points` intersect the
/// axis-aligned rectangle `[min_x, max_x] × [min_y, max_y]`? Used by
/// the [`EraserMode::SelectionRect`] path to filter spatial-broad
/// candidates down to true hits. Two consecutive points that both lie
/// outside the rect can still cross it — Liang-Barsky clip handles
/// that case.
fn polyline_intersects_rect(
    points: &[InkPoint],
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
) -> bool {
    let inside = |p: &InkPoint| p.x >= min_x && p.x <= max_x && p.y >= min_y && p.y <= max_y;
    if points.iter().any(inside) {
        return true;
    }
    let rect = (min_x, min_y, max_x, max_y);
    for pair in points.windows(2) {
        let a = pair[0];
        let b = pair[1];
        if segment_hits_rect((a.x, a.y), (b.x, b.y), rect) {
            return true;
        }
    }
    false
}

/// Liang-Barsky segment vs axis-aligned rectangle. Returns `true` when
/// any portion of segment `AB` lies inside the rect. Endpoints already
/// tested for containment upstream — this fires only when both are
/// outside, so the clip only needs to detect a non-empty intersection.
fn segment_hits_rect(
    a: (f32, f32),
    b: (f32, f32),
    (min_x, min_y, max_x, max_y): (f32, f32, f32, f32),
) -> bool {
    let (ax, ay) = a;
    let (bx, by) = b;
    let dx = bx - ax;
    let dy = by - ay;
    let mut t0: f32 = 0.0;
    let mut t1: f32 = 1.0;
    let clip = |p: f32, q: f32, t0: &mut f32, t1: &mut f32| -> bool {
        if p == 0.0 {
            return q >= 0.0;
        }
        let r = q / p;
        if p < 0.0 {
            if r > *t1 {
                return false;
            }
            if r > *t0 {
                *t0 = r;
            }
        } else {
            if r < *t0 {
                return false;
            }
            if r < *t1 {
                *t1 = r;
            }
        }
        true
    };
    clip(-dx, ax - min_x, &mut t0, &mut t1)
        && clip(dx, max_x - ax, &mut t0, &mut t1)
        && clip(-dy, ay - min_y, &mut t0, &mut t1)
        && clip(dy, max_y - ay, &mut t0, &mut t1)
        && t0 < t1
}

/// Clip `points` against a circle. Fragments outside the circle are
/// preserved; parts inside are dropped, with fresh interpolated
/// vertices inserted at every circle-boundary crossing. Cap style at
/// each endpoint tracks whether the point survived from the input
/// polyline (inherits the input cap) or was produced by a cut
/// (`CapStyle::Flat`) — the renderer terminates the ribbon
/// perpendicular to the local tangent at every cut, avoiding the
/// unwanted round bulge that used to appear where the eraser bit
/// the stroke.
fn split_polyline(
    points: &[InkPoint],
    stroke_cap_start: CapStyle,
    stroke_cap_end: CapStyle,
    cx: f32,
    cy: f32,
    r: f32,
) -> SplitOutcome {
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
    let mut cur_cap: CapStyle = stroke_cap_start;

    if inside(&points[0]) {
        out.touched = true;
        // First sample dropped; next fragment opens at a cut.
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
                    let p_enter = interp(&a, &b, clip0);
                    let p_exit = interp(&a, &b, clip1);
                    cur.push(p_enter);
                    out.fragments.push(Fragment {
                        points: std::mem::take(&mut cur),
                        cap_start: cur_cap,
                        cap_end: CapStyle::Flat,
                    });
                    cur_cap = CapStyle::Flat;
                    cur.push(p_exit);
                }
                cur.push(b);
            }
            (false, true) => {
                out.touched = true;
                if let Some(t) = enter_t(&a, &b, cx, cy, r) {
                    cur.push(interp(&a, &b, t));
                }
                if !cur.is_empty() {
                    out.fragments.push(Fragment {
                        points: std::mem::take(&mut cur),
                        cap_start: cur_cap,
                        cap_end: CapStyle::Flat,
                    });
                }
                cur_cap = CapStyle::Flat;
            }
            (true, false) => {
                out.touched = true;
                cur_cap = CapStyle::Flat;
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
        out.fragments.push(Fragment {
            points: cur,
            cap_start: cur_cap,
            cap_end: stroke_cap_end,
        });
    }
    out
}

/// Iteratively clip `points` against every circle in `circles`,
/// returning the final surviving fragments. Semantically equivalent to
/// applying `split_polyline` per-circle in order, but skips the doc
/// mutation cascade: each intermediate polyline stays local to this
/// call. Fragments shorter than two points are pruned (nothing to
/// render, easy to leave behind accidentally).
fn cumulative_split(
    points: &[InkPoint],
    stroke_cap_start: CapStyle,
    stroke_cap_end: CapStyle,
    circles: &[(f32, f32, f32)],
) -> Vec<Fragment> {
    let mut fragments: Vec<Fragment> = vec![Fragment {
        points: points.to_vec(),
        cap_start: stroke_cap_start,
        cap_end: stroke_cap_end,
    }];
    for &(cx, cy, r) in circles {
        let mut next: Vec<Fragment> = Vec::with_capacity(fragments.len());
        for frag in fragments {
            let out = split_polyline(&frag.points, frag.cap_start, frag.cap_end, cx, cy, r);
            if out.touched {
                for f in out.fragments {
                    if f.points.len() >= 2 {
                        next.push(f);
                    }
                }
            } else {
                next.push(frag);
            }
        }
        fragments = next;
    }
    fragments
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
        let out = split_polyline(&pts, CapStyle::Round, CapStyle::Round, 50.0, 0.0, 10.0);
        assert!(out.touched);
        assert_eq!(out.fragments.len(), 2);
        assert!(out.fragments[0].points.last().unwrap().x <= 40.0 + 1e-3);
        assert!(out.fragments[1].points.first().unwrap().x >= 60.0 - 1e-3);
        // Original endpoints keep their round cap; cut endpoints are flat.
        assert_eq!(out.fragments[0].cap_start, CapStyle::Round);
        assert_eq!(out.fragments[0].cap_end, CapStyle::Flat);
        assert_eq!(out.fragments[1].cap_start, CapStyle::Flat);
        assert_eq!(out.fragments[1].cap_end, CapStyle::Round);
    }

    #[test]
    fn split_leaves_untouched_alone() {
        let pts = vec![pt(0.0, 0.0), pt(100.0, 0.0)];
        let out = split_polyline(&pts, CapStyle::Round, CapStyle::Round, 50.0, 500.0, 10.0);
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
    fn erase_scoped_to_active_layer_only() {
        // Two overlapping strokes on distinct layers. Eraser passes
        // through both spatially but must only cut the stroke on the
        // active layer — the other layer's stroke stays intact.
        let mut b = Board::default();
        b.set_current_preset(BrushPreset::pen());
        b.begin(pt(0.0, 0.0));
        b.extend(pt(200.0, 0.0));
        b.end();
        let l0 = b.active_layer_id();

        let l1 = b.add_layer();
        b.set_active_layer(l1);
        b.begin(pt(0.0, 0.0));
        b.extend(pt(200.0, 0.0));
        b.end();
        assert_eq!(b.doc().layer(l0).unwrap().strokes.len(), 1);
        assert_eq!(b.doc().layer(l1).unwrap().strokes.len(), 1);

        // Active layer is l1 — eraser must only touch l1.
        b.set_current_preset(BrushPreset::eraser());
        b.begin(pt(100.0, 0.0));
        b.end();
        assert_eq!(
            b.doc().layer(l0).unwrap().strokes.len(),
            1,
            "non-active layer must stay untouched"
        );
        assert!(
            b.doc().layer(l1).unwrap().strokes.len() >= 2,
            "active layer must have been split"
        );
    }

    #[test]
    fn erase_accumulation_defers_doc_mutation_and_cancel_is_noop() {
        // Deferred model: the doc is untouched until pen-up.
        // begin+extend on an eraser gesture must leave the stroke
        // list identical, and cancel must drop it wholesale without
        // needing to roll anything back.
        let mut b = Board::default();
        b.set_current_preset(BrushPreset::pen());
        b.begin(pt(0.0, 0.0));
        b.extend(pt(400.0, 0.0));
        b.end();
        let stroke_count = |d: &Doc| d.layers.iter().map(|l| l.strokes.len()).sum::<usize>();
        assert_eq!(stroke_count(b.doc()), 1);

        b.set_current_preset(BrushPreset::eraser());
        b.begin(pt(100.0, 0.0));
        b.extend(pt(200.0, 0.0));
        b.extend(pt(300.0, 0.0));
        assert_eq!(
            stroke_count(b.doc()),
            1,
            "accumulation must not mutate the doc"
        );

        b.cancel();
        assert_eq!(stroke_count(b.doc()), 1);
        assert!(!b.undo(), "cancel must not push a history op");
    }

    #[test]
    fn multi_sample_erase_undo_restores_original_count() {
        // Erase gesture crossing a long stroke re-splits its own
        // intermediate fragments. Undo must land back at the exact
        // pre-gesture stroke count — not accumulate ghosts of the
        // intermediates as if they had existed at pen-down.
        let mut b = Board::default();
        b.set_current_preset(BrushPreset::pen());
        b.begin(pt(0.0, 0.0));
        b.extend(pt(400.0, 0.0));
        b.end();
        let stroke_count = |d: &Doc| d.layers.iter().map(|l| l.strokes.len()).sum::<usize>();
        assert_eq!(stroke_count(b.doc()), 1);

        b.set_current_preset(BrushPreset::eraser());
        b.begin(pt(50.0, 0.0));
        b.extend(pt(150.0, 0.0));
        b.extend(pt(250.0, 0.0));
        b.extend(pt(350.0, 0.0));
        b.end();
        assert!(stroke_count(b.doc()) >= 2, "erase should have split");

        assert!(b.undo());
        assert_eq!(stroke_count(b.doc()), 1);
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
