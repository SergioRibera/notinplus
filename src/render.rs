//! Extensible brush + cap rendering.
//!
//! Two orthogonal extension points:
//!
//! - [`CapRenderer`] — how a stroke's endpoint is closed. The
//!   built-in variants ([`RoundCap`], [`FlatCap`]) map to
//!   [`CapStyle::Round`] and [`CapStyle::Flat`]. External crates
//!   register additional renderers against [`CapStyle::Custom`] ids.
//! - [`BrushRenderer`] — how a whole [`Stroke`] becomes a Skia
//!   [`Path`] + [`Paint`]. The built-in [`RibbonBrush`] handles every
//!   default kind (Pencil / Marker / Pen / Highlighter / Eraser).
//!   External brushes register against [`BrushKind::Custom`] ids.
//!
//! Both live in a [`BrushRegistry`] the [`crate::canvas::Board`] owns.
//! Unknown ids fall back to the default renderer so a doc authored
//! with a plugin still opens without it.

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::{Arc, RwLock};

use freya_engine::prelude::{
    BlendMode, Color as SkColor, Paint, PaintStyle, Path, PathBuilder, PathDirection,
};

use crate::brush::{
    BrushKind, BrushPreset, CapStyle, HighlighterTip, InkPoint, SegmentMode, ShapeMode, Stroke,
    StrokeStyle,
};

/// Ribbon vertex: position, local half-width, left-hand unit normal.
/// Cap renderers consume one of these to know where to bulge past.
#[derive(Clone, Copy, Debug)]
pub struct RibbonVert {
    pub x: f32,
    pub y: f32,
    pub half: f32,
    pub nx: f32,
    pub ny: f32,
}

/// Which end of the stroke a cap belongs to. Governs the tangent
/// orientation the arc bulges along — forward past the tip on `End`,
/// backward past the head on `Start`.
#[derive(Clone, Copy, Debug)]
pub enum EndSide {
    Start,
    End,
}

/// Contract for terminating one side of a ribbon polygon.
///
/// The cursor position on entry depends on `side`:
///
/// - [`EndSide::End`]: cursor at top-edge (`vert + n*half`), must
///   line/arc to bottom-edge (`vert - n*half`).
/// - [`EndSide::Start`]: cursor at bottom-edge (`vert - n*half`), must
///   line/arc to top-edge (`vert + n*half`) so the polygon `close()`
///   connects at zero length.
///
/// `Flat` therefore emits a single `line_to` the opposite edge; `Round`
/// sweeps a semicircle bulging along the tangent away from the stroke
/// body (forward past the tip on `End`, backward past the head on
/// `Start`).
pub trait CapRenderer: Send + Sync + Debug {
    fn emit(&self, builder: &mut PathBuilder, vert: RibbonVert, side: EndSide);
}

/// Contract for turning a whole [`Stroke`] into rendered geometry.
///
/// Split into `build_path` (fill geometry, cached by [`crate::canvas::Board`])
/// and `paint` (colour + blend mode, rebuilt each frame so live
/// opacity changes propagate without invalidating the cache).
pub trait BrushRenderer: Send + Sync + Debug {
    fn build_path(&self, preset: &BrushPreset, stroke: &Stroke, caps: &CapRegistry) -> Path;
    fn paint(&self, preset: &BrushPreset, stroke_color: [u8; 4]) -> Paint;

    /// Visual cue for the pointer indicator — a floating circle (or
    /// nothing) painted at the current pen / mouse surface position so
    /// the user can gauge how much they're about to paint or erase.
    ///
    /// Default: a thin outlined disc sized to the brush's mid-pressure
    /// width in [`preset.color`]. Returned radius is in **world units**
    /// — the canvas scales by the current viewport so a 2× zoom shows
    /// a 2× pointer. Return [`PointerStyle::Hidden`] to opt a brush
    /// out entirely.
    fn pointer_style(&self, preset: &BrushPreset, stroke_color: [u8; 4]) -> PointerStyle {
        let radius = preset.width(0.5, 0.0) * 0.5;
        match preset.kind {
            BrushKind::Eraser => PointerStyle::Dashed {
                radius,
                color: [60, 60, 60, 220],
            },
            BrushKind::Shape(_) => PointerStyle::Hidden,
            _ => PointerStyle::Outline {
                radius,
                color: stroke_color,
            },
        }
    }
}

