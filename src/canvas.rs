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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use flume::{Receiver, Sender};
use freya::prelude::*;
use freya_canvas_bg::{
    AttributionMode, BgPaintCtx, CanvasBackground, PageAttachment, Rect as BgRect, RedrawHandle,
    SolidColorBackground, clamp_translation,
};
use freya_engine::prelude::{
    BlendMode, Color as SkColor, Paint, PaintStyle, Path, PathBuilder, SaveLayerRec,
};
use notein_i18n::tr;

use crate::bookmark::{Bookmark, Rgba, SourceRef, StrokeAnchor, TimestampMs, WorldPoint};
use crate::brush::{
    BrushConfig, BrushId, BrushKind, BrushPreset, CapStyle, EraserMode, InkPoint, ShapeMode, Stroke,
};
use crate::doc::{Doc, DocError};
use crate::doc_op::DocOp;
use crate::ids::{BookmarkId, StrokeId};
use crate::ink_glyph::InkGlyphId;
use crate::render::{BrushRegistry, HighlighterBrush, HighlighterState, PointerStyle};
use crate::spatial::SpatialIndex;
use crate::sync::live::LiveMessage;
use crate::text_box::TextBoxId;
use crate::undo::UndoStack;

mod handwriting;
mod ink_cache;
mod lasso;
mod tools;
mod touch;

pub use lasso::Selection;
mod text;

pub use handwriting::LETTER_IDLE;
pub use ink_cache::InkCache;

pub use text::{SceneText, TextLayerView, TextScene};

pub const SIZE_SCALE_MIN: f32 = 0.25;
pub const SIZE_SCALE_MAX: f32 = 4.0;

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
#[derive(Clone, Debug, PartialEq)]
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
    /// Filled outline per [`crate::ink_glyph::InkGlyph`], rebuilt on add / load.
    glyph_paths: HashMap<InkGlyphId, Path>,
    /// Handwriting smoothing: pending letter, font, idle timer channel.
    handwriting: handwriting::Handwriting,
    lasso: lasso::LassoState,
    touch: touch::TouchState,
    previous_preset: Option<BrushPreset>,
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
    /// Text box under edit. See [`text`].
    text_session: Option<text::TextSession>,
    /// Laid-out height of every text box the UI has measured, in world
    /// units.
    text_heights: HashMap<TextBoxId, f32>,
    /// Press that may become a tap while in [`InputMode::Text`].
    tap: Option<text::TapTrack>,
    /// Mode [`Board::toggle_text_mode`] returns to.
    mode_before_text: InputMode,
    /// Bumped whenever [`Board::text_scene`] may change.
    text_rev: u64,
    /// Wakeups for UI watchers (see [`Board::watch`]), pinged with every
    /// redraw request.
    watchers: Vec<Sender<()>>,
    /// Where ephemeral state of this document goes while it is being
    /// drawn; see [`Board::set_live_sink`].
    live_sink: Option<LiveSink>,
    /// Points of the active stroke already handed to the live sink.
    live_sent: usize,
    live_last: Option<Instant>,
    pointer_last: Option<Instant>,
    /// What other people are doing on this document right now.
    remote: HashMap<notein_sync::DeviceId, RemoteHand>,
    /// Bumped whenever the committed ink may paint differently.
    ink_rev: u64,
    /// Bumped whenever the background asks for a repaint.
    paper_rev: Arc<AtomicU64>,
    /// Edits are refused: the document is shared with us to look at.
    read_only: bool,
}

/// Where live messages go; a closure, so the board never names the
/// sync service.
#[derive(Clone)]
struct LiveSink(Arc<dyn Fn(LiveMessage) + Send + Sync>);

impl std::fmt::Debug for LiveSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LiveSink")
    }
}

/// Another person's pen on this document: the stroke under it and
/// where it hovers.
#[derive(Debug, Clone)]
pub struct RemoteHand {
    pub name: String,
    stroke: Option<RemoteStroke>,
    pointer: Option<(f32, f32)>,
    seen: Instant,
}

#[derive(Debug, Clone)]
struct RemoteStroke {
    stroke: Stroke,
    preset: BrushPreset,
    opacity: f32,
    /// Set once the author lifted the pen; the ghost stays until the
    /// committed stroke lands or a moment passes.
    done_at: Option<Instant>,
}

/// How often ink under the pen travels to other devices.
const LIVE_INK_INTERVAL: Duration = Duration::from_millis(33);
/// How often a hovering pointer travels.
const LIVE_POINTER_INTERVAL: Duration = Duration::from_millis(80);
/// A remote hand silent this long is gone.
const REMOTE_TTL: Duration = Duration::from_secs(6);
/// A finished ghost stroke whose commit never arrived is dropped after this.
const REMOTE_DONE_TTL: Duration = Duration::from_secs(3);

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
    /// Pen samples are ignored by the pump; a sibling selection overlay
    /// (see [`crate::pdf_select_overlay`]) captures pointer events and
    /// drives the active source plugin's `pointer_*` API instead. Only
    /// meaningful on PDF-backed mounts for now; no-op otherwise.
    Select,
    /// Taps (pen, finger or mouse) create text boxes or open existing
    /// ones for editing; a finger drag still pans.
    Text,
    /// The pen circles ink to select it; dragging the selection moves it.
    Lasso,
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
        let paper_rev = Arc::new(AtomicU64::new(0));
        let paper = Arc::clone(&paper_rev);
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
            glyph_paths: HashMap::new(),
            handwriting: handwriting::Handwriting::default(),
            lasso: lasso::LassoState::default(),
            touch: touch::TouchState::default(),
            previous_preset: None,
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
            redraw_handle: RedrawHandle::new(move || {
                paper.fetch_add(1, Ordering::Relaxed);
            }),
            last_surface_size: (0.0, 0.0),
            text_session: None,
            text_heights: HashMap::new(),
            tap: None,
            mode_before_text: InputMode::Draw,
            text_rev: 0,
            watchers: Vec::new(),
            live_sink: None,
            live_sent: 0,
            live_last: None,
            pointer_last: None,
            remote: HashMap::new(),
            read_only: false,
            ink_rev: 0,
            paper_rev,
        }
    }
}

impl Board {
    /// Install the wakeup handle used to request repaints after each
    /// mutation. Called once at startup by the root component. Safe to
    /// re-install (replaces the previous notifier). Also rebuilds
    /// [`Self::redraw_handle`] so async background pipelines route
    /// their wakeups through the same channel.
    pub fn set_notifier(&mut self, notifier: RedrawNotifier) {
        let mirror = notifier.clone();
        let paper = Arc::clone(&self.paper_rev);
        self.redraw_handle = RedrawHandle::new(move || {
            paper.fetch_add(1, Ordering::Relaxed);
            mirror.ping();
        });
        self.notifier = Some(notifier);
    }

    /// Swap the background layer painted underneath every stroke.
    /// Triggers a redraw so the new backdrop / pages appear on the
    /// next frame.
    pub fn set_background(&mut self, background: Arc<dyn CanvasBackground>) {
        self.background = background;
        self.ink_rev += 1;
        self.notify();
    }

    /// Currently installed background. Callers (e.g. the freya
    /// [`drawing_surface`]) read this to source the viewport backdrop
    /// color and page metadata.
    #[must_use]
    pub fn background(&self) -> &Arc<dyn CanvasBackground> {
        &self.background
    }

