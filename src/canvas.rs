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
use freya_engine::prelude::{
    BlendMode, Color as SkColor, Paint, PaintStyle, Path, PathBuilder, SaveLayerRec,
};

use crate::bookmark::{Bookmark, Rgba, SourceRef, StrokeAnchor, TimestampMs, WorldPoint};
use crate::brush::{
    BrushConfig, BrushKind, BrushPreset, CapStyle, EraserMode, InkPoint, ShapeMode, Stroke,
};
use crate::doc::Doc;
use crate::doc_op::DocOp;
use crate::ids::{BookmarkId, StrokeId};
use crate::render::{BrushRegistry, HighlighterBrush, HighlighterState, PointerStyle};
use crate::spatial::SpatialIndex;
use crate::undo::UndoStack;

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
    cached_paths: HashMap<StrokeId, Path>,
    /// `stroke_id → (layer_id, within-layer stroke index)`. Kills the
    /// O(N) `position` scan the eraser used to run per candidate hit
    /// and localises index shifting to the affected layer.
    stroke_index: HashMap<StrokeId, (u32, usize)>,
    undo: UndoStack,
    erase_session: Option<EraseAccum>,
    /// Active [`EraserMode::SelectionRect`] drag, expressed as
    /// `(anchor_x, anchor_y, cursor_x, cursor_y)` in world coords.
    /// `Some` while the pen is down; `None` between drags. The paint
    /// pass reads this to overlay the marquee preview.
    selection_rect: Option<(f32, f32, f32, f32)>,
    notifier: Option<RedrawNotifier>,
    /// Fires once per doc-mutating commit (stroke end, erase finalize,
    /// undo apply, layer attr change, clear). The autosave worker owns
    /// the receiver and coalesces bursts into a single `save_doc`.
    /// `None` outside the canvas views so palette-only sessions never
    /// touch disk.
    commit_tx: Option<flume::Sender<()>>,
    /// Fires on every toolbar-pref mutation (preset / size / colour /
    /// input mode). Drained by a global worker that bincode-encodes
    /// [`crate::prefs::Prefs`] to disk. Independent of `commit_tx` so a
    /// pen-up never also triggers a doc save when only the preset
    /// changed.
    prefs_tx: Option<flume::Sender<()>>,
    /// Fires on every viewport mutation (pan / zoom / reset). Drained
    /// by a per-doc worker that writes the view sidecar. Separate from
    /// `commit_tx` so pan/zoom churn does not rewrite the whole doc.
    view_tx: Option<flume::Sender<()>>,
    /// Live pointer position in surface pixels — pen hover, pen
    /// contact, or mouse move. Drives the overlay disc rendered in the
    /// paint pass so the user can gauge brush / eraser radius against
    /// the stroke they're about to lay down. `None` outside hover
    /// range (pen pulled away, mouse left the canvas, finger-only
    /// gesture).
    pointer: Option<(f32, f32)>,
    /// Interprets single-point pen samples (`pen_pump`) as either drawing
    /// strokes or panning the viewport. Multi-touch gestures are not
    /// affected — two-finger pinch keeps the same semantics in both modes.
    input_mode: InputMode,
    viewport: Viewport,
    /// Last surface-pixel cursor observed while a middle-drag pan is
    /// in flight. `Some` gates every `on_global_pointer_move` sample as
    /// a pan step; `None` means no pan currently active.
    pan_anchor: Option<(f32, f32)>,
    /// Live surface-pixel positions of every finger currently in
    /// contact. Populated from `on_touch_start` / `on_touch_move` and
    /// pruned by `on_touch_end` / `on_touch_cancel`. One finger drives
    /// a viewport pan (see [`Self::touch_pan_finger`]); two or more
    /// flip [`Self::gesture_active`] on and drive pinch-zoom instead.
    /// Pen input stays on its own channel either way.
    finger_positions: HashMap<u64, (f32, f32)>,
    gesture_baseline: Option<GestureBaseline>,
    /// True while two or more fingers are down. Suppresses stroke
    /// input coming through the `_screen` entrypoints so the viewport
    /// gesture doesn't share the pen path.
    gesture_active: bool,
    /// Finger id currently driving a single-touch viewport pan. `Some`
    /// between the first finger landing and either its lift or a
    /// second finger arriving (which promotes to pinch). Lets the user
    /// drag the canvas with a finger while the pen keeps drawing — the
    /// pen pump owns stroke state, this field owns viewport state.
    touch_pan_finger: Option<u64>,
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

/// Deferred-split accumulator for an eraser gesture. Collects every
/// clip circle and a one-shot snapshot per touched stroke; the actual
/// `RemoveStroke` + `PushStroke` ops fire once at pen-up inside a
/// single [`UndoStack`] transaction so the whole gesture undoes as
/// one entry.
#[derive(Debug, Default)]
struct EraseAccum {
    /// `(layer_id, stroke)` captured the first time each stroke was
    /// grazed. Kept so finalize can clip each original against the
    /// cumulative circle set in one pass.
    originals: Vec<(u32, Stroke)>,
    circles: Vec<(f32, f32, f32)>,
    touched: HashSet<StrokeId>,
}

impl EraseAccum {
    fn push_circle(&mut self, cx: f32, cy: f32, r: f32) {
        self.circles.push((cx, cy, r));
    }

    fn snapshot(&mut self, layer_id: u32, stroke: Stroke) -> bool {
        if !self.touched.insert(stroke.id) {
            return false;
        }
        self.originals.push((layer_id, stroke));
        true
    }

    fn contains(&self, id: StrokeId) -> bool {
        self.touched.contains(&id)
    }
}