/// How the live pointer indicator paints. Variants expose enough knob
/// surface for the renderer to style the overlay without the paint
/// pass hard-coding each brush family. Position + viewport-scaled
/// radius come from the caller; the style only picks colour / stroke
/// treatment.
#[derive(Clone, Copy, Debug)]
pub enum PointerStyle {
    /// Suppress the overlay entirely for this brush.
    Hidden,
    /// Thin outlined circle. The default — matches the brush size and
    /// borrows the stroke colour so the user sees exactly where ink
    /// will land.
    Outline { radius: f32, color: [u8; 4] },
    /// Dashed outline. Reserved for destructive tools (eraser) so the
    /// cursor reads differently from a "place ink here" indicator.
    Dashed { radius: f32, color: [u8; 4] },
}

/// Rounded semicircle cap. Bulges 180° past the tip along the
/// tangent direction, approximated with `CAP_ARC_STEPS` line segments
/// so no Skia arc-primitive API is required.
#[derive(Debug, Default)]
pub struct RoundCap;

const CAP_ARC_STEPS: usize = 16;

impl CapRenderer for RoundCap {
    fn emit(&self, builder: &mut PathBuilder, v: RibbonVert, side: EndSide) {
        if v.half <= 0.0 {
            return;
        }
        // Tangent unit vector recovered from the stored left-hand
        // normal: `n = (-uy, ux)` implies `u = (ny, -nx)`.
        let tx = v.ny;
        let ty = -v.nx;
        // Bulge direction: forward (+u) at End, backward (-u) at Start.
        let sign = match side {
            EndSide::End => 1.0,
            EndSide::Start => -1.0,
        };
        let steps_f = f32_from_usize(CAP_ARC_STEPS);
        // θ traversal must match cursor origin. End enters at top-edge
        // (θ=0 gives +n*half) and finishes at bottom-edge (θ=π). Start
        // enters at bottom-edge (θ=π) and finishes at top-edge (θ=0).
        // Reversing the step index for Start avoids drawing a diagonal
        // line straight across the stroke width on the very first
        // `line_to`, which used to produce a visible cut/notch at the
        // start of every user-drawn stroke.
        for k in 1..=CAP_ARC_STEPS {
            let step = match side {
                EndSide::End => k,
                EndSide::Start => CAP_ARC_STEPS - k,
            };
            let theta = f32_from_usize(step) * std::f32::consts::PI / steps_f;
            let cos_t = theta.cos();
            let sin_t = theta.sin();
            let x =
                v.nx.mul_add(cos_t * v.half, tx.mul_add(sign * sin_t * v.half, v.x));
            let y =
                v.ny.mul_add(cos_t * v.half, ty.mul_add(sign * sin_t * v.half, v.y));
            builder.line_to((x, y));
        }
    }
}

/// Flat cap: line straight from the top vertex to the bottom vertex,
/// perpendicular to the local tangent. Matches the geometry the
/// eraser leaves at cut boundaries.
#[derive(Debug, Default)]
pub struct FlatCap;

impl CapRenderer for FlatCap {
    fn emit(&self, builder: &mut PathBuilder, v: RibbonVert, side: EndSide) {
        // Opposite-edge endpoint depends on which side we terminate:
        // End cursor at top → line to bottom; Start cursor at bottom →
        // line to top. Zero-length line at Start when the previous
        // `close()` would have connected anyway, but keeps the cap
        // contract uniform for custom renderers.
        let (x, y) = match side {
            EndSide::End => ((-v.nx).mul_add(v.half, v.x), (-v.ny).mul_add(v.half, v.y)),
            EndSide::Start => (v.nx.mul_add(v.half, v.x), v.ny.mul_add(v.half, v.y)),
        };
        builder.line_to((x, y));
    }
}

