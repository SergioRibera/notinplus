//! Clean figures a stroke snaps to, and how they are traced back into
//! polylines for the stroke's own brush.

use std::f32::consts::{PI, TAU};

use wincode::{SchemaRead, SchemaWrite};

use crate::{Affine, Pt};

/// Pressure samples kept per figure, spread by arc length.
const PRESSURE_SAMPLES: usize = 32;
/// Half-angle between an arrow shaft and each side of its head.
const HEAD_ANGLE: f32 = 28.0 * PI / 180.0;

/// Which ends of an arrow carry a head.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, SchemaWrite, SchemaRead)]
pub enum Heads {
    Start,
    #[default]
    End,
    Both,
}

impl Heads {
    const fn at_start(self) -> bool {
        matches!(self, Self::Start | Self::Both)
    }

    const fn at_end(self) -> bool {
        matches!(self, Self::End | Self::Both)
    }
}

/// A clean figure.
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub enum ShapeGeom {
    Line {
        a: Pt,
        b: Pt,
    },
    Arrow {
        a: Pt,
        b: Pt,
        heads: Heads,
    },
    /// Closed, 3 to 8 corners: triangle, rectangle (maybe rotated), …
    Polygon {
        points: Vec<Pt>,
    },
    Ellipse {
        center: Pt,
        rx: f32,
        ry: f32,
        /// Radians, of the `rx` axis from +x.
        rotation: f32,
    },
}

impl ShapeGeom {
    /// The figure moved by `(dx, dy)`.
    #[must_use]
    pub fn translated(&self, dx: f32, dy: f32) -> Self {
        self.transformed(&Affine::translation(dx, dy))
    }

    /// The figure through `affine`. Exact for lines and polygons; an
    /// ellipse keeps its shape and takes the affine's rotation and mean
    /// scale, which is exact for moves, turns and uniform scaling.
    #[must_use]
    pub fn transformed(&self, affine: &Affine) -> Self {
        match self {
            Self::Line { a, b } => Self::Line {
                a: affine.apply(*a),
                b: affine.apply(*b),
            },
            Self::Arrow { a, b, heads } => Self::Arrow {
                a: affine.apply(*a),
                b: affine.apply(*b),
                heads: *heads,
            },
            Self::Polygon { points } => Self::Polygon {
                points: points.iter().map(|p| affine.apply(*p)).collect(),
            },
            Self::Ellipse {
                center,
                rx,
                ry,
                rotation,
            } => {
                let scale = affine
                    .a
                    .mul_add(affine.d, -(affine.b * affine.c))
                    .abs()
                    .sqrt();
                Self::Ellipse {
                    center: affine.apply(*center),
                    rx: rx * scale,
                    ry: ry * scale,
                    rotation: rotation + affine.b.atan2(affine.a),
                }
            }
        }
    }

    /// The outline as one polyline, a sample at most `step` apart.
    /// Corners are kept exactly; closed figures end where they start.
    #[must_use]
    pub fn trace(&self, step: f32) -> Vec<Pt> {
        let step = step.max(0.5);
        match self {
            Self::Line { a, b } | Self::Arrow { a, b, .. } => along(&[*a, *b], step),
            Self::Polygon { points } => {
                let mut ring = points.clone();
                if let Some(&first) = points.first() {
                    ring.push(first);
                }
                along(&ring, step)
            }
            Self::Ellipse {
                center,
                rx,
                ry,
                rotation,
            } => {
                // Ramanujan's perimeter, enough to pick a sample count.
                let eccentric = ((rx - ry) / (rx + ry).max(f32::EPSILON)).powi(2);
                let perimeter = PI
                    * (rx + ry)
                    * (1.0 + 3.0 * eccentric / (10.0 + 3.0f32.mul_add(-eccentric, 4.0).sqrt()));
                #[expect(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    reason = "clamped to a small positive count"
                )]
                let count = (perimeter / step).ceil().clamp(24.0, 512.0) as u16;
                let (sin, cos) = rotation.sin_cos();
                (0..=count)
                    .map(|i| {
                        let angle = TAU * f32::from(i) / f32::from(count);
                        let (x, y) = (rx * angle.cos(), ry * angle.sin());
                        Pt::new(
                            x.mul_add(cos, -y * sin) + center.x,
                            x.mul_add(sin, y * cos) + center.y,
                        )
                    })
                    .collect()
            }
        }
    }

    /// Arrow heads as `wing → tip → wing` polylines, `size` long.
    #[must_use]
    pub fn heads(&self, size: f32) -> Vec<[Pt; 3]> {
        let Self::Arrow { a, b, heads } = self else {
            return Vec::new();
        };
        let size = size.min(a.distance(*b) * 0.4);
        let head = |from: Pt, tip: Pt| {
            let angle = (from.y - tip.y).atan2(from.x - tip.x);
            let wing = |turn: f32| {
                let (sin, cos) = (angle + turn).sin_cos();
                Pt::new(size.mul_add(cos, tip.x), size.mul_add(sin, tip.y))
            };
            [wing(HEAD_ANGLE), tip, wing(-HEAD_ANGLE)]
        };
        let mut out = Vec::new();
        if heads.at_start() {
            out.push(head(*b, *a));
        }
        if heads.at_end() {
            out.push(head(*a, *b));
        }
        out
    }
}