    fn notify(&mut self) {
        if let Some(n) = &self.notifier {
            n.ping();
        }
        self.watchers
            .retain(|tx| !matches!(tx.try_send(()), Err(flume::TrySendError::Disconnected(()))));
    }

    /// Wake up whenever the board asks for a redraw. Bursts collapse into
    /// one wakeup; dropping the receiver unsubscribes.
    pub fn watch(&mut self) -> Receiver<()> {
        let (tx, rx) = flume::bounded(1);
        self.watchers.push(tx);
        rx
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
        self.emit_live_pointer();
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

    fn notify_commit(&mut self) {
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
        if self.input_mode == InputMode::Text {
            self.end_text_session();
        }
        if self.input_mode == InputMode::Lasso {
            self.lasso_cancel();
            self.clear_selection();
        }
        self.input_mode = mode;
        self.cancel();
        self.pan_end();
        self.text_press_cancel();
        self.touch_text_scene();
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
        self.ink_rev += 1;
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
        self.ink_rev += 1;
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
        stop_following();
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
        stop_following();
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
        self.remote.clear();
        self.text_session = None;
        self.text_heights.clear();
        self.touch_text_scene();
        self.undo.clear();
        self.forget_handwriting("document replaced");
        self.rebuild_derived();
    }

    /// Merge changes made on another replica of the same document (a
    /// sibling pane, another device) and refresh every cache derived
    /// from the strokes. Undo history and the viewport stay.
    ///
    /// # Errors
    /// Corrupt update bytes.
    pub fn merge_updates(&mut self, updates: &[u8]) -> Result<(), DocError> {
        self.doc.import_updates(updates)?;
        self.settle_remote();
        self.touch_text_scene();
        self.rebuild_derived();
        Ok(())
    }

    /// Rebuild the spatial index, cached paths, glyph paths and stroke
    /// positions from the current doc, then ask for a repaint.
    fn rebuild_derived(&mut self) {
        self.ink_rev += 1;
        self.spatial.clear();
        self.glyph_paths.clear();
        let glyphs: Vec<crate::ink_glyph::InkGlyph> = self
            .doc
            .layers
            .iter()
            .flat_map(|layer| layer.glyphs.iter().copied())
            .collect();
        for glyph in &glyphs {
            self.cache_glyph(glyph);
        }
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
        if self.read_only {
            return;
        }
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
        let Some(brush) = self.doc.register_brush(self.current_preset) else {
            return;
        };
        self.active_layer_at_begin = Some(target);
        let id = self.doc.allocate_stroke_id();
        self.active = Some(Stroke::new(id, brush, self.current_color, point));
        // Latch straight-mode at gesture start so an in-flight tip
        // toggle does not switch a live stroke between rubber-band and
        // freehand halfway through.
        self.two_point_active = self.is_two_point_stroke_mode();
        self.live_sent = 0;
        self.live_last = None;
        self.emit_live_ink(false);
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
        self.emit_live_ink(false);
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
        // Mask gate is enforced upstream by `pen_pump` against
        // window-coord samples. Repeating it here with canvas-local
        // coords would silently false-negative (overlay rects live in
        // window space) and false-positive when a padded canvas's
        // local (0, 0) happens to fall inside a sibling overlay's
        // window rect.
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
            self.emit_live_ink_for(&active, true);
            let (id, kind) = (active.id, self.current_kind());
            self.commit_stroke(active);
            self.two_point_active = false;
            self.handwriting_after_stroke(id, kind);
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
        if let Some(active) = self.active.take() {
            self.emit_live_ink_for(&active, true);
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
        if self.read_only {
            return false;
        }
        self.end_text_session();
        self.forget_handwriting("undo");
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
        if self.read_only {
            return false;
        }
        self.end_text_session();
        self.forget_handwriting("redo");
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
            name: tr!("canvas-layer-name", "number" => n),
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
        self.add_bookmark_with_refs(world, sticky_radius, timestamp_ms, Vec::new())
    }

    /// Variant of [`Self::add_bookmark`] that attaches initial source
    /// refs to the new pin. Used by the long-press handler to carry a
    /// backend anchor (PDF page, etc.) alongside the world position so
    /// chip strips can navigate back to the source material.
    pub fn add_bookmark_with_refs(
        &mut self,
        world: WorldPoint,
        sticky_radius: f32,
        timestamp_ms: TimestampMs,
        refs: Vec<SourceRef>,
    ) -> BookmarkId {
        self.add_bookmark_full(world, sticky_radius, timestamp_ms, String::new(), refs)
    }

    /// Full-form create: body + refs land in a single op so the "create
    /// bookmark from a source selection" flow doesn't need a follow-up
    /// `UpdateBookmark` just to seed the snippet into the body.
    pub fn add_bookmark_full(
        &mut self,
        world: WorldPoint,
        sticky_radius: f32,
        timestamp_ms: TimestampMs,
        body: String,
        refs: Vec<SourceRef>,
    ) -> BookmarkId {
        let id = BookmarkId::new_v4();
        let mut bm = Bookmark::new(id, world, timestamp_ms);
        bm.stuck_to = self.resolve_sticky(world, sticky_radius);
        bm.body = body;
        bm.refs = refs;
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

    /// Center the viewport on `id`'s live world position. Pure
    /// translation — scale is untouched so the user keeps their zoom
    /// level. Returns `true` iff `id` resolves and the surface size
    /// is known (post first-frame); otherwise the viewport is left
    /// alone and the caller can retry on the next paint.
    pub fn center_on_bookmark(&mut self, id: BookmarkId) -> bool {
        self.view_on_bookmark(id).is_some_and(|view| {
            self.set_viewport(view);
            true
        })
    }

    /// The view [`Self::center_on_bookmark`] moves to, without moving.
    #[must_use]
    pub fn view_on_bookmark(&self, id: BookmarkId) -> Option<Viewport> {
        let bm = self.doc.bookmark(id)?;
        let world = self.bookmark_world_position(bm);
        self.view_centered_on(world.x, world.y)
    }

    /// Center the viewport on world-space point `(wx, wy)`. Returns
    /// `false` when the surface size is still unknown (pre first-frame).
    pub fn center_on_world(&mut self, wx: f32, wy: f32) -> bool {
        self.view_centered_on(wx, wy).is_some_and(|view| {
            self.set_viewport(view);
            true
        })
    }

    /// The world point in the middle of the surface; `None` while its
    /// size is unknown.
    #[must_use]
    pub fn world_center(&self) -> Option<(f32, f32)> {
        let (sw, sh) = self.last_surface_size;
        (sw > 0.0 && sh > 0.0).then(|| self.viewport.screen_to_world(sw * 0.5, sh * 0.5))
    }

    /// The view with world point `(wx, wy)` in the middle at `scale`;
    /// `None` while the surface size is unknown.
    #[must_use]
    pub fn view_at(&self, wx: f32, wy: f32, scale: f32) -> Option<Viewport> {
        let (sw, sh) = self.last_surface_size;
        if !wx.is_finite() || !wy.is_finite() || !scale.is_finite() || sw <= 0.0 || sh <= 0.0 {
            return None;
        }
        let scale = scale.clamp(VIEWPORT_SCALE_MIN, VIEWPORT_SCALE_MAX);
        Some(Viewport {
            tx: sw.mul_add(0.5, -(wx * scale)),
            ty: sh.mul_add(0.5, -(wy * scale)),
            scale,
        })
    }

    /// The view with world point `(wx, wy)` in the middle at the current
    /// zoom; `None` while the surface size is unknown.
    #[must_use]
    pub fn view_centered_on(&self, wx: f32, wy: f32) -> Option<Viewport> {
        let (sw, sh) = self.last_surface_size;
        if !wx.is_finite() || !wy.is_finite() || sw <= 0.0 || sh <= 0.0 {
            return None;
        }
        let scale = self.viewport.scale;
        Some(Viewport {
            tx: sw.mul_add(0.5, -(wx * scale)),
            ty: sh.mul_add(0.5, -(wy * scale)),
            scale,
        })
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
        if self.read_only {
            return;
        }
        let inverse = self.doc.inverse_before_apply(&op);
        self.apply_op_untracked(op.clone());
        self.undo.record(op, inverse);
    }

    /// Apply a `DocOp` through `Doc::emit` and reconcile the derived
    /// state (spatial index, cached paths, `stroke_index`). No undo
    /// recording — used by both the forward path (indirectly via
    /// [`Self::apply_op`]) and the undo / redo replays.
    fn apply_op_untracked(&mut self, op: DocOp) {
        self.ink_rev += 1;
        self.touch_text_scene();
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
                self.glyph_paths
                    .retain(|id, _| self.doc.locate_glyph(*id).is_some());
            }
            DocOp::Clear => {
                self.doc.emit(DocOp::Clear);
                self.spatial.clear();
                self.cached_paths.clear();
                self.glyph_paths.clear();
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
                        .unwrap_or_else(|| layer.strokes.len().saturating_sub(1));
                    self.stroke_index.insert(id, (layer_id, idx));
                }
            }
            DocOp::AddGlyph { layer_id, glyph } => {
                self.doc.emit(DocOp::AddGlyph { layer_id, glyph });
                self.cache_glyph(&glyph);
            }
            DocOp::DeleteGlyph { id } => {
                self.doc.emit(DocOp::DeleteGlyph { id });
                self.glyph_paths.remove(&id);
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
            | DocOp::StickBookmark { .. }
            | DocOp::AddText { .. }
            | DocOp::UpdateText { .. }
            | DocOp::SetTextFrame { .. }
            | DocOp::DeleteText { .. }) => {
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
            let Some(session) = self.erase_session.as_ref() else {
                return;
            };
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

        let Some(session) = self.erase_session.as_mut() else {
            return;
        };
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
        let mut texts: Vec<TextBoxId> = Vec::new();
        for &(cx, cy, r) in &session.circles {
            for id in self.texts_touching_circle(cx, cy, r) {
                if !texts.contains(&id) {
                    texts.push(id);
                }
            }
        }
        let mut glyphs: Vec<InkGlyphId> = Vec::new();
        for &(cx, cy, r) in &session.circles {
            for id in self.glyphs_touching_circle(cx, cy, r) {
                if !glyphs.contains(&id) {
                    glyphs.push(id);
                }
            }
        }
        self.erase_glyphs(&glyphs);
        self.erase_texts(&texts);
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
            if let Some(&text) = self.texts_touching_circle(x, y, radius).last() {
                self.erase_texts(&[text]);
            } else if let Some(&glyph) = self.glyphs_touching_circle(x, y, radius).last() {
                self.erase_glyphs(&[glyph]);
            }
            return;
        };
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
        let texts = self.texts_in_rect(min_x, min_y, max_x, max_y);
        let glyphs = self.glyphs_in_rect(min_x, min_y, max_x, max_y);
        if targets.is_empty() && texts.is_empty() && glyphs.is_empty() {
            return;
        }
        self.undo.begin();
        for id in targets {
            self.apply_op(DocOp::RemoveStroke { id });
        }
        self.erase_texts(&texts);
        self.erase_glyphs(&glyphs);
        self.undo.commit();
    }

    /// Paint the slice of the document `pass` names onto `canvas`.
    ///
    /// The committed ink (and the paper under it, for the bottom pass)
    /// comes from `cache` whenever it can; see [`InkCache`]. The active
    /// stroke and the overlays are drawn fresh every frame on top.
    #[sismo::function]
    pub fn paint(
        &mut self,
        canvas: &freya_engine::prelude::Canvas,
        surface: SurfaceBounds,
        pass: &PaintPass,
        cache: &mut InkCache,
    ) {
        if pass.base {
            self.settle_surface(surface);
        }
        if !self.paint_cached(canvas, surface, pass, cache) {
            if pass.base {
                self.paint_base(canvas, surface);
            }
            canvas.save();
            canvas.translate((self.viewport.tx, self.viewport.ty));
            canvas.scale((self.viewport.scale, self.viewport.scale));
            self.paint_layers(canvas, surface, pass.layers.clone());
            canvas.restore();
        }
        if pass.base {
            self.tick_background(surface);
        }
        if pass.overlays {
            canvas.save();
            canvas.translate((self.viewport.tx, self.viewport.ty));
            canvas.scale((self.viewport.scale, self.viewport.scale));
            self.paint_world_overlays(canvas);
            canvas.restore();
            self.paint_screen_overlays(canvas);
        }
    }

    /// Paint the base and committed ink of `pass` from `cache`, drawing
    /// the cache again when it no longer fits. `false` when the pass has
    /// to be painted directly: the ink is moving under an eraser or a
    /// lasso, it blends with what lies under the pass, or the canvas
    /// cannot hold an image.
    fn paint_cached(
        &self,
        canvas: &freya_engine::prelude::Canvas,
        surface: SurfaceBounds,
        pass: &PaintPass,
        cache: &mut InkCache,
    ) -> bool {
        let erasing = self
            .erase_session
            .as_ref()
            .is_some_and(|s| !s.circles.is_empty());
        if erasing || self.lasso_dragging() {
            return false;
        }
        if !pass.base && !self.has_ink(pass.layers.clone()) {
            return false;
        }
        if !pass.base && cache.blends(self.ink_rev, || self.ink_blends(pass.layers.clone())) {
            return false;
        }
        let size = (surface.max_x - surface.min_x, surface.max_y - surface.min_y);
        let Some(frame) = ink_cache::Frame::of(canvas, size, self.viewport) else {
            return false;
        };
        let content = (self.ink_rev, self.paper_rev.load(Ordering::Relaxed));
        if !cache.fits(&frame, content, self.gesture_active) {
            let drawn = cache.redraw(canvas, &frame, content, |target, bounds| {
                if pass.base {
                    self.paint_base(target, bounds);
                }
                target.save();
                target.translate((self.viewport.tx, self.viewport.ty));
                target.scale((self.viewport.scale, self.viewport.scale));
                self.paint_layers(target, bounds, pass.layers.clone());
                target.restore();
            });
            if !drawn {
                return false;
            }
        }
        cache.draw(canvas, &frame);
        true
    }

    /// Whether the layers at `range` hold anything to paint.
    fn has_ink(&self, range: std::ops::Range<usize>) -> bool {
        self.doc.layers.get(range).is_some_and(|layers| {
            layers
                .iter()
                .any(|l| l.visible && !(l.strokes.is_empty() && l.glyphs.is_empty()))
        })
    }

    /// Whether any visible ink of the layers at `range` blends with what
    /// lies under it instead of covering it (a highlighter multiplies).
    fn ink_blends(&self, range: std::ops::Range<usize>) -> bool {
        let Some(layers) = self.doc.layers.get(range) else {
            return false;
        };
        let mut seen = HashSet::new();
        layers
            .iter()
            .filter(|layer| layer.visible)
            .flat_map(|layer| &layer.strokes)
            .filter(|stroke| seen.insert(stroke.brush))
            .filter_map(|stroke| self.doc.preset(stroke.brush))
            .any(|preset| {
                let paint = self
                    .brush_registry
                    .brush(preset.kind)
                    .paint(preset, [0, 0, 0, 255]);
                paint.as_blend_mode() != Some(BlendMode::SrcOver)
            })
    }

    /// Note the surface size and pull the viewport back inside the
    /// background before anything is painted.
    fn settle_surface(&mut self, surface: SurfaceBounds) {
        // Cache surface size so off-paint mutators (`pan_move`,
        // `viewport_zoom_at`, `viewport_zoom_by`) can consult it when
        // clamping against the current background's content bounds.
        self.last_surface_size = (surface.max_x - surface.min_x, surface.max_y - surface.min_y);
        // Correct any drift accumulated since the last frame — the
        // background may have swapped, the surface may have resized, or
        // an off-path mutator may have pushed the viewport past bounds
        // before the surface size was known.
        self.clamp_viewport_to_background();
    }

    /// Let the background fetch what the view now shows.
    fn tick_background(&self, surface: SurfaceBounds) {
        self.background
            .tick(self.visible_world(surface), self.viewport.scale);
    }

    fn visible_world(&self, surface: SurfaceBounds) -> BgRect {
        let (min_x, min_y) = self.viewport.screen_to_world(surface.min_x, surface.min_y);
        let (max_x, max_y) = self.viewport.screen_to_world(surface.max_x, surface.max_y);
        BgRect {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    /// Backdrop and background (PDF pages, paper, …): everything below the
    /// first layer.
    fn paint_base(&self, canvas: &freya_engine::prelude::Canvas, surface: SurfaceBounds) {
        // Backdrop fill lives inside the paint callback so a runtime
        // `set_background` swap (edit sheet re-tint) actually repaints
        // the surface — the enclosing freya rect's `.background(...)`
        // attribute captures a stale color at element construction and
        // only refreshes on remount, which is unacceptable for an in-
        // place colour change on the active canvas.
        if let Some(bd) = self.background.viewport_backdrop() {
            let mut paint = freya_engine::prelude::Paint::default();
            paint.set_color(bd);
            paint.set_anti_alias(false);
            canvas.draw_rect(
                freya_engine::prelude::Rect::from_ltrb(
                    surface.min_x,
                    surface.min_y,
                    surface.max_x,
                    surface.max_y,
                ),
                &paint,
            );
        }

        // Background layer (PDF pages, blank paper, image, solid
        // color, etc.) paints first — under the strokes but inside the
        // world transform so page rectangles live in the same
        // coordinate space as the ink.
        canvas.save();
        canvas.translate((self.viewport.tx, self.viewport.ty));
        canvas.scale((self.viewport.scale, self.viewport.scale));
        let mut bg_ctx = BgPaintCtx {
            canvas,
            visible: self.visible_world(surface),
            scale: self.viewport.scale,
            redraw: self.redraw_handle.clone(),
        };
        self.background.paint(&mut bg_ctx);
        canvas.restore();
    }

    /// Committed strokes of the layers at `range` (paint order indices), in
    /// world coordinates.
    fn paint_layers(
        &self,
        canvas: &freya_engine::prelude::Canvas,
        surface: SurfaceBounds,
        range: std::ops::Range<usize>,
    ) {
        let Some(layers) = self.doc.layers.get(range) else {
            return;
        };
        if layers.is_empty() {
            return;
        }
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

        for layer in layers {
            if !layer.visible {
                continue;
            }
            let mask_this = mask_circles.is_some() && layer.id == active_layer;
            if mask_this {
                canvas.save_layer(&SaveLayerRec::default());
            }
            let opacity = layer.opacity.clamp(0.0, 1.0);
            for stroke in &layer.strokes {
                let dragged = self.drag_offset_of_stroke(stroke.id);
                if dragged.is_none()
                    && let Some(set) = visible.as_ref()
                    && !set.contains(&stroke.id)
                {
                    continue;
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
                if let Some((dx, dy)) = dragged {
                    canvas.save();
                    canvas.translate((dx, dy));
                    canvas.draw_path(cached, &paint);
                    canvas.restore();
                    continue;
                }
                canvas.draw_path(cached, &paint);
            }
            self.paint_glyphs(canvas, layer, opacity);
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
    }

    /// In-flight stroke and eraser marquee, in world coordinates.
    fn paint_world_overlays(&self, canvas: &freya_engine::prelude::Canvas) {
        for hand in self.remote.values() {
            let Some(remote) = &hand.stroke else {
                continue;
            };
            let renderer = self.brush_registry.brush(remote.preset.kind);
            let mut paint = renderer.paint(&remote.preset, remote.stroke.color);
            let base = paint.color();
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let scaled = (f32::from(base.a()) * remote.opacity.clamp(0.0, 1.0) * REMOTE_INK_ALPHA)
                .clamp(0.0, 255.0) as u8;
            paint.set_color(base.with_a(scaled));
            let path =
                renderer.build_path(&remote.preset, &remote.stroke, self.brush_registry.caps());
            canvas.draw_path(&path, &paint);
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
        self.paint_lasso(canvas);
    }

    /// Pointer disc and bookmark pins, in surface pixels.
    fn paint_screen_overlays(&self, canvas: &freya_engine::prelude::Canvas) {
        // Pointer overlay paints in surface-pixel space — the brush's
        // world-unit radius multiplied by the current zoom matches the
        // on-screen stroke width exactly. Suppressed during multi-touch
        // gestures (pinch is not a drawing intent) and during a middle-
        // drag pan (the mouse is anchoring the viewport, not hovering a
        // target).
        if let Some((px, py)) = self.pointer
            && !self.gesture_active
            && !self.is_panning()
            && self.input_mode != InputMode::Text
        {
            let preset = self.current_preset;
            let color = self.current_color;
            let style = self
                .brush_registry
                .brush(preset.kind)
                .pointer_style(&preset, color);
            draw_pointer_overlay(canvas, px, py, self.viewport.scale, style);
        }

        for (device, hand) in &self.remote {
            if let Some((wx, wy)) = hand.pointer {
                let (sx, sy) = self.viewport.world_to_screen(wx, wy);
                draw_remote_pointer(canvas, sx, sy, device_color(*device), &hand.name);
            }
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
            let broken = bm
                .stuck_to
                .is_some_and(|a| self.doc.find_stroke(a.stroke).is_none());
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

/// Slice of the paint order one canvas element draws. Splitting the paint
/// lets text boxes, which are UI elements, sit between the layers they
/// belong to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaintPass {
    /// Layer indices (paint order) whose strokes this pass draws.
    pub layers: std::ops::Range<usize>,
    /// Draw the backdrop and the background first.
    pub base: bool,
    /// Draw the in-flight stroke, marquee, pointer and pins last.
    pub overlays: bool,
}

impl PaintPass {
    /// Passes for `ranges`, the first one with the base and the last one
    /// with the overlays.
    #[must_use]
    pub fn split(ranges: Vec<std::ops::Range<usize>>) -> Vec<Self> {
        let last = ranges.len().saturating_sub(1);
        ranges
            .into_iter()
            .enumerate()
            .map(|(index, layers)| Self {
                layers,
                base: index == 0,
                overlays: index == last,
            })
            .collect()
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
/// Tap radius used to decide whether an initial `pointer_down` lands
/// on an existing pin — matches `PIN_RADIUS_PX` plus a small comfort
/// margin so slightly imprecise taps still select the pin.
pub(crate) const PIN_TAP_RADIUS_PX: f32 = 20.0;
/// World-space sticky radius used when a new pin is dropped — the
/// `Board::add_bookmark` resolver snaps to a stroke within this
/// radius. 40 world units ≈ a thick stroke's half-width at default
/// zoom; keeps the sticky link forgiving but not grabby.
const PIN_STICKY_WORLD_RADIUS: f32 = 40.0;

/// Builder for the per-page anchor ref emitted when a long-press pin
/// lands over an attribution-aware background.
///
/// Takes the surface-level page attachment reported by
/// [`CanvasBackground::locate`] and returns the typed [`SourceRef`] the pin
/// should carry.
pub type AnchorSourceRef = Arc<dyn Fn(PageAttachment) -> SourceRef + Send + Sync + 'static>;

/// A long press on the page, as `(world, window)` points: `true` when the
/// face handled it (a word got selected) and no bookmark should drop.
pub type TextPressRef = Arc<dyn Fn((f32, f32), (f32, f32)) -> bool + Send + Sync + 'static>;

/// Press on the canvas that may open or drop a bookmark.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PinPress {
    x: f32,
    y: f32,
    /// The same point in window coordinates.
    window: (f32, f32),
    at: std::time::Instant,
}

/// Long press that drops a bookmark, and tap that opens an existing one.
///
/// A press only counts while it stays put: moving past the slop, a second
/// finger, a pan or a cancel ends it. Opening an existing pin waits for the
/// lift, so a drag that happens to start on a pin pans the canvas instead
/// of opening its card.
#[derive(Clone, Copy)]
struct PinGesture {
    press: freya::prelude::State<Option<PinPress>>,
    selected: freya::prelude::State<Option<crate::ids::BookmarkId>>,
    body: freya::prelude::State<String>,
}

impl PinGesture {
    fn use_gesture(
        selected: freya::prelude::State<Option<crate::ids::BookmarkId>>,
        body: freya::prelude::State<String>,
    ) -> Self {
        Self {
            press: freya::prelude::use_state(|| None),
            selected,
            body,
        }
    }

    /// Start a press at `(x, y)` (surface pixels). The pin drops after
    /// [`PIN_LONG_PRESS`] unless the press ends first.
    fn down(
        mut self,
        board: &Arc<Mutex<Board>>,
        (x, y): (f32, f32),
        window: (f32, f32),
        anchor: Option<AnchorSourceRef>,
        text_press: Option<TextPressRef>,
    ) {
        let press = PinPress {
            x,
            y,
            window,
            at: std::time::Instant::now(),
        };
        self.press.set(Some(press));
        let board = Arc::clone(board);
        freya::prelude::spawn(async move {
            async_io::Timer::after(PIN_LONG_PRESS).await;
            // Copied out first: a guard alive across `set` would panic.
            let current = *self.press.peek();
            if current == Some(press) {
                self.press.set(None);
                self.hold(&board, press, anchor.as_ref(), text_press.as_ref());
            }
        });
    }

    /// The pointer moved to `(x, y)`: past the slop the press is a drag.
    fn moved(self, x: f32, y: f32) {
        let current = *self.press.peek();
        if let Some(press) = current
            && (x - press.x).hypot(y - press.y) >= PIN_LONG_PRESS_SLOP_PX
        {
            self.cancel();
        }
    }

    /// Forget the press: second finger, pan, cancelled touch.
    fn cancel(mut self) {
        if self.press.peek().is_some() {
            self.press.set(None);
        }
    }

    /// The pointer lifted. A still, short press on a pin opens it.
    fn lift(mut self, board: &Arc<Mutex<Board>>) {
        let Some(press) = *self.press.peek() else {
            return;
        };
        self.press.set(None);
        let hit = lock(board).bookmark_near_screen(press.x, press.y, PIN_TAP_RADIUS_PX);
        if let Some(id) = hit {
            self.open(board, id);
        }
    }

    fn open(mut self, board: &Arc<Mutex<Board>>, id: crate::ids::BookmarkId) {
        let body = lock(board)
            .doc()
            .bookmark(id)
            .map(|bm| bm.body.clone())
            .unwrap_or_default();
        self.body.set(body);
        self.selected.set(Some(id));
    }

    /// The press was held long enough: open the pin under it, or drop one.
    fn hold(
        mut self,
        board: &Arc<Mutex<Board>>,
        press: PinPress,
        anchor: Option<&AnchorSourceRef>,
        text_press: Option<&TextPressRef>,
    ) {
        let hit = lock(board).bookmark_near_screen(press.x, press.y, PIN_TAP_RADIUS_PX);
        if let Some(id) = hit {
            self.open(board, id);
            return;
        }
        if let Some(on_text) = text_press {
            let world = lock(board).viewport().screen_to_world(press.x, press.y);
            if on_text(world, press.window) {
                return;
            }
        }
        let (world, refs) = {
            let guard = lock(board);
            let (wx, wy) = guard.viewport().screen_to_world(press.x, press.y);
            let refs = match anchor {
                Some(builder)
                    if guard.background().attribution_mode() == AttributionMode::PerPage =>
                {
                    guard
                        .background()
                        .locate(wx, wy)
                        .map(|att| vec![builder(att)])
                        .unwrap_or_default()
                }
                _ => Vec::new(),
            };
            drop(guard);
            (crate::bookmark::WorldPoint::new(wx, wy), refs)
        };
        let created_at = crate::format::now();
        let id =
            lock(board).add_bookmark_with_refs(world, PIN_STICKY_WORLD_RADIUS, created_at, refs);
        self.body.set(String::new());
        self.selected.set(Some(id));
    }
}

/// A window or element point as `f32`, the space the board works in.
#[allow(clippy::cast_possible_truncation)] // window coordinates
const fn point(at: freya::prelude::CursorPoint) -> (f32, f32) {
    (at.x as f32, at.y as f32)
}

/// The input handlers of one drawing surface.
#[derive(Clone)]
struct Surface {
    board: Arc<Mutex<Board>>,
    /// Window coordinates of the surface's top-left, to compare touches
    /// with pen samples (which come in window space).
    origin: freya::prelude::State<(f32, f32)>,
    id: crate::pen_pump::CanvasSurfaceId,
    pin: PinGesture,
    anchor_source: Option<AnchorSourceRef>,
    text_press: Option<TextPressRef>,
}

impl Surface {
    /// The surface was laid out: tell the pen pump where it is.
    fn sized(&self, area: Area) {
        let rect = crate::pen_pump::SurfaceRect {
            min_x: area.min_x(),
            min_y: area.min_y(),
            max_x: area.max_x(),
            max_y: area.max_y(),
        };
        crate::pen_pump::register_canvas_surface(self.id, &self.board, rect);
        let mut origin = self.origin;
        origin.set_if_modified((rect.min_x, rect.min_y));
    }

    /// Scrolling up zooms in around the pointer.
    fn wheel(&self, e: &WheelEventData) {
        let (x, y) = point(e.element_location);
        #[allow(clippy::cast_possible_truncation)] // a wheel delta
        let factor = (-(e.delta_y as f32) * 0.0015).exp();
        lock(&self.board).viewport_zoom_at(x, y, factor);
    }

    /// Left presses only matter to text mode, and not when they echo the
    /// pen's own contact (libinput does so for tablets). Middle pans.
    fn mouse_down(&self, e: &MouseEventData) {
        if e.button == Some(MouseButton::Left) {
            let (gx, gy) = point(e.global_location);
            if !crate::pen_pump::is_pen_near(gx, gy, crate::pen_pump::PEN_TOUCH_FILTER_RADIUS) {
                let mut guard = lock(&self.board);
                if guard.input_mode() == InputMode::Text {
                    let (x, y) = point(e.element_location);
                    guard.text_press_down(x, y);
                }
            }
            return;
        }
        // Left is reserved: tablets deliver the pen tip as a left press,
        // so panning on it would hijack every stroke.
        if e.button == Some(MouseButton::Middle) {
            let (x, y) = point(e.global_location);
            lock(&self.board).pan_begin(x, y);
        }
    }

    /// Touches arrive as global pointer events too; they are handled by
    /// the touch handlers in element space, so skip them here.
    fn global_move(&self, e: &PointerEventData) {
        if matches!(e, PointerEventData::Touch(_)) {
            return;
        }
        let (x, y) = point(e.global_location());
        let mut guard = lock(&self.board);
        if guard.is_panning() {
            guard.pan_move(x, y);
        } else {
            let (ox, oy) = *self.origin.peek();
            guard.text_press_move(x - ox, y - oy);
        }
    }

    fn global_up(&self, e: &PointerEventData) {
        if matches!(e, PointerEventData::Touch(_)) {
            return;
        }
        self.pin.lift(&self.board);
        let (x, y) = point(e.global_location());
        let (ox, oy) = *self.origin.peek();
        let mut guard = lock(&self.board);
        guard.pan_end();
        guard.text_press_up(x - ox, y - oy);
    }

    /// A finger: ignored when it is the pen seen as touch, or the palm
    /// resting while the pen is in use.
    fn touch_start(&self, e: &TouchEventData) {
        let (x, y) = point(e.element_location);
        let (ox, oy) = *self.origin.peek();
        if crate::pen_pump::is_pen_near(x + ox, y + oy, crate::pen_pump::PEN_TOUCH_FILTER_RADIUS) {
            return;
        }
        let palm =
            crate::settings::SettingsStore::try_get().is_some_and(|s| s.peek().pen.palm_rejection);
        if palm && crate::pen_pump::pen_in_use() {
            return;
        }
        let mut guard = lock(&self.board);
        guard.touch_down(e.finger_id, x, y);
        if guard.is_gesture_active() {
            self.pin.cancel();
        }
    }

    fn touch_move(&self, e: &TouchEventData) {
        let (x, y) = point(e.element_location);
        // Finger drags do not reach `on_pointer_move`, so a pan ends the
        // press here.
        self.pin.moved(x, y);
        lock(&self.board).touch_move(e.finger_id, x, y);
    }

    fn touch_end(&self, e: &TouchEventData) {
        self.pin.lift(&self.board);
        lock(&self.board).touch_up(e.finger_id);
    }

    fn touch_cancel(&self, e: &TouchEventData) {
        self.pin.cancel();
        lock(&self.board).touch_up(e.finger_id);
    }

    /// The mouse cursor feeds the overlay like pen hover does.
    fn pointer_move(&self, e: &PointerEventData) {
        let (x, y) = point(e.element_location());
        lock(&self.board).set_pointer(Some((x, y)));
        self.pin.moved(x, y);
    }

    fn pointer_down(&self, e: &PointerEventData) {
        self.pin.down(
            &self.board,
            point(e.element_location()),
            point(e.global_location()),
            self.anchor_source.clone(),
            self.text_press.clone(),
        );
    }
}

pub fn drawing_surface(
    board: &Arc<Mutex<Board>>,
    selected_bookmark: freya::prelude::State<Option<crate::ids::BookmarkId>>,
    body_buffer: freya::prelude::State<String>,
    text_editor: crate::canvas_text::TextEditorSlot,
    anchor_source: Option<AnchorSourceRef>,
    text_press: Option<TextPressRef>,
) -> impl IntoElement {
    // Read once: a background swap remounts the surface.
    let backdrop = lock(board)
        .background()
        .viewport_backdrop()
        .unwrap_or_else(|| SkColor::from_rgb(250, 250, 248));
    let backdrop = Color::from_argb(backdrop.a(), backdrop.r(), backdrop.g(), backdrop.b());
    // Keyed per mount, so two surfaces of one board (a document open in
    // two panes) each keep their pen-pump entry.
    let id = freya::prelude::use_hook(crate::pen_pump::allocate_canvas_surface_id);
    freya::prelude::use_drop(move || crate::pen_pump::unregister_canvas_surface(id));
    let surface = Surface {
        board: Arc::clone(board),
        origin: freya::prelude::use_state(|| (0.0_f32, 0.0_f32)),
        id,
        pin: PinGesture::use_gesture(selected_bookmark, body_buffer),
        anchor_source,
        text_press,
    };
    let on = |handler: fn(&Surface, &PointerEventData)| {
        let surface = surface.clone();
        move |e: Event<PointerEventData>| handler(&surface, &e)
    };
    let touch = |handler: fn(&Surface, &TouchEventData)| {
        let surface = surface.clone();
        move |e: Event<TouchEventData>| handler(&surface, &e)
    };
    let leave_board = Arc::clone(board);
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .background(backdrop)
        .on_sized({
            let surface = surface.clone();
            move |e: Event<SizedEventData>| surface.sized(e.area)
        })
        .on_wheel({
            let surface = surface.clone();
            move |e: Event<WheelEventData>| surface.wheel(&e)
        })
        .on_mouse_down({
            let surface = surface.clone();
            move |e: Event<MouseEventData>| surface.mouse_down(&e)
        })
        .on_global_pointer_move(on(Surface::global_move))
        .on_global_pointer_up(on(Surface::global_up))
        .on_touch_start(touch(Surface::touch_start))
        .on_touch_move(touch(Surface::touch_move))
        .on_touch_end(touch(Surface::touch_end))
        .on_touch_cancel(touch(Surface::touch_cancel))
        .on_pointer_move(on(Surface::pointer_move))
        .on_pointer_leave(move |_: Event<PointerEventData>| {
            lock(&leave_board).set_pointer(None);
        })
        .on_pointer_down(on(Surface::pointer_down))
        // Canvases paint in element-local coordinates, the space pen
        // samples arrive in; text boxes sit between their layers' passes.
        .child(crate::canvas_text::CanvasLayers {
            board: Arc::clone(board),
            editor: text_editor,
        })
}

/// Lock the shared board, recovering transparently from mutex
/// poisoning. Every mutation site is a short vec push with no
/// panic-prone code — forward progress beats an unactionable error.
/// Ghost ink from other devices is painted this much fainter than the
/// author sees it.
const REMOTE_INK_ALPHA: f32 = 0.7;

impl Board {
    /// Route ephemeral state (ink under the pen, pointer) to `sink`
    /// while this board shows a document others may be looking at.
    pub fn set_live_sink(&mut self, sink: impl Fn(LiveMessage) + Send + Sync + 'static) {
        self.live_sink = Some(LiveSink(Arc::new(sink)));
    }

    pub fn clear_live_sink(&mut self) {
        self.live_sink = None;
    }

    /// Ask for a repaint (an overlay painted over the board changed).
    pub fn request_repaint(&mut self) {
        self.notify();
    }

    /// Refuse every edit while `read_only`; what others do still lands.
    pub fn set_read_only(&mut self, read_only: bool) {
        if self.read_only == read_only {
            return;
        }
        self.read_only = read_only;
        if read_only {
            self.cancel();
            self.end_text_session();
        }
        self.notify();
    }

    #[must_use]
    pub const fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Everyone else working on this document right now.
    pub fn remote_hands(&self) -> impl Iterator<Item = (&notein_sync::DeviceId, &RemoteHand)> {
        self.remote.iter()
    }

    /// Apply what another device is doing: extend or finish its ghost
    /// stroke, move its pointer. Silent hands expire here too.
    #[sismo::function]
    pub fn apply_live(&mut self, from: notein_sync::DeviceId, name: &str, message: LiveMessage) {
        let now = Instant::now();
        self.remote
            .retain(|_, hand| now.duration_since(hand.seen) < REMOTE_TTL);
        let hand = self.remote.entry(from).or_insert_with(|| RemoteHand {
            name: name.to_owned(),
            stroke: None,
            pointer: None,
            seen: now,
        });
        hand.seen = now;
        name.clone_into(&mut hand.name);
        match message {
            LiveMessage::Ink {
                stroke,
                preset,
                color,
                opacity,
                points,
                done,
            } => {
                let same = hand
                    .stroke
                    .as_ref()
                    .is_some_and(|r| r.stroke.id == stroke && r.done_at.is_none());
                if !same {
                    let Some(first) = points.first().copied() else {
                        if done {
                            hand.stroke = None;
                        }
                        self.notify();
                        return;
                    };
                    let mut ghost = Stroke::new(stroke, BrushId::default(), color, first);
                    ghost.points.extend(points.iter().skip(1).copied());
                    hand.stroke = Some(RemoteStroke {
                        stroke: ghost,
                        preset,
                        opacity,
                        done_at: None,
                    });
                } else if let Some(remote) = hand.stroke.as_mut() {
                    remote.stroke.points.extend(points);
                    remote.preset = preset;
                    remote.opacity = opacity;
                }
                if done && let Some(remote) = hand.stroke.as_mut() {
                    remote.done_at = Some(now);
                }
            }
            LiveMessage::Pointer { x, y, shown } => {
                hand.pointer = shown.then_some((x, y));
            }
            LiveMessage::Left => {
                self.remote.remove(&from);
            }
            LiveMessage::Presence { .. } => {}
        }
        self.settle_remote();
        self.notify();
    }

    /// `from` is gone: its ghost ink and pointer go with it.
    pub fn forget_hand(&mut self, from: notein_sync::DeviceId) {
        if self.remote.remove(&from).is_some() {
            self.notify();
        }
    }

    /// Drop hands that went silent and settled ghost strokes, without
    /// waiting for another live message to arrive.
    pub fn expire_hands(&mut self) {
        let now = Instant::now();
        let before = self.remote.len();
        let strokes = self.remote.values().filter(|h| h.stroke.is_some()).count();
        self.remote
            .retain(|_, hand| now.duration_since(hand.seen) < REMOTE_TTL);
        self.settle_remote();
        let after = self.remote.values().filter(|h| h.stroke.is_some()).count();
        if self.remote.len() != before || after != strokes {
            self.notify();
        }
    }

    /// Drop ghost strokes whose committed twin is in the document, or
    /// that finished a while ago without one.
    fn settle_remote(&mut self) {
        let now = Instant::now();
        for hand in self.remote.values_mut() {
            let gone = hand.stroke.as_ref().is_some_and(|remote| {
                self.doc.find_stroke(remote.stroke.id).is_some()
                    || remote
                        .done_at
                        .is_some_and(|at| now.duration_since(at) > REMOTE_DONE_TTL)
            });
            if gone {
                hand.stroke = None;
            }
        }
    }

    fn emit_live_ink(&mut self, done: bool) {
        if self.live_sink.is_none() {
            return;
        }
        let Some(active) = self.active.take() else {
            return;
        };
        self.emit_live_ink_for(&active, done);
        self.active = Some(active);
    }

    fn emit_live_ink_for(&mut self, stroke: &Stroke, done: bool) {
        let Some(sink) = self.live_sink.clone() else {
            return;
        };
        let due = done
            || self
                .live_last
                .is_none_or(|last| last.elapsed() >= LIVE_INK_INTERVAL);
        if !due {
            return;
        }
        // Rubber-band strokes rewrite their last point: resend them whole.
        let from = if self.two_point_active {
            0
        } else {
            self.live_sent.min(stroke.points.len())
        };
        let points = stroke.points[from..].to_vec();
        if points.is_empty() && !done {
            return;
        }
        let opacity = self
            .active_layer_at_begin
            .and_then(|id| self.doc.layer(id))
            .map_or(1.0, |l| l.opacity);
        (sink.0)(LiveMessage::Ink {
            stroke: stroke.id,
            preset: self.current_preset,
            color: stroke.color,
            opacity,
            points,
            done,
        });
        self.live_sent = stroke.points.len();
        self.live_last = Some(Instant::now());
    }

    fn emit_live_pointer(&mut self) {
        let Some(sink) = self.live_sink.clone() else {
            return;
        };
        let shown = self.pointer.is_some() && !self.gesture_active;
        let due = !shown
            || self
                .pointer_last
                .is_none_or(|last| last.elapsed() >= LIVE_POINTER_INTERVAL);
        if !due {
            return;
        }
        let (x, y) = self
            .pointer
            .map_or((0.0, 0.0), |(sx, sy)| self.viewport.screen_to_world(sx, sy));
        (sink.0)(LiveMessage::Pointer { x, y, shown });
        self.pointer_last = Some(Instant::now());
    }
}

/// A stable, readable colour for a device, from its id.
fn device_color(device: notein_sync::DeviceId) -> [u8; 4] {
    let [r, g, b] = crate::sync::presence::color(device);
    [r, g, b, 255]
}

/// Another person's pointer: a small disc in their colour with a
/// white halo so it reads on any paper, and their name beside it.
fn draw_remote_pointer(
    canvas: &freya_engine::prelude::Canvas,
    cx: f32,
    cy: f32,
    color: [u8; 4],
    name: &str,
) {
    const RADIUS: f32 = 5.0;
    let mut halo = Paint::default();
    halo.set_color(SkColor::from_argb(200, 255, 255, 255));
    halo.set_style(PaintStyle::Fill);
    halo.set_anti_alias(true);
    canvas.draw_circle((cx, cy), RADIUS + 2.0, &halo);
    let mut dot = Paint::default();
    dot.set_color(SkColor::from_argb(color[3], color[0], color[1], color[2]));
    dot.set_style(PaintStyle::Fill);
    dot.set_anti_alias(true);
    canvas.draw_circle((cx, cy), RADIUS, &dot);
    draw_name_tag(canvas, cx + RADIUS + 4.0, cy + RADIUS + 2.0, color, name);
}

/// `name` on a pill of `color`, its top left corner at `(x, y)`.
fn draw_name_tag(
    canvas: &freya_engine::prelude::Canvas,
    x: f32,
    y: f32,
    color: [u8; 4],
    name: &str,
) {
    use freya_engine::prelude::{Font, FontMgr, FontStyle, Rect, Typeface};
    const SIZE: f32 = 11.5;
    const PAD_X: f32 = 6.0;
    const PAD_Y: f32 = 3.0;
    thread_local! {
        static FACE: Option<Typeface> = FontMgr::new().legacy_make_typeface(None, FontStyle::bold());
    }
    if name.is_empty() {
        return;
    }
    let Some(face) = FACE.with(Clone::clone) else {
        return;
    };
    let font = Font::from_typeface(face, SIZE);
    let (width, _) = font.measure_str(name, None);
    let (_, metrics) = font.metrics();
    let height = PAD_Y.mul_add(2.0, SIZE);
    let mut pill = Paint::default();
    pill.set_color(SkColor::from_argb(235, color[0], color[1], color[2]));
    pill.set_anti_alias(true);
    canvas.draw_round_rect(
        Rect::from_xywh(x, y, PAD_X.mul_add(2.0, width), height),
        height / 2.0,
        height / 2.0,
        &pill,
    );
    let mut ink = Paint::default();
    ink.set_color(SkColor::from_argb(255, 21, 21, 26));
    ink.set_anti_alias(true);
    canvas.draw_str(name, (x + PAD_X, y + PAD_Y - metrics.ascent), &font, &ink);
}

pub fn lock(board: &Arc<Mutex<Board>>) -> std::sync::MutexGuard<'_, Board> {
    match board.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// How long a jump of the view takes.
const GLIDE: Duration = Duration::from_millis(250);
/// Steps of a jump, about one per frame.
const GLIDE_STEPS: u16 = 15;

/// Move `board`'s view to `target` over a quarter second, easing out.
///
/// A jump (a page, a bookmark, back to 100 %) keeps the reader oriented. A
/// jump farther than two screens, or with reduced motion, cuts straight
/// there; touching the canvas mid-way stops the glide where it is.
pub fn glide(board: &Arc<Mutex<Board>>, target: Viewport) {
    let (from, (sw, sh)) = {
        let guard = lock(board);
        (guard.viewport(), guard.last_surface_size)
    };
    let far = (target.tx - from.tx).hypot(target.ty - from.ty) > 2.0 * sw.max(sh);
    if far || notein_ui_kit::reduce_motion() {
        lock(board).set_viewport(target);
        return;
    }
    let board = Arc::clone(board);
    spawn(async move {
        let mut last = from;
        for step in 1..=GLIDE_STEPS {
            timer(GLIDE / u32::from(GLIDE_STEPS)).await;
            let mut guard = lock(&board);
            if guard.viewport() != last {
                return;
            }
            let t = 1.0 - (1.0 - f32::from(step) / f32::from(GLIDE_STEPS)).powi(3);
            guard.set_viewport(Viewport {
                tx: (target.tx - from.tx).mul_add(t, from.tx),
                ty: (target.ty - from.ty).mul_add(t, from.ty),
                scale: (target.scale - from.scale).mul_add(t, from.scale),
            });
            last = guard.viewport();
        }
    });
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
/// Pin outer radius in surface pixels. Matches the `SOURCES_PLAN` §7
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
    const SEGMENTS: u16 = 24;
    const SUBSTEPS: u16 = 4;
    let mut path = PathBuilder::new();
    let step = std::f32::consts::TAU / f32::from(SEGMENTS);
    for segment in (0..SEGMENTS).step_by(2) {
        let start = step * f32::from(segment);
        for sub in 0..=SUBSTEPS {
            let angle = (f32::from(sub) / f32::from(SUBSTEPS)).mul_add(step, start);
            let point = (
                radius.mul_add(angle.cos(), cx),
                radius.mul_add(angle.sin(), cy),
            );
            if sub == 0 {
                path.move_to(point);
            } else {
                path.line_to(point);
            }
        }
    }
    path.detach()
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

/// The person moved the view themselves: whoever they followed, they
/// stop.
fn stop_following() {
    if crate::sync::presence::following().is_some() {
        crate::sync::presence::follow(None);
    }
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

    fn ghost(b: &mut Board, from: notein_sync::DeviceId) {
        b.apply_live(
            from,
            "Ana",
            LiveMessage::Ink {
                stroke: StrokeId([4; 16]),
                preset: BrushPreset::pen(),
                color: [0, 0, 0, 255],
                opacity: 1.0,
                points: vec![
                    crate::brush::InkPoint::new(0.0, 0.0, 128, 0, 0),
                    crate::brush::InkPoint::new(5.0, 5.0, 128, 0, 0),
                ],
                done: false,
            },
        );
        b.apply_live(
            from,
            "Ana",
            LiveMessage::Pointer {
                x: 5.0,
                y: 5.0,
                shown: true,
            },
        );
    }

    #[test]
    fn a_peer_that_leaves_takes_its_ghost_ink_and_pointer() {
        let mut b = Board::default();
        let ana = notein_sync::DeviceId([1; 32]);
        ghost(&mut b, ana);
        assert_eq!(b.remote_hands().count(), 1);
        b.forget_hand(ana);
        assert_eq!(b.remote_hands().count(), 0);
    }

    #[test]
    fn a_silent_hand_expires_without_another_message() {
        let mut b = Board::default();
        let ana = notein_sync::DeviceId([1; 32]);
        ghost(&mut b, ana);
        b.expire_hands();
        assert_eq!(b.remote_hands().count(), 1, "still fresh");
        if let Some(hand) = b.remote.get_mut(&ana) {
            hand.seen = Instant::now() - REMOTE_TTL * 2;
        }
        b.expire_hands();
        assert_eq!(b.remote_hands().count(), 0);
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
        for i in 1..=10_u8 {
            b.extend(pt(f32::from(i) * 10.0, 0.0));
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
        assert_eq!(b.active_layer_id(), before_active, "active layer restored");
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

        b.update_bookmark(bm_id, "nota".into(), vec![], Some([10, 20, 30, 255]), 2_000);
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
    fn read_only_board_refuses_strokes_and_undo_but_takes_remote_changes() {
        let mut b = Board::default();
        b.set_current_preset(BrushPreset::pen());
        b.begin(pt(0.0, 0.0));
        b.extend(pt(10.0, 0.0));
        b.end();
        assert_eq!(b.doc().layer(0).unwrap().strokes.len(), 1);

        b.set_read_only(true);
        b.begin(pt(0.0, 20.0));
        b.extend(pt(10.0, 20.0));
        b.end();
        assert_eq!(b.doc().layer(0).unwrap().strokes.len(), 1);
        assert!(!b.undo());
        b.clear();
        assert_eq!(b.doc().layer(0).unwrap().strokes.len(), 1);

        let mut other = Board::default();
        other.replace_doc(Doc::from_snapshot(&b.doc().snapshot_bytes().unwrap()).unwrap());
        other.set_current_preset(BrushPreset::pen());
        other.begin(pt(0.0, 40.0));
        other.extend(pt(10.0, 40.0));
        other.end();
        let updates = other.doc().updates_since(&b.doc().version()).unwrap();
        b.merge_updates(&updates).unwrap();
        assert_eq!(b.doc().layer(0).unwrap().strokes.len(), 2);

        b.set_read_only(false);
        b.begin(pt(0.0, 60.0));
        b.extend(pt(10.0, 60.0));
        b.end();
        assert_eq!(b.doc().layer(0).unwrap().strokes.len(), 3);
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