/// Default variable-width ribbon renderer.
///
/// Emits one closed polygon covering the whole stroke, walking the
/// polyline as a single sequence so subdivision vertices are shared
/// across adjacent Catmull-Rom segments (no between-slab seams at
/// high zoom). Terminates each end via the registered
/// [`CapRenderer`]. Reinforces every original sample knot with a
/// filled circle — nonzero fill accumulates with the polygon, so the
/// circles cover any inner-side fold on tight turns without punching
/// holes on gentle ones. Non-highlighter kinds all end up here;
/// highlighter also uses it but its `Paint` picks `BlendMode::Multiply`.
#[derive(Debug, Default)]
pub struct RibbonBrush;

impl BrushRenderer for RibbonBrush {
    fn build_path(&self, preset: &BrushPreset, stroke: &Stroke, caps: &CapRegistry) -> Path {
        build_ribbon_path(
            preset,
            &stroke.points,
            stroke.cap_start,
            stroke.cap_end,
            caps,
        )
    }

    fn paint(&self, preset: &BrushPreset, stroke_color: [u8; 4]) -> Paint {
        let style = preset.stroke_style(stroke_color);
        let mut paint = Paint::default();
        configure_fill_paint(&mut paint, &style);
        paint
    }
}

/// Shared tip-shape state for the highlighter kind.
///
/// Owned by [`crate::canvas::Board`] and read by [`HighlighterBrush`]
/// each `build_path`. `RwLock` because reads happen on every paint
/// and writes only on popup toggles — reader-heavy access pattern.
/// Kept off the wire: it is a UI-level knob whose default matches
/// the pre-Phase-3 behaviour ([`HighlighterTip::Round`]).
#[derive(Debug)]
pub struct HighlighterState {
    inner: RwLock<HighlighterTip>,
}

impl HighlighterState {
    #[must_use]
    pub const fn new(tip: HighlighterTip) -> Self {
        Self {
            inner: RwLock::new(tip),
        }
    }

    /// Overwrite the current tip. Silently ignores mutex poisoning —
    /// the only writer is a brief popup callback and the read side
    /// recovers by defaulting to `Round`.
    pub fn set(&self, tip: HighlighterTip) {
        if let Ok(mut g) = self.inner.write() {
            *g = tip;
        }
    }

    /// Snapshot the current tip. Poisoned lock falls back to
    /// [`HighlighterTip::Round`] so a corrupted state never bricks
    /// paint.
    #[must_use]
    pub fn get(&self) -> HighlighterTip {
        self.inner.read().map_or(HighlighterTip::Round, |g| *g)
    }
}

impl Default for HighlighterState {
    fn default() -> Self {
        Self::new(HighlighterTip::Round)
    }
}

/// Highlighter renderer that dispatches between the default ribbon
/// path (for [`HighlighterTip::Round`]) and a fixed-angle offset
/// polygon (for [`HighlighterTip::Bevel`]).
///
/// Angle is expressed in degrees, measured counter-clockwise from the
/// positive x-axis, and interpreted as the direction of the "height"
/// vector of the flat tip (perpendicular to the tip's flat edge).
/// A `Bevel { angle_deg: 0.0 }` stripe therefore has the tip's flat
/// edge along the y-axis, so a stroke drawn horizontally shows full
/// width and a vertical stroke collapses to a thin line.
#[derive(Debug)]
pub struct HighlighterBrush {
    round: RibbonBrush,
    state: Arc<HighlighterState>,
}