/// Distance along `points` at every sample, starting at 0.
fn arc_lengths(points: &[Pt]) -> Vec<f32> {
    let mut total = 0.0;
    let mut out = Vec::with_capacity(points.len());
    let mut prev = points.first().copied();
    for &p in points {
        total += prev.map_or(0.0, |q| q.distance(p));
        out.push(total);
        prev = Some(p);
    }
    out
}

/// The polyline through `corners`, subdivided so no gap exceeds `step`.
fn along(corners: &[Pt], step: f32) -> Vec<Pt> {
    let mut out = Vec::new();
    for pair in corners.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to a small positive count"
        )]
        let n = (from.distance(to) / step).ceil().clamp(1.0, 4096.0) as u16;
        out.extend((0..n).map(|i| from.lerp(to, f32::from(i) / f32::from(n))));
    }
    out.extend(corners.last());
    out
}

/// A figure on a stroke, with the pressure of the ink it came from.
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct StrokeShape {
    pub geom: ShapeGeom,
    /// Pressure along the outline, evenly spread by arc length. Empty →
    /// constant (figures made with the shape tool have no raw ink).
    pub pressure: Vec<u8>,
    /// Drawn as dashes.
    pub dashed: bool,
}

impl StrokeShape {
    /// A figure without pressure, as the shape tool makes them.
    #[must_use]
    pub const fn plain(geom: ShapeGeom, dashed: bool) -> Self {
        Self {
            geom,
            pressure: Vec::new(),
            dashed,
        }
    }

    /// A figure keeping the feel of `raw` ink: its pressure, resampled by
    /// arc length, is laid along the outline.
    #[must_use]
    pub fn from_ink(geom: ShapeGeom, raw: &[(Pt, u8)]) -> Self {
        let points: Vec<Pt> = raw.iter().map(|(p, _)| *p).collect();
        let lengths = arc_lengths(&points);
        let total = lengths.last().copied().unwrap_or(0.0);
        let pressure = if total <= f32::EPSILON {
            raw.iter().map(|(_, p)| *p).take(1).collect()
        } else {
            let mut cursor = 0;
            (0..PRESSURE_SAMPLES)
                .map(|i| {
                    #[expect(clippy::cast_precision_loss, reason = "a few dozen samples")]
                    let at = total * i as f32 / (PRESSURE_SAMPLES - 1) as f32;
                    while cursor + 1 < lengths.len() && lengths[cursor + 1] < at {
                        cursor += 1;
                    }
                    raw.get(cursor).map_or(0, |(_, p)| *p)
                })
                .collect()
        };
        Self {
            geom,
            pressure,
            dashed: false,
        }
    }

    /// The pressure `t` (0..=1) of the way along the outline.
    #[must_use]
    pub fn pressure_at(&self, t: f32) -> Option<u8> {
        let last = self.pressure.len().checked_sub(1)?;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "index into a short array, clamped"
        )]
        let index = (t.clamp(0.0, 1.0) * last as f32).round() as usize;
        self.pressure.get(index.min(last)).copied()
    }

    /// Polylines to draw with the stroke's brush, each sample paired with
    /// how far along the outline it sits (0..=1, for [`Self::pressure_at`]).
    /// `step` bounds the sample gap and `dash` is `(on, off)` lengths,
    /// used when [`Self::dashed`]. Arrow heads grow with the shaft.
    #[must_use]
    pub fn polylines(&self, step: f32, dash: (f32, f32)) -> Vec<Vec<(Pt, f32)>> {
        let trace = self.geom.trace(step);
        let lengths = arc_lengths(&trace);
        let total = lengths.last().copied().unwrap_or(0.0).max(f32::EPSILON);
        let head = (0.2 * total).clamp(8.0, 28.0);
        let main: Vec<(Pt, f32)> = trace
            .into_iter()
            .zip(lengths.iter().map(|l| l / total))
            .collect();
        let mut out = if self.dashed {
            dashes(&main, &lengths, dash)
        } else {
            vec![main]
        };
        // `heads` lists the start head before the end one.
        let ends = match self.geom {
            ShapeGeom::Arrow {
                heads: Heads::End, ..
            } => [1.0, 1.0],
            _ => [0.0, 1.0],
        };
        for ([w1, tip, w2], t) in self.geom.heads(head).into_iter().zip(ends) {
            out.push(vec![(w1, t), (tip, t), (w2, t)]);
        }
        out
    }
}

