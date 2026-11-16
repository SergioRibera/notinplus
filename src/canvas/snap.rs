//! Holding the pen still at the end of a stroke snaps it to a figure.
//!
//! The pen counts as still once it stays within [`STILL_RADIUS`] screen
//! pixels for [`HOLD_STILL`] (the UI owns that timer, see
//! [`Board::set_hold_waker`]). The stroke then shows the fitted figure;
//! while the pen stays down, moving it turns and scales the figure about
//! its anchor, and lifting it keeps the figure. A short V drawn right
//! after a snapped line, near one of its ends, makes it an arrow.

use std::time::{Duration, Instant};

use freya_engine::prelude::{Canvas, Paint};
use notin_beautify::{
    Affine, Bounds, Pt, ShapeGeom, StrokeShape, arrow_head, fit_shape, snap_ends,
};

use super::Board;
use super::beautify::writes;
use crate::brush::{InkPoint, Stroke};
use crate::doc_op::DocOp;
use crate::ids::StrokeId;

/// How long the pen must rest for a stroke to snap.
pub const HOLD_STILL: Duration = Duration::from_millis(450);
/// Movement, in screen pixels, that still counts as resting.
const STILL_RADIUS: f32 = 2.0;
/// Ends of a new figure snap onto other figures this close, in screen
/// pixels.
const END_SNAP: f32 = 12.0;
/// An arrow head must follow its line within this.
const HEAD_WINDOW: Duration = Duration::from_secs(2);
/// How long the ink takes to morph into its figure.
const MORPH: Duration = Duration::from_millis(150);
/// Samples of the morph between ink and figure.
const MORPH_SAMPLES: usize = 96;

/// Board-side state of figure snapping.
#[derive(Debug, Default)]
pub(super) struct Snap {
    /// Where the pen came to rest, in world units.
    rest: Option<Pt>,
    /// Bumped whenever the pen moves off its rest point.
    generation: u64,
    waker: Option<flume::Sender<u64>>,
    /// The figure under the pen while it stays down.
    held: Option<Held>,
    /// The last line snapped, for a head drawn just after it.
    last_line: Option<(StrokeId, Instant)>,
    /// The ink of the stroke being morphed into its figure.
    morph: Option<(Vec<Pt>, Instant)>,
}

/// A figure held under the pen: turning and scaling follow the pen
/// about `pivot`.
#[derive(Debug, Clone)]
struct Held {
    base: ShapeGeom,
    pivot: Pt,
    grab: Pt,
}

/// What a figure turns and scales about: the start of a line, the
/// middle of a closed figure.
fn pivot(geom: &ShapeGeom) -> Pt {
    match geom {
        ShapeGeom::Line { a, .. } | ShapeGeom::Arrow { a, .. } => *a,
        ShapeGeom::Ellipse { center, .. } => *center,
        ShapeGeom::Polygon { points } => {
            Bounds::of(points).map_or_else(Pt::default, Bounds::center)
        }
    }
}

/// Points other figures offer to snap onto.
fn anchors(geom: &ShapeGeom) -> Vec<Pt> {
    match geom {
        ShapeGeom::Line { a, b } | ShapeGeom::Arrow { a, b, .. } => vec![*a, *b],
        ShapeGeom::Polygon { points } => points.clone(),
        ShapeGeom::Ellipse { center, .. } => vec![*center],
    }
}

/// `points` respaced to `n` samples along their path.
fn respace(points: &[Pt], n: usize) -> Vec<Pt> {
    let mut lengths = vec![0.0_f32];
    for w in points.windows(2) {
        lengths.push(lengths[lengths.len() - 1] + w[0].distance(w[1]));
    }
    let total = lengths[lengths.len() - 1].max(f32::EPSILON);
    let mut cursor = 0;
    (0..n)
        .map(|i| {
            #[expect(clippy::cast_precision_loss, reason = "a hundred samples")]
            let at = total * i as f32 / (n - 1).max(1) as f32;
            while cursor + 2 < lengths.len() && lengths[cursor + 1] < at {
                cursor += 1;
            }
            let span = (lengths[cursor + 1] - lengths[cursor]).max(f32::EPSILON);
            points[cursor].lerp(points[cursor + 1], (at - lengths[cursor]) / span)
        })
        .collect()
}