impl HighlighterBrush {
    #[must_use]
    pub const fn new(state: Arc<HighlighterState>) -> Self {
        Self {
            round: RibbonBrush,
            state,
        }
    }
}

impl BrushRenderer for HighlighterBrush {
    fn build_path(&self, preset: &BrushPreset, stroke: &Stroke, caps: &CapRegistry) -> Path {
        match self.state.get() {
            HighlighterTip::Round => self.round.build_path(preset, stroke, caps),
            HighlighterTip::Bevel { angle_deg } => build_bevel_path(preset, stroke, angle_deg),
        }
    }

    fn paint(&self, preset: &BrushPreset, stroke_color: [u8; 4]) -> Paint {
        // Blend mode + colour do not depend on tip shape — highlighter
        // is always Multiply.
        self.round.paint(preset, stroke_color)
    }
}

/// Constant-width stripe swept along the polyline, offset perpendicular
/// to the fixed bevel-tip axis (not the local tangent). Zero caps —
/// bevel tips terminate flat by construction.
fn build_bevel_path(preset: &BrushPreset, stroke: &Stroke, angle_deg: f32) -> Path {
    let mut builder = PathBuilder::new();
    let points = &stroke.points;
    if points.len() < 2 {
        return builder.detach();
    }
    let half = 0.5 * preset.width(0.5, 0.0);
    if half <= 0.0 {
        return builder.detach();
    }
    let angle_rad = angle_deg.to_radians();
    let (sin_a, cos_a) = angle_rad.sin_cos();
    let nx = cos_a * half;
    let ny = sin_a * half;
    // Top edge forward.
    builder.move_to((points[0].x + nx, points[0].y + ny));
    for p in &points[1..] {
        builder.line_to((p.x + nx, p.y + ny));
    }
    // Bottom edge backward.
    for p in points.iter().rev() {
        builder.line_to((p.x - nx, p.y - ny));
    }
    builder.close();
    builder.detach()
}

/// Geometric-primitive renderer for [`BrushKind::Shape`].
///
/// Each stroke stores exactly two anchor points (`points[0]` = A,
/// `points.last()` = B) captured by the two-anchor rubber-band path
/// in [`crate::canvas::Board`]. `build_path` interprets that pair
/// via the [`ShapeMode`] carried in the preset kind; `paint` returns
/// a stroked (not filled) paint at the preset's width.
///
/// Round caps + round joins so short-segment shapes (small arrows,
/// dashed segments) don't render with visible mitre spikes at high
/// zoom.
#[derive(Debug, Default)]
pub struct ShapeBrush;

impl BrushRenderer for ShapeBrush {
    fn build_path(&self, preset: &BrushPreset, stroke: &Stroke, _caps: &CapRegistry) -> Path {
        let mut b = PathBuilder::new();
        let BrushKind::Shape(mode) = preset.kind else {
            return b.detach();
        };
        let pts = &stroke.points;
        if pts.len() < 2 {
            return b.detach();
        }
        let a = pts[0];
        let z = pts[pts.len() - 1];
        match mode {
            ShapeMode::Line => {
                b.move_to((a.x, a.y));
                b.line_to((z.x, z.y));
            }
            ShapeMode::Dashed => emit_dashed_line(&mut b, (a.x, a.y), (z.x, z.y), 10.0, 6.0),
            ShapeMode::Arrow => {
                b.move_to((a.x, a.y));
                b.line_to((z.x, z.y));
                let width = preset.width(0.5, 0.0);
                emit_arrow_head(&mut b, (a.x, a.y), (z.x, z.y), (width * 5.0).max(10.0));
            }
            ShapeMode::Rect => {
                b.move_to((a.x, a.y));
                b.line_to((z.x, a.y));
                b.line_to((z.x, z.y));
                b.line_to((a.x, z.y));
                b.close();
            }
            ShapeMode::Circle => {
                let dx = z.x - a.x;
                let dy = z.y - a.y;
                let r = dx.hypot(dy);
                if r > 0.0 {
                    b.add_circle((a.x, a.y), r, PathDirection::CW);
                }
            }
        }
        b.detach()
    }