/// `line` cut into `(on, off)` dashes by arc length.
fn dashes(line: &[(Pt, f32)], lengths: &[f32], (on, off): (f32, f32)) -> Vec<Vec<(Pt, f32)>> {
    let period = (on + off).max(1.0);
    let on = on.max(0.5);
    let mut out: Vec<Vec<(Pt, f32)>> = Vec::new();
    let mut current: Vec<(Pt, f32)> = Vec::new();
    for (i, pair) in line.windows(2).enumerate() {
        let (from, to) = (pair[0], pair[1]);
        let (start, end) = (lengths[i], lengths[i + 1]);
        let span = (end - start).max(f32::EPSILON);
        // Boundaries where the pattern flips inside this segment.
        let mut cuts = vec![start];
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "period index along a finite outline"
        )]
        let (first, last) = (
            (start / period).floor() as u32,
            (end / period).floor() as u32,
        );
        for index in first..=last {
            #[expect(clippy::cast_precision_loss, reason = "period index")]
            let base = index as f32 * period;
            for edge in [base, base + on] {
                if edge > start && edge < end {
                    cuts.push(edge);
                }
            }
        }
        cuts.push(end);
        for w in cuts.windows(2) {
            let mid = f32::midpoint(w[0], w[1]);
            let drawing = mid.rem_euclid(period) < on;
            let at = |l: f32| {
                let t = (l - start) / span;
                (from.0.lerp(to.0, t), (to.1 - from.1).mul_add(t, from.1))
            };
            if drawing {
                if current.is_empty() {
                    current.push(at(w[0]));
                }
                current.push(at(w[1]));
            } else if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        }
    }
    if current.len() > 1 {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polygon_trace_closes_and_keeps_corners() {
        let square = ShapeGeom::Polygon {
            points: vec![
                Pt::new(0.0, 0.0),
                Pt::new(10.0, 0.0),
                Pt::new(10.0, 10.0),
                Pt::new(0.0, 10.0),
            ],
        };
        let trace = square.trace(3.0);
        assert_eq!(trace.first(), trace.last());
        for corner in [Pt::new(10.0, 0.0), Pt::new(10.0, 10.0), Pt::new(0.0, 10.0)] {
            assert!(trace.contains(&corner));
        }
        assert!(trace.windows(2).all(|w| w[0].distance(w[1]) <= 3.0 + 1e-4));
    }

    #[test]
    fn ellipse_trace_stays_on_the_ellipse() {
        let e = ShapeGeom::Ellipse {
            center: Pt::new(5.0, 5.0),
            rx: 20.0,
            ry: 10.0,
            rotation: 0.3,
        };
        let (sin, cos) = 0.3f32.sin_cos();
        for p in e.trace(2.0) {
            let (dx, dy) = (p.x - 5.0, p.y - 5.0);
            let (u, v) = (dx.mul_add(cos, dy * sin), (-dx).mul_add(sin, dy * cos));
            let r = (u / 20.0).mul_add(u / 20.0, (v / 10.0).powi(2));
            assert!((r - 1.0).abs() < 1e-3, "{r}");
        }
    }

    #[test]
    fn arrow_heads_point_at_the_tips() {
        let arrow = ShapeGeom::Arrow {
            a: Pt::new(0.0, 0.0),
            b: Pt::new(100.0, 0.0),
            heads: Heads::Both,
        };
        let heads = arrow.heads(10.0);
        assert_eq!(heads.len(), 2);
        assert_eq!(heads[0][1], Pt::new(0.0, 0.0));
        assert!(heads[0][0].x > 0.0);
        assert_eq!(heads[1][1], Pt::new(100.0, 0.0));
        assert!(heads[1][0].x < 100.0);
    }

    #[test]
    fn pressure_follows_the_ink() {
        let raw: Vec<(Pt, u8)> = (0..=100u8)
            .map(|i| (Pt::new(f32::from(i), 0.0), i * 2))
            .collect();
        let line = ShapeGeom::Line {
            a: Pt::new(0.0, 0.0),
            b: Pt::new(100.0, 0.0),
        };
        let shape = StrokeShape::from_ink(line, &raw);
        assert_eq!(shape.pressure.len(), PRESSURE_SAMPLES);
        assert!(shape.pressure_at(0.0) < Some(10));
        assert!(shape.pressure_at(1.0) > Some(190));
        assert_eq!(StrokeShape::plain(shape.geom, false).pressure_at(0.5), None);
    }

    #[test]
    fn dashes_alternate() {
        let line = ShapeGeom::Line {
            a: Pt::new(0.0, 0.0),
            b: Pt::new(100.0, 0.0),
        };
        let polylines = StrokeShape::plain(line, true).polylines(2.0, (10.0, 6.0));
        assert_eq!(polylines.len(), 7);
        let first = &polylines[0];
        assert!((first.last().map_or(0.0, |(p, _)| p.x) - 10.0).abs() < 1e-3);
    }

    #[test]
    fn translate_moves_every_figure() {
        let e = ShapeGeom::Ellipse {
            center: Pt::new(1.0, 1.0),
            rx: 3.0,
            ry: 2.0,
            rotation: 0.0,
        };
        assert_eq!(
            e.translated(2.0, 3.0),
            ShapeGeom::Ellipse {
                center: Pt::new(3.0, 4.0),
                rx: 3.0,
                ry: 2.0,
                rotation: 0.0,
            }
        );
    }
}