/// How single-point pen input maps onto the board. Toggled from the UI;
/// multi-touch gestures ignore this and always pinch-zoom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Pen samples build strokes (current workflow).
    Draw,
    /// Pen samples drive the viewport — down/move/up translate into
    /// `pan_begin` / `pan_move` / `pan_end`. Useful on touchscreens where
    /// the user wants to drag the canvas with a single finger.
    Pan,
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
            undo: UndoStack::default(),
            erase_session: None,
            selection_rect: None,
            notifier: None,
            commit_tx: None,
            prefs_tx: None,
            view_tx: None,
            pointer: None,
            input_mode: InputMode::Draw,
            viewport: Viewport::default(),
            pan_anchor: None,
            finger_positions: HashMap::new(),
            gesture_baseline: None,
            gesture_active: false,
            touch_pan_finger: None,
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

    /// Install (or replace) the autosave sink. Called by the canvas-view
    /// mount hook; the matching receiver drives a worker that snapshots
    /// `doc()` and persists it after every commit. Dropping the old
    /// sender wakes the previous worker with `Disconnected` so it exits
    /// cleanly on the next remount.
    pub fn set_commit_sink(&mut self, tx: flume::Sender<()>) {
        self.commit_tx = Some(tx);
    }

    /// Drop the current commit sink. Any worker holding the matching
    /// receiver wakes on `Disconnected` and exits. Used by ephemeral
    /// canvas flows that must not autosave onto the previous mount's
    /// item.
    pub fn clear_commit_sink(&mut self) {
        self.commit_tx = None;
    }

    /// Install (or replace) the global toolbar-prefs sink.
    pub fn set_prefs_sink(&mut self, tx: flume::Sender<()>) {
        self.prefs_tx = Some(tx);
    }

    /// Install (or replace) the per-doc viewport sink.
    pub fn set_view_sink(&mut self, tx: flume::Sender<()>) {
        self.view_tx = Some(tx);
    }

    /// Drop the current viewport sink. Mirror of
    /// [`Self::clear_commit_sink`] for the ephemeral flow.
    pub fn clear_view_sink(&mut self) {
        self.view_tx = None;
    }

    /// Update the live pointer position in surface pixels. `None`
    /// hides the overlay. Fires `notify` so the paint pass sees the
    /// new position on the next frame — the pump threads and the
    /// freya pointer handlers all route through this one setter.
    pub fn set_pointer(&mut self, pos: Option<(f32, f32)>) {
        if self.pointer == pos {
            return;
        }
        self.pointer = pos;
        self.notify();
    }

    #[must_use]
    pub const fn pointer(&self) -> Option<(f32, f32)> {
        self.pointer
    }

    /// Fired at the end of every doc-mutating public method after the
    /// in-memory state is consistent — see [`Self::notify`] for the
    /// repaint half.
    fn ping_commit(&self) {
        if let Some(tx) = &self.commit_tx {
            // `try_send` on unbounded can only fail on Disconnected,
            // which just means the autosave worker has already torn
            // down. The next mount reinstalls the sink.
            let _ = tx.try_send(());
        }
    }

    fn ping_prefs(&self) {
        if let Some(tx) = &self.prefs_tx {
            let _ = tx.try_send(());
        }
    }

    fn ping_view(&self) {
        if let Some(tx) = &self.view_tx {
            let _ = tx.try_send(());
        }
    }

    fn notify_commit(&self) {
        self.notify();
        self.ping_commit();
    }

    #[must_use]
    pub const fn input_mode(&self) -> InputMode {
        self.input_mode
    }

    /// Flip between [`InputMode::Draw`] and [`InputMode::Pan`]. Cancels
    /// any in-flight stroke / pan so the next pen sample starts fresh
    /// under the new interpretation.
    pub fn set_input_mode(&mut self, mode: InputMode) {
        if self.input_mode == mode {
            return;
        }
        self.input_mode = mode;
        self.cancel();
        self.pan_end();
        self.notify();
        self.ping_prefs();
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
        self.ping_prefs();
    }

    pub fn set_current_color(&mut self, color: [u8; 4]) {
        self.current_color = color;
        self.ping_prefs();
    }

    /// Update the active tool's size multiplier (clamped to a sane
    /// range). Persisted per-kind so the value survives palette
    /// switches.
    pub fn set_current_size(&mut self, scale: f32) {
        let clamped = scale.clamp(SIZE_SCALE_MIN, SIZE_SCALE_MAX);
        self.current_preset.size_scale = clamped;
        self.size_scales.insert(self.current_preset.kind, clamped);
        self.ping_prefs();
    }

    /// Prime the per-kind size memory without changing the active
    /// preset. Used by the prefs-restore path to repopulate every
    /// remembered size before `set_current_preset` folds the matching
    /// one into the live tool.
    pub fn set_size_scale(&mut self, kind: BrushKind, scale: f32) {
        let clamped = scale.clamp(SIZE_SCALE_MIN, SIZE_SCALE_MAX);
        self.size_scales.insert(kind, clamped);
    }

    /// Snapshot of every remembered per-kind size. Used by the prefs
    /// save path to persist the full map without exposing interior
    /// mutability.
    #[must_use]
    pub fn size_scales_snapshot(&self) -> Vec<(BrushKind, f32)> {
        self.size_scales.iter().map(|(k, v)| (*k, *v)).collect()
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
        self.ping_view();
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
        self.ping_view();
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
        self.ping_view();
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

    /// Register a new touch point. The first finger latches a
    /// single-touch viewport pan so the user can drag the canvas while
    /// the pen keeps drawing on its own channel. A second finger
    /// promotes to pinch-zoom and rolls back any in-flight stroke.
    ///
    /// Touches landing within [`crate::pen_pump::PEN_TOUCH_FILTER_RADIUS`]
    /// of a recent pen sample are dropped — some platforms deliver
    /// stylus contact as both an istmo-pen event AND a freya touch
    /// event, which would otherwise fire a spurious finger-pan on
    /// every stroke.
    pub fn touch_down(&mut self, id: u64, sx: f32, sy: f32) {
        if crate::pen_pump::is_pen_near(sx, sy, crate::pen_pump::PEN_TOUCH_FILTER_RADIUS) {
            return;
        }
        self.finger_positions.insert(id, (sx, sy));
        let n = self.finger_positions.len();
        if n == 1 {
            // Pen stays on the pen pump; this just moves the viewport.
            self.touch_pan_finger = Some(id);
            self.pan_begin(sx, sy);
        } else if n >= 2 && !self.gesture_active {
            self.gesture_active = true;
            self.cancel();
            // Drop any pan anchor so the second finger lands a clean
            // pinch-zoom instead of fighting a stale translate (either
            // from our own single-finger pan above or from the pen
            // pump in `InputMode::Pan`).
            self.pan_end();
            self.touch_pan_finger = None;
        }
        self.gesture_baseline = self.compute_gesture_baseline();
    }

    /// Update a tracked finger. Single-finger moves pan the viewport;
    /// two-finger moves pinch-zoom around the centroid.
    pub fn touch_move(&mut self, id: u64, sx: f32, sy: f32) {
        if !self.finger_positions.contains_key(&id) {
            return;
        }
        self.finger_positions.insert(id, (sx, sy));
        if self.gesture_active {
            let Some(new) = self.compute_gesture_baseline() else {
                return;
            };
            if let Some(prev) = self.gesture_baseline {
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
            return;
        }
        if self.touch_pan_finger == Some(id) {
            self.pan_move(sx, sy);
        }
    }

    /// Drop a finger from the tracker. Leaving multi-finger mode clears
    /// pinch state and re-latches the pan onto whichever finger is
    /// still down, so a 2→1 lift transitions seamlessly from pinch
    /// back to drag.
    pub fn touch_up(&mut self, id: u64) {
        self.finger_positions.remove(&id);
        if self.touch_pan_finger == Some(id) {
            self.touch_pan_finger = None;
            self.pan_end();
        }
        if self.finger_positions.len() < 2 {
            self.gesture_active = false;
            self.gesture_baseline = None;
            if self.touch_pan_finger.is_none() {
                if let Some((&remaining_id, &(sx, sy))) = self.finger_positions.iter().next() {
                    self.touch_pan_finger = Some(remaining_id);
                    self.pan_begin(sx, sy);
                }
            }
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
        self.undo.clear();
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
                    self.erase_session = Some(EraseAccum::default());
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
            self.undo.begin();
            self.finalize_erase_session(&mut session);
            self.undo.commit();
            self.notify_commit();
            return;
        }
        if let Some((ax, ay, cx, cy)) = self.selection_rect.take() {
            self.erase_strokes_in_rect(ax.min(cx), ay.min(cy), ax.max(cx), ay.max(cy));
            self.notify_commit();
            return;
        }
        if let Some(active) = self.active.take() {
            self.commit_stroke(active);
            self.two_point_active = false;
            self.notify_commit();
        }
    }

    pub fn cancel(&mut self) {
        if self.erase_session.take().is_some() {
            // Deferred model: accumulation never mutates the doc, so
            // dropping the accumulator is a full rollback. No undo
            // transaction was opened (we only begin one at pen-up
            // inside `end`), so nothing to abort.
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
        // Single op with a snapshot inverse — undo rebuilds every
        // layer + stroke the clear removed. UI-only state (in-flight
        // stroke, erase accumulator, selection marquee) resets around
        // the op since those are not part of the doc.
        self.active = None;
        self.active_layer_at_begin = None;
        self.erase_session = None;
        self.selection_rect = None;
        self.two_point_active = false;
        self.apply_op(DocOp::Clear);
        self.notify_commit();
    }

    /// Undo the most recent user action. Pops the top transaction,
    /// re-emits every pre-captured inverse op through `Doc::emit`
    /// so the log stays append-only (CRDT contract), and parks the
    /// forward ops on the redo stack. Returns `true` iff something
    /// was undone.
    pub fn undo(&mut self) -> bool {
        let Some(entry) = self.undo.pop_undo() else {
            return false;
        };
        for op in entry.inverse.iter().cloned() {
            self.apply_op_untracked(op);
        }
        self.undo.push_redo(entry);
        self.notify_commit();
        true
    }

    /// Count of pending undo entries. For UI gates ("Undo" button
    /// enabled / disabled) and for tests.
    #[must_use]
    pub fn undo_depth(&self) -> usize {
        self.undo.undo_len()
    }

    /// Count of pending redo entries. Mirror of [`Self::undo_depth`].
    #[must_use]
    pub fn redo_depth(&self) -> usize {
        self.undo.redo_len()
    }

    /// Re-apply the most recently undone transaction. Mirror of
    /// [`Self::undo`] — pops redo, re-emits every forward op through
    /// `Doc::emit`, parks the entry back on the undo stack. Returns
    /// `true` iff something was redone.
    pub fn redo(&mut self) -> bool {
        let Some(entry) = self.undo.pop_redo() else {
            return false;
        };
        for op in entry.forward.iter().cloned() {
            self.apply_op_untracked(op);
        }
        self.undo.push_undo_raw(entry);
        self.notify_commit();
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
    /// the new id. `AddLayer` + `SetActiveLayer` group into a single
    /// undo entry so the pair rolls back together.
    pub fn add_layer(&mut self) -> u32 {
        let n = self.doc.layers.len() + 1;
        let id = self.doc.next_layer_id;
        self.undo.begin();
        self.apply_op(DocOp::AddLayer {
            id,
            name: format!("Layer {n}"),
        });
        self.apply_op(DocOp::SetActiveLayer { id });
        self.undo.commit();
        self.notify_commit();
        id
    }

    /// Remove the currently active layer. No-op if it's the only
    /// layer. Derived index entries + the in-flight stroke (if its
    /// target layer vanished) are cleaned up by [`Self::apply_op`].
    pub fn remove_active_layer(&mut self) {
        let id = self.doc.active_layer;
        if self.doc.layers.len() <= 1 || self.doc.layer(id).is_none() {
            return;
        }
        if self.active_layer_at_begin == Some(id) {
            self.active = None;
            self.active_layer_at_begin = None;
        }
        self.apply_op(DocOp::RemoveLayer { id });
        self.notify_commit();
    }

    pub fn set_active_layer(&mut self, id: u32) {
        if self.doc.active_layer == id || self.doc.layer(id).is_none() {
            return;
        }
        self.apply_op(DocOp::SetActiveLayer { id });
        self.notify_commit();
    }

    /// All committed stroke ids within `radius` world-units of
    /// `(world_x, world_y)`. Broad-phase via [`SpatialIndex`] only —
    /// callers that need pixel-tight filtering do their own hit test
    /// against each stroke's polyline. Used by the bookmark pin
    /// "sticky anchor" lookup (Phase 1+); exposed on `Board` so plugin
    /// code never reaches into the private spatial index.
    #[must_use]
    pub fn strokes_near(&self, world_x: f32, world_y: f32, radius: f32) -> Vec<StrokeId> {
        self.spatial.query_circle(world_x, world_y, radius)
    }

    pub fn set_layer_visible(&mut self, id: u32, visible: bool) {
        let Some(layer) = self.doc.layer(id) else {
            return;
        };
        if layer.visible == visible {
            return;
        }
        self.apply_op(DocOp::SetLayerVisible { id, visible });
        self.notify_commit();
    }

    pub fn set_layer_locked(&mut self, id: u32, locked: bool) {
        let Some(layer) = self.doc.layer(id) else {
            return;
        };
        if layer.locked == locked {
            return;
        }
        self.apply_op(DocOp::SetLayerLocked { id, locked });
        self.notify_commit();
    }

    pub fn set_layer_opacity(&mut self, id: u32, opacity: f32) {
        let Some(layer) = self.doc.layer(id) else {
            return;
        };
        let clamped = opacity.clamp(0.0, 1.0);
        if (layer.opacity - clamped).abs() < f32::EPSILON {
            return;
        }
        self.apply_op(DocOp::SetLayerOpacity {
            id,
            opacity: clamped,
        });
        self.notify_commit();
    }

    /// Add a bookmark at `world` in world coordinates. If a stroke
    /// lies within `sticky_radius` world-units of the point, the pin
    /// adopts a [`StrokeAnchor`] pointing at the nearest one so later
    /// stroke moves drag it along. Returns the fresh id so callers
    /// can route it into UI focus / inline refs.
    pub fn add_bookmark(
        &mut self,
        world: WorldPoint,
        sticky_radius: f32,
        timestamp_ms: TimestampMs,
    ) -> BookmarkId {
        let id = BookmarkId::new_v4();
        let mut bm = Bookmark::new(id, world, timestamp_ms);
        bm.stuck_to = self.resolve_sticky(world, sticky_radius);
        self.apply_op(DocOp::AddBookmark { bookmark: bm });
        self.notify_commit();
        id
    }

    /// LWW update of the mutable body / refs / color trio. Pass
    /// `updated_at` from a monotonic clock so the undo entry preserves
    /// the user-facing timestamp.
    pub fn update_bookmark(
        &mut self,
        id: BookmarkId,
        body: String,
        refs: Vec<SourceRef>,
        color: Option<Rgba>,
        updated_at: TimestampMs,
    ) -> bool {
        if self.doc.bookmark(id).is_none() {
            return false;
        }
        self.apply_op(DocOp::UpdateBookmark {
            id,
            body,
            refs,
            color,
            updated_at,
        });
        self.notify_commit();
        true
    }

    pub fn delete_bookmark(&mut self, id: BookmarkId) -> bool {
        if self.doc.bookmark(id).is_none() {
            return false;
        }
        self.apply_op(DocOp::DeleteBookmark { id });
        self.notify_commit();
        true
    }

    /// Re-peg a bookmark to a stroke (or clear the link). The world
    /// anchor stays in the stored `Bookmark::anchor` as a fallback for
    /// when the sticky target disappears later.
    pub fn stick_bookmark(&mut self, id: BookmarkId, to: Option<StrokeAnchor>) -> bool {
        if self.doc.bookmark(id).is_none() {
            return false;
        }
        self.apply_op(DocOp::StickBookmark { id, to });
        self.notify_commit();
        true
    }

    #[must_use]
    pub fn bookmarks(&self) -> &[Bookmark] {
        self.doc.bookmarks()
    }

    /// Live world-space position of a bookmark. When `stuck_to` is
    /// `Some` and the target stroke still exists, the position tracks
    /// `stroke.bbox.origin + offset_in_bbox` so the pin follows later
    /// `MoveStroke` ops. Falls back to the stored `anchor` when the
    /// sticky target is missing (chip ⚠ "ancla rota" will indicate it
    /// at the render layer).
    #[must_use]
    pub fn bookmark_world_position(&self, bm: &Bookmark) -> WorldPoint {
        let Some(anchor) = bm.stuck_to else {
            return bm.anchor;
        };
        let Some(stroke) = self.doc.find_stroke(anchor.stroke) else {
            return bm.anchor;
        };
        let Some(first) = stroke.points.first() else {
            return bm.anchor;
        };
        let (min_x, min_y) = stroke
            .points
            .iter()
            .fold((first.x, first.y), |(ax, ay), p| (ax.min(p.x), ay.min(p.y)));
        WorldPoint::new(
            min_x + anchor.offset_in_bbox.0,
            min_y + anchor.offset_in_bbox.1,
        )
    }

    /// Topmost bookmark whose live screen position lies within
    /// `radius_px` of `(screen_x, screen_y)`. Hit-tested in screen
    /// space so the tap target stays constant across zoom levels.
    /// `None` when no pin is within range.
    #[must_use]
    pub fn bookmark_near_screen(
        &self,
        screen_x: f32,
        screen_y: f32,
        radius_px: f32,
    ) -> Option<BookmarkId> {
        let mut best: Option<(f32, BookmarkId)> = None;
        for bm in self.doc.bookmarks() {
            let world = self.bookmark_world_position(bm);
            let (sx, sy) = self.viewport.world_to_screen(world.x, world.y);
            let dist = (screen_x - sx).hypot(screen_y - sy);
            if dist > radius_px {
                continue;
            }
            if best.is_none_or(|(d, _)| dist < d) {
                best = Some((dist, bm.id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// Nearest stroke within `radius` of `world`, expressed as a
    /// [`StrokeAnchor`] whose `offset_in_bbox` lets the pin follow the
    /// stroke under later `MoveStroke` ops (Phase 2+). `None` when no
    /// stroke lies in range — caller stores the pin as a free world
    /// position.
    fn resolve_sticky(&self, world: WorldPoint, radius: f32) -> Option<StrokeAnchor> {
        if radius <= 0.0 {
            return None;
        }
        let candidates = self.spatial.query_circle(world.x, world.y, radius);
        let mut best: Option<(f32, StrokeId, (f32, f32))> = None;
        for id in candidates {
            let Some(stroke) = self.doc.find_stroke(id) else {
                continue;
            };
            let Some((bbx, bby)) = stroke.points.first().map(|p| (p.x, p.y)) else {
                continue;
            };
            let (min_x, min_y) = stroke
                .points
                .iter()
                .fold((bbx, bby), |(ax, ay), p| (ax.min(p.x), ay.min(p.y)));
            let dx = world.x - min_x;
            let dy = world.y - min_y;
            // Distance metric: Euclidean from world to AABB origin.
            // The spatial query already pruned by bbox radius, so this
            // is just a tie-breaker when several strokes share the
            // bucket.
            let dist = dx.hypot(dy);
            if best.is_none_or(|(d, _, _)| dist < d) {
                best = Some((dist, id, (dx, dy)));
            }
        }
        best.map(|(_, stroke, offset_in_bbox)| StrokeAnchor {
            stroke,
            offset_in_bbox,
        })
    }

    fn commit_stroke(&mut self, stroke: Stroke) {
        let layer_id = self
            .active_layer_at_begin
            .take()
            .unwrap_or(self.doc.active_layer);
        if stroke.points.is_empty() {
            return;
        }
        if self.doc.preset(stroke.brush).is_none() {
            return;
        }
        if self.doc.layer(layer_id).is_none() {
            return;
        }
        self.apply_op(DocOp::PushStroke { layer_id, stroke });
    }

    /// Record one op in the undo stack and apply it to the doc + the
    /// derived indices. The transaction grouping (if any) is implied
    /// by the surrounding `undo.begin` / `undo.commit` pair.
    ///
    /// Every user-driven mutation that produces a `DocOp` flows
    /// through this method. Undo and redo use [`Self::apply_op_untracked`]
    /// so re-applying a pre-captured inverse / forward op does not
    /// append another entry.
    fn apply_op(&mut self, op: DocOp) {
        let inverse = self.doc.inverse_before_apply(&op);
        self.apply_op_untracked(op.clone());
        self.undo.record(op, inverse);
    }

    /// Apply a `DocOp` through `Doc::emit` and reconcile the derived
    /// state (spatial index, cached paths, `stroke_index`). No undo
    /// recording — used by both the forward path (indirectly via
    /// [`Self::apply_op`]) and the undo / redo replays.
    fn apply_op_untracked(&mut self, op: DocOp) {
        match op {
            DocOp::RemoveStroke { id } => {
                // Capture spatial payload + index position before the
                // doc drops the stroke — SpatialIndex is a reverse
                // index keyed by (bucket, id) and needs the point set
                // to prune every bucket cheaply.
                let Some((layer_id, points, idx)) = self.capture_stroke_derived(id) else {
                    self.doc.emit(DocOp::RemoveStroke { id });
                    return;
                };
                self.doc.emit(DocOp::RemoveStroke { id });
                self.spatial.remove(id, &points);
                self.stroke_index.remove(&id);
                for entry in self.stroke_index.values_mut() {
                    if entry.0 == layer_id && entry.1 > idx {
                        entry.1 -= 1;
                    }
                }
                self.cached_paths.remove(&id);
            }
            DocOp::RemoveLayer { id } => {
                let snapshot: Vec<(StrokeId, Vec<InkPoint>)> = self
                    .doc
                    .layer(id)
                    .map(|l| l.strokes.iter().map(|s| (s.id, s.points.clone())).collect())
                    .unwrap_or_default();
                self.doc.emit(DocOp::RemoveLayer { id });
                for (sid, pts) in snapshot {
                    self.spatial.remove(sid, &pts);
                    self.stroke_index.remove(&sid);
                    self.cached_paths.remove(&sid);
                }
            }
            DocOp::Clear => {
                self.doc.emit(DocOp::Clear);
                self.spatial.clear();
                self.cached_paths.clear();
                self.stroke_index.clear();
            }
            DocOp::PushStroke { layer_id, stroke } => {
                let id = stroke.id;
                let preset = self.doc.preset(stroke.brush).copied();
                self.doc.emit(DocOp::PushStroke {
                    layer_id,
                    stroke: stroke.clone(),
                });
                if let Some(preset) = preset {
                    let path = self.brush_registry.brush(preset.kind).build_path(
                        &preset,
                        &stroke,
                        self.brush_registry.caps(),
                    );
                    self.cached_paths.insert(id, path);
                }
                self.spatial.insert(id, &stroke.points);
                if let Some(layer) = self.doc.layer(layer_id) {
                    let idx = layer
                        .strokes
                        .iter()
                        .rposition(|s| s.id == id)
                        .unwrap_or(layer.strokes.len().saturating_sub(1));
                    self.stroke_index.insert(id, (layer_id, idx));
                }
            }
            // Layer attr, brush registry and bookmark ops don't touch
            // the stroke-derived indices.
            op @ (DocOp::RegisterBrush { .. }
            | DocOp::AddLayer { .. }
            | DocOp::SetActiveLayer { .. }
            | DocOp::SetLayerVisible { .. }
            | DocOp::SetLayerLocked { .. }
            | DocOp::SetLayerOpacity { .. }
            | DocOp::AddBookmark { .. }
            | DocOp::UpdateBookmark { .. }
            | DocOp::DeleteBookmark { .. }
            | DocOp::StickBookmark { .. }) => {
                self.doc.emit(op);
            }
        }
    }

    /// Snapshot the derived-state keys for `id`: `(layer, points, idx)`.
    /// `None` when the stroke lives in the doc but has no derived
    /// state (shouldn't happen under invariant — doc + indices stay
    /// in sync) OR when the stroke has already been removed.
    fn capture_stroke_derived(&self, id: StrokeId) -> Option<(u32, Vec<InkPoint>, usize)> {
        let &(layer_id, idx) = self.stroke_index.get(&id)?;
        let layer = self.doc.layer(layer_id)?;
        let stroke = layer.strokes.get(idx)?;
        if stroke.id != id {
            return None;
        }
        Some((layer_id, stroke.points.clone(), idx))
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
    /// emit a `RemoveStroke` for the original + a `PushStroke` for
    /// each resulting fragment. The caller wraps this in a single
    /// [`UndoStack`] transaction, so the whole gesture rolls back as
    /// one undo entry.
    fn finalize_erase_session(&mut self, session: &mut EraseAccum) {
        if session.originals.is_empty() {
            return;
        }
        let circles = std::mem::take(&mut session.circles);
        let originals = std::mem::take(&mut session.originals);
        for (layer_id, original) in originals {
            let fragments = cumulative_split(
                &original.points,
                original.cap_start,
                original.cap_end,
                &circles,
            );
            self.apply_op(DocOp::RemoveStroke { id: original.id });
            for fragment in fragments {
                if fragment.points.len() < 2 {
                    // Solitary points are hard to see and easy to
                    // accidentally leave behind; drop them so the
                    // eraser fully clears where the user gestured.
                    continue;
                }
                let frag = Stroke {
                    id: self.doc.allocate_stroke_id(),
                    brush: original.brush,
                    color: original.color,
                    cap_start: fragment.cap_start,
                    cap_end: fragment.cap_end,
                    points: fragment.points,
                };
                self.apply_op(DocOp::PushStroke {
                    layer_id,
                    stroke: frag,
                });
            }
        }
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
    /// active layer. No-op when no stroke is hit. Emits a single
    /// `RemoveStroke` op so [`Board::undo`] restores it exactly.
    fn erase_stroke_at(&mut self, x: f32, y: f32) {
        let radius = self.eraser_tap_radius();
        let active_layer = self.doc.active_layer;
        let candidates = self.spatial.query_circle(x, y, radius);
        let mut best: Option<(usize, StrokeId)> = None;
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
        let _ = active_layer;
        self.apply_op(DocOp::RemoveStroke { id });
    }

    /// Delete every stroke on the active layer whose polyline
    /// intersects the world-space rect. Empty selection is a no-op —
    /// no undo entry is pushed. All removals fall under one transaction
    /// so a marquee erase undoes in a single step.
    fn erase_strokes_in_rect(&mut self, min_x: f32, min_y: f32, max_x: f32, max_y: f32) {
        if (max_x - min_x).abs() < f32::EPSILON || (max_y - min_y).abs() < f32::EPSILON {
            return;
        }
        let active_layer = self.doc.active_layer;
        let candidates = self.spatial.query_rect(min_x, min_y, max_x, max_y);
        let mut targets: Vec<StrokeId> = Vec::new();
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
        self.undo.begin();
        for id in targets {
            self.apply_op(DocOp::RemoveStroke { id });
        }
        self.undo.commit();
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

        // Pointer overlay paints in surface-pixel space — the brush's
        // world-unit radius multiplied by the current zoom matches the
        // on-screen stroke width exactly. Suppressed during multi-touch
        // gestures (pinch is not a drawing intent) and during a middle-
        // drag pan (the mouse is anchoring the viewport, not hovering a
        // target).
        if let Some((px, py)) = self.pointer
            && !self.gesture_active
            && !self.is_panning()
        {
            let preset = self.current_preset;
            let color = self.current_color;
            let style = self
                .brush_registry
                .brush(preset.kind)
                .pointer_style(&preset, color);
            draw_pointer_overlay(canvas, px, py, self.viewport.scale, style);
        }

        // Bookmark pins. Rendered after the overlay so pins sit on
        // top of the in-flight pointer disc; drawn in screen space so
        // the tap target stays a constant 16 px regardless of zoom.
        // Stuck pins track their stroke's current AABB — see
        // `bookmark_world_position` for the fallback / broken-anchor
        // rules.
        //
        // Pins are collected first so the immutable borrow of
        // `self.doc.bookmarks()` is released before the (immutable
        // again, but adjacent) viewport projection — keeps the
        // iteration ergonomic without needing a `.collect()` on the
        // whole bookmark list.
        for bm in self.doc.bookmarks() {
            let world = self.bookmark_world_position(bm);
            let (sx, sy) = self.viewport.world_to_screen(world.x, world.y);
            let broken = bm.stuck_to.is_some_and(|a| self.doc.find_stroke(a.stroke).is_none());
            draw_bookmark_pin(canvas, sx, sy, bm.color, broken);
        }
    }

    /// Compute the visible stroke set for the current viewport. Returns
    /// `None` when culling is not beneficial (very small doc or 100%
    /// zoom showing "most of it") — the caller then paints every
    /// stroke, matching pre-culling behaviour and skipping the
    /// per-frame hash-set build.
    fn visible_stroke_set(&self, surface: SurfaceBounds) -> Option<HashSet<StrokeId>> {
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
/// Long-press dwell before the gesture promotes to "create bookmark".
/// Matches the home-page long-press feel so touch users get consistent
/// timing across routes.
const PIN_LONG_PRESS: std::time::Duration = std::time::Duration::from_millis(550);
/// Movement slack (surface pixels) before a pending long-press
/// cancels. Finger / stylus holds emit sub-pixel jitter even when the
/// user intends a static hold; a strict "any move cancels" rule
/// would make the pin never land.
const PIN_LONG_PRESS_SLOP_PX: f32 = 12.0;
/// Tap radius used to decide whether an initial pointer_down lands
/// on an existing pin — matches `PIN_RADIUS_PX` plus a small comfort
/// margin so slightly imprecise taps still select the pin.
const PIN_TAP_RADIUS_PX: f32 = 20.0;
/// World-space sticky radius used when a new pin is dropped — the
/// `Board::add_bookmark` resolver snaps to a stroke within this
/// radius. 40 world units ≈ a thick stroke's half-width at default
/// zoom; keeps the sticky link forgiving but not grabby.
const PIN_STICKY_WORLD_RADIUS: f32 = 40.0;

pub fn drawing_surface(
    board: &Arc<Mutex<Board>>,
    selected_bookmark: freya::prelude::State<Option<crate::ids::BookmarkId>>,
    body_buffer: freya::prelude::State<String>,
) -> impl IntoElement {
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
    let pointer_move_board = Arc::clone(board);
    let pointer_leave_board = Arc::clone(board);
    // Shared long-press state: token invalidates when the pointer
    // moves far enough or leaves the surface; press_start records the
    // down-event location so the timer can create the pin at the
    // right world coord even if the pointer drifts a few pixels.
    let press_token = freya::prelude::use_state(|| 0u64);
    let press_start = freya::prelude::use_state(|| Option::<(f32, f32)>::None);
    let press_down_board = Arc::clone(board);

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
            // Middle only. Left is reserved: pen tablets (Wacom et al.)
            // deliver pen-tip contact as a left-button press through
            // libinput, so panning on left would hijack every drawing
            // stroke. Finger pan runs through `on_touch_*` instead.
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
        .on_global_pointer_up(move |_: Event<PointerEventData>| {
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
        .on_pointer_move({
            let mut press_token_move = press_token;
            let mut press_start_move = press_start;
            move |e: Event<PointerEventData>| {
                // Mouse / trackpad cursor feeds the overlay the same
                // way pen hover does. Pen contact overrides via
                // `pen_pump` (which locks the same board); whichever
                // source fires last wins — fine, since the stylus
                // never shares a surface pixel with the mouse pointer
                // in practice.
                let loc = e.element_location();
                #[allow(clippy::cast_possible_truncation)]
                let x = loc.x as f32;
                #[allow(clippy::cast_possible_truncation)]
                let y = loc.y as f32;
                lock(&pointer_move_board).set_pointer(Some((x, y)));
                // Invalidate the long-press timer if the pointer has
                // drifted past the slop — static hold is the whole
                // point, a dragging gesture should not create a pin.
                let start = *press_start_move.peek();
                if let Some((sx, sy)) = start {
                    let dx = x - sx;
                    let dy = y - sy;
                    if dx.hypot(dy) >= PIN_LONG_PRESS_SLOP_PX {
                        press_token_move.set(0);
                        press_start_move.set(None);
                    }
                }
            }
        })
        .on_pointer_leave(move |_: Event<PointerEventData>| {
            lock(&pointer_leave_board).set_pointer(None);
        })
        .on_pointer_down({
            let mut press_token = press_token;
            let mut press_start = press_start;
            let mut selected = selected_bookmark;
            let mut body_buffer = body_buffer;
            move |e: Event<PointerEventData>| {
                let loc = e.element_location();
                #[allow(clippy::cast_possible_truncation)]
                let x = loc.x as f32;
                #[allow(clippy::cast_possible_truncation)]
                let y = loc.y as f32;
                // Immediate hit test — a tap on an existing pin
                // selects it without waiting for the long-press
                // dwell so the card feels responsive.
                let hit = {
                    let guard = lock(&press_down_board);
                    guard.bookmark_near_screen(x, y, PIN_TAP_RADIUS_PX)
                };
                if let Some(id) = hit {
                    let body = lock(&press_down_board)
                        .doc()
                        .bookmark(id)
                        .map(|bm| bm.body.clone())
                        .unwrap_or_default();
                    body_buffer.set(body);
                    selected.set(Some(id));
                    // Short-circuit: don't schedule a long-press timer
                    // on top of an existing-pin tap.
                    let prev = *press_token.peek();
                    press_token.set(prev.wrapping_add(1));
                    press_start.set(None);
                    return;
                }
                let prev = *press_token.peek();
                let token = prev.wrapping_add(1);
                press_token.set(token);
                press_start.set(Some((x, y)));
                let timer_board = Arc::clone(&press_down_board);
                let mut timer_token = press_token;
                let mut timer_start = press_start;
                let mut timer_selected = selected;
                let mut timer_body = body_buffer;
                freya::prelude::spawn(async move {
                    async_io::Timer::after(PIN_LONG_PRESS).await;
                    if *timer_token.peek() != token {
                        return;
                    }
                    let Some((px, py)) = *timer_start.peek() else {
                        return;
                    };
                    let (world, now) = {
                        let guard = lock(&timer_board);
                        let (wx, wy) = guard.viewport().screen_to_world(px, py);
                        (crate::bookmark::WorldPoint::new(wx, wy), 0u64)
                    };
                    let _ = now;
                    let created_at = crate::bookmark_ui::now_ms();
                    let id = lock(&timer_board).add_bookmark(
                        world,
                        PIN_STICKY_WORLD_RADIUS,
                        created_at,
                    );
                    timer_body.set(String::new());
                    timer_selected.set(Some(id));
                    // Consume the pending press so a sibling pointer_up
                    // doesn't try to re-trigger anything.
                    timer_token.set(0);
                    timer_start.set(None);
                });
            }
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

/// Default pin tint when a bookmark carries no custom color.
/// Warm amber reads as "attention mark" on both light and dark
/// canvases without clashing with the usual stroke palette.
const PIN_DEFAULT_COLOR: [u8; 4] = [245, 158, 11, 255];
/// Pin outer radius in surface pixels. Matches the SOURCES_PLAN §7
/// spec (16 px disc). Independent of zoom so the tap target stays
/// constant.
const PIN_RADIUS_PX: f32 = 8.0;

/// Draw one bookmark pin at the given surface-pixel position. White
/// halo keeps the disc legible on busy backgrounds; a dashed ring is
/// overlaid when `broken` is true to signal a stale sticky anchor.
fn draw_bookmark_pin(
    canvas: &freya_engine::prelude::Canvas,
    cx: f32,
    cy: f32,
    color: Option<[u8; 4]>,
    broken: bool,
) {
    let rgba = color.unwrap_or(PIN_DEFAULT_COLOR);
    let fill_color = SkColor::from_argb(rgba[3], rgba[0], rgba[1], rgba[2]);

    // White halo under the pin for contrast against dark strokes.
    let mut halo = Paint::default();
    halo.set_color(SkColor::from_argb(220, 255, 255, 255));
    halo.set_style(PaintStyle::Fill);
    halo.set_anti_alias(true);
    canvas.draw_circle((cx, cy), PIN_RADIUS_PX + 2.0, &halo);

    // Tinted fill.
    let mut fill = Paint::default();
    fill.set_color(fill_color);
    fill.set_style(PaintStyle::Fill);
    fill.set_anti_alias(true);
    canvas.draw_circle((cx, cy), PIN_RADIUS_PX, &fill);

    // Thin dark border — reads cleanly on light pin colors too.
    let mut border = Paint::default();
    border.set_color(SkColor::from_argb(180, 30, 30, 30));
    border.set_style(PaintStyle::Stroke);
    border.set_stroke_width(1.5);
    border.set_anti_alias(true);
    canvas.draw_circle((cx, cy), PIN_RADIUS_PX, &border);

    if broken {
        // Dashed red outer ring = sticky-anchor target disappeared.
        // Matches the "⚠ ancla rota" chip language in SOURCES_PLAN §7.
        let mut warn = Paint::default();
        warn.set_color(SkColor::from_argb(255, 220, 38, 38));
        warn.set_style(PaintStyle::Stroke);
        warn.set_stroke_width(2.0);
        warn.set_anti_alias(true);
        canvas.draw_circle((cx, cy), PIN_RADIUS_PX + 4.0, &warn);
    }
}

/// Pointer indicator (hover disc). Projects a brush's world-unit
/// radius through the current viewport scale so the on-screen circle
/// matches the stroke width the user would commit. Halo + inner ring
/// keep the outline legible on both light and dark backgrounds.
fn draw_pointer_overlay(
    canvas: &freya_engine::prelude::Canvas,
    cx: f32,
    cy: f32,
    scale: f32,
    style: PointerStyle,
) {
    const MIN_RADIUS: f32 = 2.0;
    const MAX_RADIUS: f32 = 400.0;
    let (radius_world, color, dashed) = match style {
        PointerStyle::Hidden => return,
        PointerStyle::Outline { radius, color } => (radius, color, false),
        PointerStyle::Dashed { radius, color } => (radius, color, true),
    };
    let radius = (radius_world * scale).clamp(MIN_RADIUS, MAX_RADIUS);
    // White halo first, slightly thicker so it reads as a soft outline
    // when the overlay sits on a dark colour or busy background.
    let mut halo = Paint::default();
    halo.set_color(SkColor::from_argb(140, 255, 255, 255));
    halo.set_style(PaintStyle::Stroke);
    halo.set_stroke_width(2.5);
    halo.set_anti_alias(true);
    if dashed {
        canvas.draw_path(&pointer_dash_path(cx, cy, radius), &halo);
    } else {
        canvas.draw_circle((cx, cy), radius, &halo);
    }
    let mut ring = Paint::default();
    ring.set_color(SkColor::from_argb(color[3], color[0], color[1], color[2]));
    ring.set_style(PaintStyle::Stroke);
    ring.set_stroke_width(1.2);
    ring.set_anti_alias(true);
    if dashed {
        canvas.draw_path(&pointer_dash_path(cx, cy, radius), &ring);
    } else {
        canvas.draw_circle((cx, cy), radius, &ring);
    }
}

/// Build a dashed-circle path by sampling 24 arc segments and keeping
/// every other one. Avoids depending on Skia's `PathEffect::dash`,
/// which freya-engine's prelude does not re-export.
fn pointer_dash_path(cx: f32, cy: f32, radius: f32) -> freya_engine::prelude::Path {
    const SEGMENTS: u32 = 24;
    const SUBSTEPS: u32 = 4;
    let mut b = PathBuilder::new();
    #[allow(clippy::cast_precision_loss)]
    let seg = std::f32::consts::TAU / SEGMENTS as f32;
    for i in 0..SEGMENTS {
        if i & 1 == 1 {
            continue;
        }
        #[allow(clippy::cast_precision_loss)]
        let a0 = seg * i as f32;
        for k in 0..=SUBSTEPS {
            #[allow(clippy::cast_precision_loss)]
            let t = k as f32 / SUBSTEPS as f32;
            let a = t.mul_add(seg, a0);
            let x = radius.mul_add(a.cos(), cx);
            let y = radius.mul_add(a.sin(), cy);
            if k == 0 {
                b.move_to((x, y));
            } else {
                b.line_to((x, y));
            }
        }
    }
    b.detach()
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
        // pushing a redundant undo entry.
        let mut b = Board::default();
        b.set_current_preset(BrushPreset::pen());
        b.begin(pt(0.0, 0.0));
        b.extend(pt(400.0, 0.0));
        b.end();
        let stroke_count = |d: &Doc| d.layers.iter().map(|l| l.strokes.len()).sum::<usize>();
        assert_eq!(stroke_count(b.doc()), 1);
        let undo_depth_after_stroke = b.undo_depth();
        assert_eq!(undo_depth_after_stroke, 1, "one stroke commit = one undo");

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
        assert_eq!(
            b.undo_depth(),
            undo_depth_after_stroke,
            "cancel must not push an undo entry"
        );
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
    fn pen_stroke_commits_as_single_undo_entry() {
        // One stroke gesture (register brush + many samples + commit)
        // must land as exactly one undo entry — the "transaction
        // grouping" promise of Phase 1.3.
        let mut b = Board::default();
        b.set_current_preset(BrushPreset::pen());
        b.begin(pt(0.0, 0.0));
        for i in 1..=10 {
            b.extend(pt(i as f32 * 10.0, 0.0));
        }
        b.end();
        let stroke_count = |d: &Doc| d.layers.iter().map(|l| l.strokes.len()).sum::<usize>();
        assert_eq!(stroke_count(b.doc()), 1);
        assert_eq!(b.undo_depth(), 1, "one gesture = one undo entry");

        assert!(b.undo());
        assert_eq!(stroke_count(b.doc()), 0);
        assert_eq!(b.undo_depth(), 0);
        assert_eq!(b.redo_depth(), 1);
    }

    #[test]
    fn redo_after_undo_restores_stroke() {
        let mut b = Board::default();
        b.set_current_preset(BrushPreset::pen());
        b.begin(pt(0.0, 0.0));
        b.extend(pt(50.0, 0.0));
        b.end();
        let stroke_count = |d: &Doc| d.layers.iter().map(|l| l.strokes.len()).sum::<usize>();
        assert_eq!(stroke_count(b.doc()), 1);

        assert!(b.undo());
        assert_eq!(stroke_count(b.doc()), 0);

        assert!(b.redo());
        assert_eq!(stroke_count(b.doc()), 1);
        assert_eq!(b.undo_depth(), 1);
        assert_eq!(b.redo_depth(), 0);
    }

    #[test]
    fn new_action_after_undo_clears_redo_stack() {
        let mut b = Board::default();
        b.set_current_preset(BrushPreset::pen());
        b.begin(pt(0.0, 0.0));
        b.extend(pt(50.0, 0.0));
        b.end();
        assert!(b.undo());
        assert_eq!(b.redo_depth(), 1);

        // A fresh action invalidates the redo branch — standard
        // editor semantics, no "y-branch" of history.
        b.begin(pt(10.0, 10.0));
        b.extend(pt(60.0, 10.0));
        b.end();
        assert_eq!(b.redo_depth(), 0);
        assert_eq!(b.undo_depth(), 1);
    }

    #[test]
    fn marquee_erase_undoes_as_single_entry() {
        // Two strokes removed by a selection-rect erase must restore
        // in one undo — SOURCES_PLAN §1.3 transaction grouping.
        let mut b = Board::default();
        b.set_current_preset(BrushPreset::pen());
        b.begin(pt(0.0, 0.0));
        b.extend(pt(100.0, 0.0));
        b.end();
        b.begin(pt(0.0, 50.0));
        b.extend(pt(100.0, 50.0));
        b.end();
        let stroke_count = |d: &Doc| d.layers.iter().map(|l| l.strokes.len()).sum::<usize>();
        assert_eq!(stroke_count(b.doc()), 2);
        let undo_depth = b.undo_depth();

        // Simulate selection-rect erase via the end() path:
        // b.set_current_preset(BrushPreset::eraser()); drives through
        // eraser mode but SelectionRect mode needs explicit config.
        // Easier: call erase_strokes_in_rect directly to isolate the
        // transaction behaviour from eraser-mode plumbing.
        b.erase_strokes_in_rect(-10.0, -10.0, 200.0, 100.0);
        assert_eq!(stroke_count(b.doc()), 0);
        assert_eq!(b.undo_depth(), undo_depth + 1, "one erase = one entry");

        assert!(b.undo());
        assert_eq!(stroke_count(b.doc()), 2);
    }

    #[test]
    fn layer_opacity_undo_restores_prev_value() {
        let mut b = Board::default();
        assert!((b.doc().layer(0).unwrap().opacity - 1.0).abs() < f32::EPSILON);
        b.set_layer_opacity(0, 0.25);
        assert!((b.doc().layer(0).unwrap().opacity - 0.25).abs() < f32::EPSILON);
        assert!(b.undo());
        assert!((b.doc().layer(0).unwrap().opacity - 1.0).abs() < f32::EPSILON);
        assert!(b.redo());
        assert!((b.doc().layer(0).unwrap().opacity - 0.25).abs() < f32::EPSILON);
    }

    #[test]
    fn add_layer_and_set_active_undo_as_one_entry() {
        let mut b = Board::default();
        let before_active = b.active_layer_id();
        let before_depth = b.undo_depth();
        let new_id = b.add_layer();
        assert_eq!(b.active_layer_id(), new_id);
        assert_eq!(
            b.undo_depth(),
            before_depth + 1,
            "AddLayer + SetActiveLayer group into one undo entry"
        );
        assert!(b.undo());
        assert!(b.doc().layer(new_id).is_none(), "layer removed on undo");
        assert_eq!(
            b.active_layer_id(),
            before_active,
            "active layer restored"
        );
    }

    #[test]
    fn bookmark_create_sticks_to_nearby_stroke() {
        let mut b = Board::default();
        b.set_current_preset(BrushPreset::pen());
        b.begin(pt(0.0, 0.0));
        b.extend(pt(100.0, 0.0));
        b.end();
        let stroke_id = b
            .doc()
            .strokes()
            .next()
            .map(|s| s.id)
            .expect("stroke committed");

        let bm_id = b.add_bookmark(WorldPoint::new(5.0, 2.0), 50.0, 1000);
        let bm = b.doc().bookmark(bm_id).unwrap();
        let anchor = bm.stuck_to.expect("pin should stick to nearby stroke");
        assert_eq!(anchor.stroke, stroke_id);
    }

    #[test]
    fn bookmark_create_with_no_stroke_in_range_is_free() {
        let mut b = Board::default();
        let bm_id = b.add_bookmark(WorldPoint::new(0.0, 0.0), 10.0, 1000);
        assert!(b.doc().bookmark(bm_id).unwrap().stuck_to.is_none());
    }

    #[test]
    fn bookmark_add_update_delete_round_through_undo_redo() {
        let mut b = Board::default();
        let bm_id = b.add_bookmark(WorldPoint::new(0.0, 0.0), 0.0, 1_000);
        assert_eq!(b.bookmarks().len(), 1);
        assert_eq!(b.undo_depth(), 1);

        b.update_bookmark(
            bm_id,
            "nota".into(),
            vec![],
            Some([10, 20, 30, 255]),
            2_000,
        );
        assert_eq!(b.doc().bookmark(bm_id).unwrap().body, "nota");
        assert_eq!(b.undo_depth(), 2);

        assert!(b.undo(), "undo update");
        assert_eq!(b.doc().bookmark(bm_id).unwrap().body, "");
        assert!(b.redo(), "redo update");
        assert_eq!(b.doc().bookmark(bm_id).unwrap().body, "nota");

        assert!(b.delete_bookmark(bm_id));
        assert!(b.doc().bookmark(bm_id).is_none());
        assert!(b.undo(), "undo delete");
        assert!(b.doc().bookmark(bm_id).is_some());
    }

    #[test]
    fn stick_bookmark_undo_restores_previous_anchor() {
        let mut b = Board::default();
        let bm_id = b.add_bookmark(WorldPoint::new(0.0, 0.0), 0.0, 0);
        let anchor = StrokeAnchor {
            stroke: StrokeId::new_v4(),
            offset_in_bbox: (1.0, 2.0),
        };
        assert!(b.stick_bookmark(bm_id, Some(anchor)));
        assert_eq!(b.doc().bookmark(bm_id).unwrap().stuck_to, Some(anchor));
        assert!(b.undo());
        assert!(b.doc().bookmark(bm_id).unwrap().stuck_to.is_none());
    }

    #[test]
    fn clear_undo_restores_layers_and_strokes() {
        let mut b = Board::default();
        b.set_current_preset(BrushPreset::pen());
        b.begin(pt(0.0, 0.0));
        b.extend(pt(50.0, 0.0));
        b.end();
        let l2 = b.add_layer();
        b.begin(pt(0.0, 10.0));
        b.extend(pt(50.0, 10.0));
        b.end();
        let stroke_count = |d: &Doc| d.layers.iter().map(|l| l.strokes.len()).sum::<usize>();
        assert_eq!(stroke_count(b.doc()), 2);
        assert_eq!(b.doc().layers.len(), 2);

        b.clear();
        assert_eq!(stroke_count(b.doc()), 0);
        assert_eq!(b.doc().layers.len(), 1);

        assert!(b.undo());
        assert_eq!(stroke_count(b.doc()), 2);
        assert_eq!(b.doc().layers.len(), 2);
        assert!(b.doc().layer(l2).is_some(), "layer id preserved on undo");
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