    fn paint(&self, preset: &BrushPreset, stroke_color: [u8; 4]) -> Paint {
        let mut paint = Paint::default();
        let [r, g, b_, a] = stroke_color;
        paint.set_anti_alias(true);
        paint.set_style(PaintStyle::Stroke);
        paint.set_stroke_width(preset.width(0.5, 0.0).max(1.0));
        paint.set_color(SkColor::from_argb(a, r, g, b_));
        paint
    }
}

fn emit_dashed_line(b: &mut PathBuilder, from: (f32, f32), to: (f32, f32), dash: f32, gap: f32) {
    let (ax, ay) = from;
    let (bx, by) = to;
    let dx = bx - ax;
    let dy = by - ay;
    let len = dx.hypot(dy);
    if len <= 0.0 {
        return;
    }
    let ux = dx / len;
    let uy = dy / len;
    let step = dash + gap;
    // Integer stride avoids the accumulated-float `while` clippy flags
    // and keeps a stable dash count under viewport zoom.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let count = ((len / step).ceil() as usize).max(1);
    for i in 0..count {
        #[allow(clippy::cast_precision_loss)]
        let start = (i as f32) * step;
        let end = (start + dash).min(len);
        b.move_to((ux.mul_add(start, ax), uy.mul_add(start, ay)));
        b.line_to((ux.mul_add(end, ax), uy.mul_add(end, ay)));
    }
}

fn emit_arrow_head(b: &mut PathBuilder, from: (f32, f32), to: (f32, f32), size: f32) {
    let (ax, ay) = from;
    let (bx, by) = to;
    let dx = bx - ax;
    let dy = by - ay;
    let len = dx.hypot(dy);
    if len <= 0.0 {
        return;
    }
    let ux = dx / len;
    let uy = dy / len;
    // Wings sweep back from the tip at ±25° off the line direction.
    let (sin_a, cos_a) = 25f32.to_radians().sin_cos();
    let wing1_x = (-ux).mul_add(cos_a, -(uy * sin_a));
    let wing1_y = (-uy).mul_add(cos_a, ux * sin_a);
    let wing2_x = (-ux).mul_add(cos_a, uy * sin_a);
    let wing2_y = (-uy).mul_add(cos_a, -(ux * sin_a));
    b.move_to((wing1_x.mul_add(size, bx), wing1_y.mul_add(size, by)));
    b.line_to((bx, by));
    b.line_to((wing2_x.mul_add(size, bx), wing2_y.mul_add(size, by)));
}

/// Registry mapping [`CapStyle`] → [`CapRenderer`]. Cloneable via
/// `Arc` so a snapshot can be handed to renderers without contention.
#[derive(Clone, Debug)]
pub struct CapRegistry {
    caps: HashMap<CapStyle, Arc<dyn CapRenderer>>,
    fallback: Arc<dyn CapRenderer>,
}

impl CapRegistry {
    #[must_use]
    pub fn new() -> Self {
        let mut caps: HashMap<CapStyle, Arc<dyn CapRenderer>> = HashMap::new();
        let round: Arc<dyn CapRenderer> = Arc::new(RoundCap);
        let flat: Arc<dyn CapRenderer> = Arc::new(FlatCap);
        caps.insert(CapStyle::Round, Arc::clone(&round));
        caps.insert(CapStyle::Flat, flat);
        Self {
            caps,
            fallback: round,
        }
    }

    pub fn set(&mut self, style: CapStyle, renderer: Arc<dyn CapRenderer>) {
        self.caps.insert(style, renderer);
    }

    #[must_use]
    pub fn get(&self, style: CapStyle) -> &dyn CapRenderer {
        self.caps
            .get(&style)
            .map_or_else(|| self.fallback.as_ref(), AsRef::as_ref)
    }
}