fn raw(stroke: &Stroke) -> Vec<Pt> {
    stroke.points.iter().map(|p| Pt::new(p.x, p.y)).collect()
}

impl Board {
    /// Channel the hold timer listens on. Each time the pen moves off its
    /// rest point the board sends a new generation; after [`HOLD_STILL`]
    /// with no newer one, the timer calls [`Self::hold_elapsed`].
    pub fn set_hold_waker(&mut self, waker: flume::Sender<u64>) {
        self.snap.waker = Some(waker);
    }

    /// Whether the stroke under the pen can snap to a figure.
    const fn can_snap(&self) -> bool {
        self.beautify_options().shapes
            && !self.two_point_active
            && !self.read_only
            && writes(self.current_kind())
    }

    /// A pen sample of the stroke being drawn: re-arm the hold timer when
    /// the pen moved, or follow the pen with a held figure. `true` when
    /// the sample was taken by a held figure (the ink stays as drawn).
    pub(super) fn snap_track(&mut self, point: InkPoint) -> bool {
        let pen = Pt::new(point.x, point.y);
        if let Some(held) = &self.snap.held {
            let affine = follow(held, pen);
            let geom = held.base.transformed(&affine);
            if let Some(shape) = self.active.as_mut().and_then(|a| a.shape.as_mut()) {
                shape.geom = geom;
            }
            return true;
        }
        if !self.can_snap() {
            return false;
        }
        let scale = self.viewport.scale.max(f32::EPSILON);
        let moved = self
            .snap
            .rest
            .is_none_or(|rest| rest.distance(pen) * scale > STILL_RADIUS);
        if moved {
            self.snap.rest = Some(pen);
            self.snap.generation += 1;
            if let Some(waker) = &self.snap.waker {
                let _ = waker.send(self.snap.generation);
            }
        }
        false
    }

    /// The pen rested: snap the stroke under it to a figure, if it is
    /// one and the rest is still the one armed as `generation`.
    #[sismo::function("snap::hold_elapsed")]
    pub fn hold_elapsed(&mut self, generation: u64) {
        if generation != self.snap.generation || self.snap.held.is_some() || !self.can_snap() {
            return;
        }
        let Some(active) = &self.active else {
            return;
        };
        let ink = raw(active);
        let Some(geom) = fit_shape(&ink, false) else {
            return;
        };
        let radius = END_SNAP / self.viewport.scale.max(f32::EPSILON);
        let geom = snap_ends(geom, &self.figure_anchors(), radius);
        let with_pressure: Vec<(Pt, u8)> = active
            .points
            .iter()
            .map(|p| (Pt::new(p.x, p.y), p.pressure))
            .collect();
        let shape = StrokeShape::from_ink(geom.clone(), &with_pressure);
        self.snap.held = Some(Held {
            pivot: pivot(&geom),
            grab: ink.last().copied().unwrap_or_default(),
            base: geom,
        });
        if self.beautify_options().animate {
            self.snap.morph = Some((ink, Instant::now()));
        }
        if let Some(active) = self.active.as_mut() {
            active.shape = Some(shape);
        }
        self.notify();
    }

    /// Ends and corners of the figures on the active layer.
    fn figure_anchors(&self) -> Vec<Pt> {
        self.doc
            .layer(self.doc.active_layer)
            .into_iter()
            .flat_map(|layer| layer.strokes.iter())
            .filter_map(|s| s.shape.as_ref())
            .flat_map(|s| anchors(&s.geom))
            .collect()
    }

    /// Forget the rest point and any held figure (pen down, cancel).
    pub(super) fn snap_reset(&mut self) {
        self.snap.rest = None;
        self.snap.held = None;
        self.snap.morph = None;
        self.snap.generation += 1;
    }

