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
use freya_engine::prelude::{BlendMode, Color as SkColor, Paint, PaintStyle, PathBuilder};

use crate::brush::{BrushKind, BrushPreset, InkPoint, SegmentMode, Stroke, StrokeStyle};
use crate::doc::Doc;
use crate::history::{EraseSession, HistoryOp};
use crate::spatial::SpatialIndex;

const SIZE_SCALE_MIN: f32 = 0.25;
const SIZE_SCALE_MAX: f32 = 4.0;

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
    current_preset: BrushPreset,
    current_color: [u8; 4],
    size_scales: HashMap<BrushKind, f32>,
    spatial: SpatialIndex,
    history: Vec<HistoryOp>,
    erase_session: Option<EraseSession>,
    notifier: Option<RedrawNotifier>,
}

impl Default for Board {
    fn default() -> Self {
        let default_preset = BrushPreset::pen();
        Self {
            doc: Doc::default(),
            active: None,
            current_preset: default_preset,
            current_color: default_preset.color,
            size_scales: HashMap::new(),
            spatial: SpatialIndex::new(),
            history: Vec::new(),
            erase_session: None,
            notifier: None,
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

    /// Replace the doc wholesale — used by load-from-disk. Rebuilds the
    /// spatial index and drops any in-flight session.
    pub fn replace_doc(&mut self, doc: Doc) {
        self.doc = doc;
        self.active = None;
        self.erase_session = None;
        self.history.clear();
        self.spatial.clear();
        for stroke in &self.doc.strokes {
            self.spatial.insert(stroke.id, &stroke.points);
        }
        self.notify();
    }

    pub fn begin(&mut self, point: InkPoint) {
        if let Some(active) = self.active.take() {
            self.commit_stroke(active);
        }
        if self.current_kind() == BrushKind::Eraser {
            self.erase_session = Some(EraseSession::default());
            self.apply_erase(point);
            self.notify();
            return;
        }
        let brush = self.doc.register_brush(self.current_preset);
        let id = self.doc.allocate_stroke_id();
        self.active = Some(Stroke::new(id, brush, self.current_color, point));
        self.notify();
    }

    pub fn extend(&mut self, point: InkPoint) {
        if self.erase_session.is_some() {
            self.apply_erase(point);
            self.notify();
            return;
        }
        if let Some(active) = self.active.as_mut() {
            active.points.push(point);
            self.notify();
        }
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
            self.notify();
        }
    }

    pub fn clear(&mut self) {
        self.doc.clear();
        self.active = None;
        self.erase_session = None;
        self.history.clear();
        self.spatial.clear();
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

    fn commit_stroke(&mut self, stroke: Stroke) {
        if stroke.points.is_empty() {
            return;
        }
        self.spatial.insert(stroke.id, &stroke.points);
        self.doc.insert_stroke(stroke);
    }

    fn apply_erase(&mut self, sample: InkPoint) {
        let preset = self.current_preset;
        let radius = 0.5 * preset.width(sample.pressure_f32(), sample.tilt_f32());
        if radius <= 0.0 {
            return;
        }
        let candidates = self.spatial.query_circle(sample.x, sample.y, radius);
        for id in candidates {
            self.erase_stroke(id, sample.x, sample.y, radius);
        }
    }

    fn erase_stroke(&mut self, id: u32, cx: f32, cy: f32, r: f32) {
        let Some(idx) = self.doc.strokes.iter().position(|s| s.id == id) else {
            return;
        };
        let stroke = &self.doc.strokes[idx];
        let outcome = split_polyline(&stroke.points, cx, cy, r);
        if !outcome.touched {
            return;
        }

        let original = self.doc.strokes.remove(idx);
        self.spatial.remove(original.id, &original.points);

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
            self.doc.insert_stroke(Stroke {
                id: frag_id,
                brush: original.brush,
                color: original.color,
                points: fragment,
            });
            let last = self.doc.strokes.last().expect("just inserted");
            self.spatial.insert(last.id, &last.points);
        }

        if let Some(session) = self.erase_session.as_mut() {
            session.record(original, &fragment_ids);
        }
    }

    fn rollback_session(&mut self, session: &EraseSession) {
        for frag_id in &session.added_fragments {
            if let Some(removed) = self.doc.remove_stroke(*frag_id) {
                self.spatial.remove(removed.id, &removed.points);
            }
        }
        for original in &session.originals {
            self.doc.insert_stroke(original.clone());
            self.spatial.insert(original.id, &original.points);
        }
    }

    /// Paint every committed stroke plus the active one onto `canvas`.
    pub fn paint(&self, canvas: &freya_engine::prelude::Canvas) {
        for stroke in &self.doc.strokes {
            if let Some(preset) = self.doc.preset(stroke.brush) {
                paint_stroke(canvas, preset, stroke);
            }
        }
        if let Some(active) = &self.active {
            if let Some(preset) = self.doc.preset(active.brush) {
                paint_stroke(canvas, preset, active);
            }
        }
    }
}

/// Variable-width ribbon renderer.
///
/// Draws a filled circle at every sample vertex plus a trapezoid
/// connecting consecutive circles. Because the trapezoid interpolates
/// half-width linearly between endpoints and the vertex circles cap
/// every seam, pressure ramps (e.g. lift-off tapers) render as a smooth
/// wedge instead of the stepped rectangles a variable-stroke-width
/// `draw_line` per segment produces. One `Paint` config per stroke —
/// colour + opacity + blend mode are stroke-invariant.
fn paint_stroke(canvas: &freya_engine::prelude::Canvas, preset: &BrushPreset, stroke: &Stroke) {
    if stroke.points.is_empty() {
        return;
    }
    let style = preset.stroke_style(stroke.color);
    let mut paint = Paint::default();
    configure_fill_paint(&mut paint, &style);

    let widths: Vec<f32> = stroke
        .points
        .iter()
        .map(|p| 0.5 * preset.width(p.pressure_f32(), p.tilt_f32()))
        .collect();

    for (point, half_w) in stroke.points.iter().zip(&widths) {
        if *half_w > 0.0 {
            canvas.draw_circle((point.x, point.y), *half_w, &paint);
        }
    }

    for i in 0..stroke.points.len().saturating_sub(1) {
        draw_trapezoid(
            canvas,
            &paint,
            stroke.points[i],
            stroke.points[i + 1],
            widths[i],
            widths[i + 1],
        );
    }
}

fn draw_trapezoid(
    canvas: &freya_engine::prelude::Canvas,
    paint: &Paint,
    from: InkPoint,
    to: InkPoint,
    half_from: f32,
    half_to: f32,
) {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let len = dx.hypot(dy);
    if len < 1e-3 {
        return;
    }
    // Left-hand perpendicular unit vector.
    let nx = -dy / len;
    let ny = dx / len;

    let mut builder = PathBuilder::new();
    builder
        .move_to((nx.mul_add(half_from, from.x), ny.mul_add(half_from, from.y)))
        .line_to((nx.mul_add(half_to, to.x), ny.mul_add(half_to, to.y)))
        .line_to((nx.mul_add(-half_to, to.x), ny.mul_add(-half_to, to.y)))
        .line_to((
            nx.mul_add(-half_from, from.x),
            ny.mul_add(-half_from, from.y),
        ))
        .close();
    canvas.draw_path(&builder.detach(), paint);
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

    rect()
        .width(Size::fill())
        .height(Size::fill())
        .background(Color::from_rgb(250, 250, 248))
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
        assert_ne!(b.doc().strokes.len(), 1, "erase should have split");

        assert!(b.undo());
        assert_eq!(b.doc().strokes.len(), 1);
        assert_eq!(b.doc().strokes[0].points, doc_before.strokes[0].points);
    }
}