impl Default for CapRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Registry mapping [`BrushKind`] → [`BrushRenderer`] plus caps.
///
/// Owned by [`crate::canvas::Board`] behind an `Arc`; external crates
/// install custom renderers via [`Self::set_brush`] /
/// [`Self::set_cap`] before handing the registry to the Board.
#[derive(Clone, Debug)]
pub struct BrushRegistry {
    brushes: HashMap<BrushKind, Arc<dyn BrushRenderer>>,
    fallback: Arc<dyn BrushRenderer>,
    caps: CapRegistry,
}

impl BrushRegistry {
    #[must_use]
    pub fn new() -> Self {
        let mut brushes: HashMap<BrushKind, Arc<dyn BrushRenderer>> = HashMap::new();
        let ribbon: Arc<dyn BrushRenderer> = Arc::new(RibbonBrush);
        for k in [
            BrushKind::Pencil,
            BrushKind::Marker,
            BrushKind::Eraser,
            BrushKind::Pen,
            BrushKind::Highlighter,
        ] {
            brushes.insert(k, Arc::clone(&ribbon));
        }
        let shape: Arc<dyn BrushRenderer> = Arc::new(ShapeBrush);
        for mode in [
            ShapeMode::Line,
            ShapeMode::Dashed,
            ShapeMode::Arrow,
            ShapeMode::Rect,
            ShapeMode::Circle,
        ] {
            brushes.insert(BrushKind::Shape(mode), Arc::clone(&shape));
        }
        Self {
            brushes,
            fallback: ribbon,
            caps: CapRegistry::new(),
        }
    }

    pub fn set_brush(&mut self, kind: BrushKind, renderer: Arc<dyn BrushRenderer>) {
        self.brushes.insert(kind, renderer);
    }

    pub fn set_cap(&mut self, style: CapStyle, renderer: Arc<dyn CapRenderer>) {
        self.caps.set(style, renderer);
    }

    #[must_use]
    pub fn brush(&self, kind: BrushKind) -> &dyn BrushRenderer {
        self.brushes
            .get(&kind)
            .map_or_else(|| self.fallback.as_ref(), AsRef::as_ref)
    }

    #[must_use]
    pub const fn caps(&self) -> &CapRegistry {
        &self.caps
    }
}

impl Default for BrushRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn build_ribbon_path(
    preset: &BrushPreset,
    points: &[InkPoint],
    cap_start: CapStyle,
    cap_end: CapStyle,
    caps: &CapRegistry,
) -> Path {
    let mut builder = PathBuilder::new();
    let n = points.len();
    if n == 0 {
        return builder.detach();
    }
    let mut halves = smoothed_halves(preset, points);
    apply_flat_cap_taper(&mut halves, cap_start, cap_end);
    if n == 1 {
        let p = points[0];
        let half = halves[0];
        if half > 0.0
            && (matches!(cap_start, CapStyle::Round) || matches!(cap_end, CapStyle::Round))
        {
            builder.add_circle((p.x, p.y), half, PathDirection::CCW);
        }
        return builder.detach();
    }
    let verts = build_ribbon_vertices(points, &halves);
    if verts.len() < 2 {
        return builder.detach();
    }
    emit_ribbon_polygon(&mut builder, &verts, cap_start, cap_end, caps);
    // Sample-knot join reinforcement. Every interior sample gets a
    // filled circle at its position; nonzero winding accumulates the
    // circle with the ribbon polygon, so on tight turns the inner
    // fold that the offset polygon produces gets covered without
    // punching a hole. Uses the same smoothed + tapered widths as the
    // polygon walk, so no raw-pressure spike sticks out as a lump nor
    // does a full-width knot punch through the tapered cut end.
    for (i, p) in points[1..(n - 1)].iter().enumerate() {
        let half = halves[i + 1];
        if half > 0.0 {
            builder.add_circle((p.x, p.y), half, PathDirection::CCW);
        }
    }
    builder.detach()
}