    /// At pen up: the figure the stroke snapped to by holding, taken off
    /// the stroke so it is committed raw and the figure lands as its own
    /// undo step.
    pub(super) fn take_held_figure(&mut self, active: &mut Stroke) -> Option<StrokeShape> {
        self.snap.held.take()?;
        active.shape.take()
    }

    /// After a held figure lands on stroke `id`.
    pub(super) fn figure_landed(&mut self, id: StrokeId, shape: StrokeShape) {
        let line = matches!(shape.geom, ShapeGeom::Line { .. });
        self.apply_op(DocOp::SetStrokeShape {
            id,
            shape: Some(shape),
        });
        self.snap.last_line = line.then(|| (id, Instant::now()));
    }

    /// Whether `stroke`, just drawn, is a head for the line snapped a
    /// moment ago; if so the line becomes an arrow and `true` comes back
    /// (the head itself is not kept).
    pub(super) fn arrow_head_for_last_line(&mut self, stroke: &Stroke) -> bool {
        let Some((line_id, at)) = self.snap.last_line.take() else {
            return false;
        };
        if at.elapsed() > HEAD_WINDOW || !self.can_snap() {
            return false;
        }
        let Some(mut shape) = self.doc.find_stroke(line_id).and_then(|s| s.shape.clone()) else {
            return false;
        };
        let ShapeGeom::Line { a, b } = shape.geom else {
            return false;
        };
        let Some(heads) = arrow_head(a, b, &raw(stroke)) else {
            self.snap.last_line = Some((line_id, at));
            return false;
        };
        shape.geom = ShapeGeom::Arrow { a, b, heads };
        self.apply_op(DocOp::SetStrokeShape {
            id: line_id,
            shape: Some(shape),
        });
        true
    }

    /// Paint the stroke under the pen part way through its morph into a
    /// figure. `false` when it is not morphing (paint it as usual).
    pub(super) fn paint_morph(&self, canvas: &Canvas, active: &Stroke, paint: &Paint) -> bool {
        let Some((ink, start)) = &self.snap.morph else {
            return false;
        };
        let t = start.elapsed().as_secs_f32() / MORPH.as_secs_f32();
        let Some(shape) = active.shape.as_ref().filter(|_| t < 1.0) else {
            return false;
        };
        let Some(preset) = self.doc.preset(active.brush) else {
            return false;
        };
        let eased = 1.0 - (1.0 - t).powi(3);
        let target = shape.geom.trace(4.0);
        let (from, to) = (respace(ink, MORPH_SAMPLES), respace(&target, MORPH_SAMPLES));
        let points: Vec<InkPoint> = from
            .iter()
            .zip(&to)
            .map(|(a, b)| {
                let p = a.lerp(*b, eased);
                InkPoint::new(p.x, p.y, 128, 0, 0)
            })
            .collect();
        let shown = Stroke {
            points,
            shape: None,
            fix: None,
            ..active.clone()
        };
        canvas.draw_path(&self.brush_registry.stroke_path(preset, &shown), paint);
        if let Some(notifier) = &self.notifier {
            notifier.ping();
        }
        true
    }
}

/// The turn and scale about the held figure's pivot that carries its
/// grab point to `pen`.
fn follow(held: &Held, pen: Pt) -> Affine {
    let (gx, gy) = (held.grab.x - held.pivot.x, held.grab.y - held.pivot.y);
    let (px, py) = (pen.x - held.pivot.x, pen.y - held.pivot.y);
    let from = gx.hypot(gy);
    if from <= f32::EPSILON {
        return Affine::IDENTITY;
    }
    let scale = px.hypot(py) / from;
    let angle = py.atan2(px) - gy.atan2(gx);
    Affine::translation(-held.pivot.x, -held.pivot.y)
        .then(&Affine::rotation(angle))
        .then(&Affine::scale(scale))
        .then(&Affine::translation(held.pivot.x, held.pivot.y))
}

#[cfg(test)]
mod tests {
    use crate::brush::{BrushPreset, InkPoint};
    use crate::canvas::Board;
    use notin_beautify::{Heads, ShapeGeom};