/// Progressive width taper on samples adjacent to a [`CapStyle::Flat`]
/// terminator. Flat caps otherwise render as a perpendicular line at
/// full local half-width — visually a square wall at eraser cuts. The
/// taper shrinks the terminal vertex to `TAPER_TIP` of its smoothed
/// width, and the neighbour behind it to `TAPER_SHOULDER`, so the
/// ribbon narrows smoothly into the cut and the flat closing line is
/// a small stub rather than a full-width perpendicular slab.
fn apply_flat_cap_taper(halves: &mut [f32], cap_start: CapStyle, cap_end: CapStyle) {
    const TAPER_TIP: f32 = 0.3;
    const TAPER_SHOULDER: f32 = 0.7;
    let n = halves.len();
    if n == 0 {
        return;
    }
    if matches!(cap_end, CapStyle::Flat) {
        halves[n - 1] *= TAPER_TIP;
        if n >= 3 {
            halves[n - 2] *= TAPER_SHOULDER;
        }
    }
    if matches!(cap_start, CapStyle::Flat) {
        halves[0] *= TAPER_TIP;
        if n >= 3 {
            halves[1] *= TAPER_SHOULDER;
        }
    }
}

/// Walk every Catmull-Rom segment in order, collect interpolated
/// positions + half-widths, dedup shared knots between adjacent
/// segments, then compute per-vertex left-hand normals via central
/// differences over the joined polyline.
///
/// `halves` is the per-sample half-width array computed upstream —
/// pre-smoothed via [`smoothed_halves`] and, at flat cap terminators,
/// tapered via [`apply_flat_cap_taper`] so the caller can share the
/// same values with sample-knot reinforcement circles (raw pressure
/// used to spike a lump through the tapered ribbon polygon).
fn build_ribbon_vertices(points: &[InkPoint], halves: &[f32]) -> Vec<RibbonVert> {
    let n = points.len();
    let mut positions: Vec<(f32, f32, f32)> = Vec::with_capacity(n * 6);
    for k in 0..(n - 1) {
        let p1 = points[k];
        let p2 = points[k + 1];
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
        // Width knots mirror the position knots — reflect at boundaries
        // so the endpoint segment has a natural tangent instead of
        // clamping (which would flat-line the width over the first/last
        // step and reintroduce a visible shoulder).
        let h1 = halves[k];
        let h2 = halves[k + 1];
        let h0 = if k > 0 {
            halves[k - 1]
        } else {
            2.0f32.mul_add(h1, -h2)
        };
        let h3 = if k + 2 < n {
            halves[k + 2]
        } else {
            2.0f32.mul_add(h2, -h1)
        };
        let steps = subdivision_count(p1_xy, p2_xy);
        let steps_f = f32_from_usize(steps);
        let start_j = usize::from(k != 0);
        for j in start_j..=steps {
            let t = f32_from_usize(j) / steps_f;
            let q = catmull_rom_centripetal(p0, p1_xy, p2_xy, p3, t);
            let half = catmull_rom_1d(h0, h1, h2, h3, t).max(0.0);
            positions.push((q.0, q.1, half));
        }
    }
    let m = positions.len();
    let mut verts = Vec::with_capacity(m);
    for i in 0..m {
        let (x, y, half) = positions[i];
        let (tx, ty) = if i == 0 {
            (positions[1].0 - x, positions[1].1 - y)
        } else if i == m - 1 {
            (x - positions[i - 1].0, y - positions[i - 1].1)
        } else {
            (
                positions[i + 1].0 - positions[i - 1].0,
                positions[i + 1].1 - positions[i - 1].1,
            )
        };
        let len = tx.hypot(ty).max(1e-6);
        let ux = tx / len;
        let uy = ty / len;
        verts.push(RibbonVert {
            x,
            y,
            half,
            nx: -uy,
            ny: ux,
        });
    }
    verts
}

fn emit_ribbon_polygon(
    builder: &mut PathBuilder,
    verts: &[RibbonVert],
    cap_start: CapStyle,
    cap_end: CapStyle,
    caps: &CapRegistry,
) {
    let m = verts.len();
    let v0 = verts[0];
    let vend = verts[m - 1];
    builder.move_to((v0.nx.mul_add(v0.half, v0.x), v0.ny.mul_add(v0.half, v0.y)));
    for v in &verts[1..] {
        builder.line_to((v.nx.mul_add(v.half, v.x), v.ny.mul_add(v.half, v.y)));
    }
    caps.get(cap_end).emit(builder, vend, EndSide::End);
    for i in (0..m - 1).rev() {
        let v = verts[i];
        builder.line_to(((-v.nx).mul_add(v.half, v.x), (-v.ny).mul_add(v.half, v.y)));
    }
    caps.get(cap_start).emit(builder, v0, EndSide::Start);
    builder.close();
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

/// Centripetal Catmull-Rom, `α = 0.5`. Suppresses cusps on tight
/// loops and non-uniform sample spacing — the failure modes a
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

/// Uniform 1D Catmull-Rom on scalar knots. Wraps `catmull_rom_centripetal`
/// by embedding scalars as x-coordinates and reading the interpolated
/// x back; centripetal parameterisation degenerates to uniform when
/// consecutive knot differences match, and the endpoint-reflection
/// caller already keeps `h0..h3` well-conditioned.
fn catmull_rom_1d(h0: f32, h1: f32, h2: f32, h3: f32, t: f32) -> f32 {
    let (h, _) = catmull_rom_centripetal((h0, 0.0), (h1, 1.0), (h2, 2.0), (h3, 3.0), t);
    h
}

/// Binomial 5-tap low-pass over per-sample half-widths. Kernel
/// `[1, 4, 6, 4, 1] / 16` — kills isolated single-sample pressure
/// spikes without noticeably lagging genuine soft→hard→soft envelopes
/// (a 5-sample transition is still ~40ms at typical digitiser rates).
/// Endpoints reflect neighbours so the last visible width does not get
/// pulled toward 0 by an implicit zero-padded tap.
fn smoothed_halves(preset: &BrushPreset, points: &[InkPoint]) -> Vec<f32> {
    let n = points.len();
    let raw: Vec<f32> = points
        .iter()
        .map(|p| 0.5 * preset.width(p.pressure_f32(), p.tilt_f32()))
        .collect();
    if n < 3 {
        return raw;
    }
    let last = n - 1;
    let idx_lo = |i: usize, off: usize| -> usize { i.saturating_sub(off) };
    let idx_hi = |i: usize, off: usize| -> usize { (i + off).min(last) };
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let i0 = raw[idx_lo(i, 2)];
        let i1 = raw[idx_lo(i, 1)];
        let i2 = raw[i];
        let i3 = raw[idx_hi(i, 1)];
        let i4 = raw[idx_hi(i, 2)];
        let sum = 6.0f32.mul_add(i2, 4.0f32.mul_add(i1 + i3, i0 + i4));
        out.push(sum / 16.0);
    }
    out
}

#[allow(clippy::cast_precision_loss)]
const fn f32_from_usize(n: usize) -> f32 {
    n as f32
}

/// Subdivisions per Catmull-Rom segment. One step per ~6 world units
/// of chord length, clamped so tight-sample slow strokes still stay
/// visibly smooth and fast wide strokes don't blow up the vertex
/// count.
fn subdivision_count(a: (f32, f32), b: (f32, f32)) -> usize {
    let d = dist_sq(a, b).sqrt();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let raw = (d / 6.0).ceil() as usize;
    raw.clamp(6, 12)
}