    fn pt(x: f32, y: f32) -> InkPoint {
        InkPoint::new(x, y, 140, 0, 0)
    }

    /// Draw through `points` and hold at the end (timer fired).
    fn draw_and_hold(board: &mut Board, points: &[(f32, f32)]) {
        board.begin(pt(points[0].0, points[0].1));
        for &(x, y) in &points[1..] {
            board.extend(pt(x, y));
        }
        board.hold_elapsed(board.snap.generation);
    }

    fn shaky_line(from: (f32, f32), to: (f32, f32)) -> Vec<(f32, f32)> {
        (0..=40u8)
            .map(|i| {
                let t = f32::from(i) / 40.0;
                let wobble = (f32::from(i) * 1.7).sin();
                (
                    (to.0 - from.0).mul_add(t, from.0),
                    (to.1 - from.1).mul_add(t, from.1) + wobble,
                )
            })
            .collect()
    }

    fn board() -> Board {
        let mut board = Board::default();
        board.set_current_preset(BrushPreset::pen());
        board
    }

    #[test]
    fn holding_snaps_and_undo_brings_the_ink_back() {
        let mut board = board();
        draw_and_hold(&mut board, &shaky_line((0.0, 100.0), (300.0, 103.0)));
        let depth = board.undo_depth();
        board.end();
        let stroke = board.doc().layer(0).unwrap().strokes[0].clone();
        assert!(matches!(
            stroke.shape.as_ref().map(|s| &s.geom),
            Some(ShapeGeom::Line { .. })
        ));
        assert!(
            stroke
                .shape
                .as_ref()
                .is_some_and(|s| !s.pressure.is_empty())
        );
        assert_eq!(board.undo_depth(), depth + 2);
        assert!(board.undo());
        let stroke = &board.doc().layer(0).unwrap().strokes[0];
        assert!(stroke.shape.is_none(), "the raw ink stays");
    }

    #[test]
    fn moving_while_held_turns_and_scales() {
        let mut board = board();
        draw_and_hold(&mut board, &shaky_line((0.0, 100.0), (300.0, 100.0)));
        board.extend(pt(0.0, 400.0));
        board.end();
        let stroke = &board.doc().layer(0).unwrap().strokes[0];
        let Some(ShapeGeom::Line { b, .. }) = stroke.shape.as_ref().map(|s| s.geom.clone()) else {
            panic!();
        };
        assert!(
            (b.x - 0.0).abs() < 1.0 && (b.y - 400.0).abs() < 1.0,
            "{b:?}"
        );
    }

    #[test]
    fn without_holding_nothing_snaps() {
        let mut board = board();
        for (x, y) in shaky_line((0.0, 100.0), (300.0, 103.0)) {
            if board.active.is_none() {
                board.begin(pt(x, y));
            } else {
                board.extend(pt(x, y));
            }
        }
        let stale = board.snap.generation.wrapping_sub(1);
        board.hold_elapsed(stale);
        board.end();
        assert!(board.doc().layer(0).unwrap().strokes[0].shape.is_none());
    }

    #[test]
    fn a_v_after_a_line_makes_an_arrow() {
        let mut board = board();
        draw_and_hold(&mut board, &shaky_line((0.0, 100.0), (300.0, 100.0)));
        board.end();
        board.begin(pt(275.0, 80.0));
        for i in 1..=10u8 {
            let t = f32::from(i) / 10.0;
            board.extend(pt(25.0f32.mul_add(t, 275.0), 20.0f32.mul_add(t, 80.0)));
        }
        for i in 1..=10u8 {
            let t = f32::from(i) / 10.0;
            board.extend(pt((-25.0f32).mul_add(t, 300.0), 20.0f32.mul_add(t, 100.0)));
        }
        board.end();
        let strokes = &board.doc().layer(0).unwrap().strokes;
        assert_eq!(strokes.len(), 1, "the head is not kept as ink");
        assert!(matches!(
            strokes[0].shape.as_ref().map(|s| &s.geom),
            Some(ShapeGeom::Arrow {
                heads: Heads::End,
                ..
            })
        ));
    }
}
