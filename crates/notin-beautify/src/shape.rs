//! Recognizing a drawn stroke as a figure and fitting it.
//!
//! 1. Resample at a fixed spacing.
//! 2. Find corners (`ShortStraw`, with the `IStraw` check that drops
//!    "corners" on gentle curves).
//! 3. Closed or open, by the gap between the ends.
//! 4. Fit every candidate figure and measure its error against the ink.
//! 5. The best fit under its threshold wins; then angles near 0/45/90°
//!    snap.

#![expect(
    clippy::many_single_char_names,
    reason = "geometry reads closest to its formulas with short names"
)]

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

use crate::metrics::{Bounds, median};
use crate::{Heads, Pt, ShapeGeom};

/// Samples along a stroke once resampled.
const SAMPLES: f32 = 64.0;
/// Half-window of the `ShortStraw` straws.
const STRAW: usize = 3;
/// Ends closer than this share of the length make a closed stroke.
const CLOSED_GAP: f32 = 0.15;
/// Chord over path above which a stretch is a straight line.
const STRAIGHT: f32 = 0.95;
/// Turns gentler than this (radians) are curve, not corner.
const MIN_TURN: f32 = 35.0 * PI / 180.0;
/// Angles within this of a multiple of 45° snap onto it.
const ANGLE_SNAP: f32 = 6.0 * PI / 180.0;
/// Rectangle corners may be this far from square.
const SQUARE_CORNER: f32 = 15.0 * PI / 180.0;
/// Sides (or radii) this close count as equal.
const EQUAL: f32 = 0.1;

/// Largest fitting error (mean distance over the figure's size) each
/// figure may have when the user asked for a figure by holding.
const LINE_ERROR: f32 = 0.035;
const POLYGON_ERROR: f32 = 0.05;
const ELLIPSE_ERROR: f32 = 0.09;
/// Thresholds shrink by this when nobody asked (beautifying a selection).
const STRICT: f32 = 0.6;

/// The figure `points` were drawn as, if any. `strict` raises the bar,
/// for ink beautified after the fact rather than held on purpose.
#[must_use]
#[cfg_attr(feature = "sismo", sismo::function("beautify::fit_shape"))]
pub fn fit_shape(points: &[Pt], strict: bool) -> Option<ShapeGeom> {
    let length = path_length(points);
    if points.len() < 2 || length < 4.0 {
        return None;
    }
    let pts = resample(points, length / SAMPLES);
    let corners = corners(&pts);
    let closed = pts[0].distance(pts[pts.len() - 1]) < CLOSED_GAP * length;
    let bar = if strict { STRICT } else { 1.0 };
    let mut best: Option<(f32, ShapeGeom)> = None;
    let mut offer = |score: f32, geom: ShapeGeom| {
        if score <= 1.0 && best.as_ref().is_none_or(|(s, _)| score < *s) {
            best = Some((score, geom));
        }
    };
    if closed {
        if let Some((err, geom)) = fit_polygon(&pts, &corners) {
            offer(err / (POLYGON_ERROR * bar), geom);
        }
        if let Some((err, geom)) = fit_ellipse(&pts) {
            offer(err / (ELLIPSE_ERROR * bar), geom);
        }
    } else {
        if let Some((err, geom)) = fit_line(&pts) {
            offer(err / (LINE_ERROR * bar), geom);
        }
        if let Some((err, geom)) = fit_arrow(&pts, &corners) {
            offer(err / (LINE_ERROR * bar), geom);
        }
    }
    best.map(|(_, geom)| snap_angles(geom))
}

/// Which end of the line `a`–`b` a short stroke `head`, drawn just after
/// it, adds an arrow head to.
#[must_use]
pub fn arrow_head(a: Pt, b: Pt, head: &[Pt]) -> Option<Heads> {
    let shaft = a.distance(b);
    let bounds = Bounds::of(head)?;
    if shaft < 8.0 || bounds.width().max(bounds.height()) > 0.5 * shaft {
        return None;
    }
    let length = path_length(head);
    let pts = resample(head, length / 24.0);
    // A V: one corner, two arms of similar length.
    let corners = corners(&pts);
    let [_, tip, _] = corners[..] else {
        return None;
    };
    let tip = pts[tip];
    let reach = 0.25 * shaft;
    if tip.distance(b) <= reach {
        Some(Heads::End)
    } else if tip.distance(a) <= reach {
        Some(Heads::Start)
    } else {
        None
    }
}

/// `geom` with the ends of lines and arrows and the corners of polygons
/// moved onto any of `targets` within `radius`.
#[must_use]
pub fn snap_ends(geom: ShapeGeom, targets: &[Pt], radius: f32) -> ShapeGeom {
    let snap = |p: Pt| {
        targets
            .iter()
            .filter(|t| t.distance(p) <= radius)
            .min_by(|x, y| x.distance(p).total_cmp(&y.distance(p)))
            .copied()
            .unwrap_or(p)
    };
    match geom {
        ShapeGeom::Line { a, b } => ShapeGeom::Line {
            a: snap(a),
            b: snap(b),
        },
        ShapeGeom::Arrow { a, b, heads } => ShapeGeom::Arrow {
            a: snap(a),
            b: snap(b),
            heads,
        },
        other @ (ShapeGeom::Polygon { .. } | ShapeGeom::Ellipse { .. }) => other,
    }
}

fn path_length(points: &[Pt]) -> f32 {
    points.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// `points` resampled every `step` along the path, ends kept.
fn resample(points: &[Pt], step: f32) -> Vec<Pt> {
    let step = step.max(f32::EPSILON);
    let mut out = vec![points[0]];
    let mut carried = 0.0;
    for pair in points.windows(2) {
        let (mut from, to) = (pair[0], pair[1]);
        let mut d = from.distance(to);
        // Each pass moves `from` towards `to`, so `d` shrinks to the end.
        loop {
            if carried + d < step {
                break;
            }
            let t = (step - carried) / d;
            let p = from.lerp(to, t);
            out.push(p);
            from = p;
            d = from.distance(to);
            carried = 0.0;
        }
        carried += d;
    }
    if out
        .last()
        .is_some_and(|p| p.distance(points[points.len() - 1]) > f32::EPSILON)
    {
        out.push(points[points.len() - 1]);
    }
    out
}

/// Turn at `i` measured `k` samples either side, radians (0 = straight).
fn turn(pts: &[Pt], i: usize, k: usize) -> f32 {
    let (a, b) = (pts[i.saturating_sub(k)], pts[(i + k).min(pts.len() - 1)]);
    let p = pts[i];
    let (u, v) = ((p.x - a.x, p.y - a.y), (b.x - p.x, b.y - p.y));
    let cross = u.0.mul_add(v.1, -(u.1 * v.0));
    let dot = u.0.mul_add(v.0, u.1 * v.1);
    cross.atan2(dot).abs()
}

fn is_line(pts: &[Pt], from: usize, to: usize) -> bool {
    let path = path_length(&pts[from..=to]);
    path <= f32::EPSILON || pts[from].distance(pts[to]) / path > STRAIGHT
}

/// Corner indices, ends included (`ShortStraw` + `IStraw` curve check).
fn corners(pts: &[Pt]) -> Vec<usize> {
    let n = pts.len();
    let mut corners = vec![0];
    if n > 2 * STRAW + 1 {
        let straws: Vec<f32> = (STRAW..n - STRAW)
            .map(|i| pts[i - STRAW].distance(pts[i + STRAW]))
            .collect();
        let threshold = median(&mut straws.clone()).unwrap_or(0.0) * 0.95;
        let mut i = 0;
        while i < straws.len() {
            if straws[i] < threshold {
                let start = i;
                while i < straws.len() && straws[i] < threshold {
                    i += 1;
                }
                let lowest = (start..i)
                    .min_by(|a, b| straws[*a].total_cmp(&straws[*b]))
                    .unwrap_or(start);
                corners.push(lowest + STRAW);
            }
            i += 1;
        }
    }
    corners.push(n - 1);
    // A stretch between corners that is not straight hides a corner.
    let mut k = 1;
    while k < corners.len() {
        let (a, b) = (corners[k - 1], corners[k]);
        if b - a > 2 * STRAW && !is_line(pts, a, b) {
            let mid = usize::midpoint(a, b);
            let split = (a + STRAW..=b - STRAW)
                .max_by(|x, y| turn(pts, *x, STRAW).total_cmp(&turn(pts, *y, STRAW)))
                .unwrap_or(mid);
            if turn(pts, split, STRAW) >= MIN_TURN && split != a && split != b {
                corners.insert(k, split);
                continue;
            }
        }
        k += 1;
    }
    // Drop corners on a straight run or on a gentle curve.
    let mut k = 1;
    while k + 1 < corners.len() {
        let (a, c, b) = (corners[k - 1], corners[k], corners[k + 1]);
        if is_line(pts, a, b) || turn(pts, c, 2 * STRAW) < MIN_TURN {
            corners.remove(k);
        } else {
            k += 1;
        }
    }
    corners
}

/// Mean distance from `p` to the segment `a`–`b`.
fn segment_distance(p: Pt, a: Pt, b: Pt) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx.mul_add(dx, dy * dy);
    if len2 <= f32::EPSILON {
        return p.distance(a);
    }
    let t = ((p.x - a.x).mul_add(dx, (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.distance(a.lerp(b, t))
}

/// Mean distance of `pts` to the closest of the polyline's segments,
/// over the size of `pts`.
fn polyline_error(pts: &[Pt], corners: &[Pt]) -> Option<f32> {
    let size = Bounds::of(pts).map(|b| b.width().hypot(b.height()))?;
    if size <= f32::EPSILON {
        return None;
    }
    #[expect(clippy::cast_precision_loss, reason = "a few dozen samples")]
    let mean = pts
        .iter()
        .map(|p| {
            corners
                .windows(2)
                .map(|w| segment_distance(*p, w[0], w[1]))
                .fold(f32::INFINITY, f32::min)
        })
        .sum::<f32>()
        / pts.len() as f32;
    Some(mean / size)
}

fn fit_line(pts: &[Pt]) -> Option<(f32, ShapeGeom)> {
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    if !is_line(pts, 0, pts.len() - 1) {
        return None;
    }
    Some((polyline_error(pts, &[a, b])?, ShapeGeom::Line { a, b }))
}

/// A shaft and a head drawn in one go at its end.
fn fit_arrow(pts: &[Pt], corners: &[usize]) -> Option<(f32, ShapeGeom)> {
    let &[0, joint, ..] = corners else {
        return None;
    };
    if corners.len() < 3 || !is_line(pts, 0, joint) {
        return None;
    }
    let (a, b) = (pts[0], pts[joint]);
    let shaft = a.distance(b);
    let head = &pts[joint..];
    if path_length(head) > 1.2 * shaft || head.iter().any(|p| p.distance(b) > 0.4 * shaft) {
        return None;
    }
    let error = polyline_error(&pts[..=joint], &[a, b])?;
    Some((
        error,
        ShapeGeom::Arrow {
            a,
            b,
            heads: Heads::End,
        },
    ))
}

/// Least-squares line through `pts`: a point on it and its direction.
fn fit_direction(pts: &[Pt]) -> Option<(Pt, (f32, f32))> {
    if pts.is_empty() {
        return None;
    }
    #[expect(clippy::cast_precision_loss, reason = "a few dozen samples")]
    let n = pts.len() as f32;
    let cx = pts.iter().map(|p| p.x).sum::<f32>() / n;
    let cy = pts.iter().map(|p| p.y).sum::<f32>() / n;
    let (mut sxx, mut syy, mut sxy) = (0.0_f32, 0.0_f32, 0.0_f32);
    for p in pts {
        let (dx, dy) = (p.x - cx, p.y - cy);
        sxx = dx.mul_add(dx, sxx);
        syy = dy.mul_add(dy, syy);
        sxy = dx.mul_add(dy, sxy);
    }
    let angle = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    Some((Pt::new(cx, cy), (angle.cos(), angle.sin())))
}

fn intersect((p, d): (Pt, (f32, f32)), (q, e): (Pt, (f32, f32))) -> Option<Pt> {
    let cross = d.0.mul_add(e.1, -(d.1 * e.0));
    if cross.abs() < 1e-3 {
        return None;
    }
    let t = (q.x - p.x).mul_add(e.1, -((q.y - p.y) * e.0)) / cross;
    Some(Pt::new(d.0.mul_add(t, p.x), d.1.mul_add(t, p.y)))
}

/// A closed stroke with 3 to 8 corners.
fn fit_polygon(pts: &[Pt], corners: &[usize]) -> Option<(f32, ShapeGeom)> {
    // The ends meet: the last corner is the first one again.
    let mut inner: Vec<usize> = corners[1..corners.len() - 1].to_vec();
    let start_is_corner = turn_closed(pts, 0) >= MIN_TURN;
    if start_is_corner {
        inner.insert(0, 0);
    }
    if !(3..=8).contains(&inner.len()) {
        return None;
    }
    let sides: Vec<(Pt, (f32, f32))> = (0..inner.len())
        .map(|k| {
            let (a, b) = (inner[k], inner.get(k + 1).copied().unwrap_or(pts.len() - 1));
            let mut side: Vec<Pt> = pts[a..=b].to_vec();
            if k + 1 == inner.len() && !start_is_corner {
                side.extend_from_slice(&pts[..=inner[0]]);
            }
            // Leave out the rounded ends of each side.
            let trim = side.len() / 6;
            fit_direction(&side[trim..side.len() - trim])
        })
        .collect::<Option<_>>()?;
    let vertices: Vec<Pt> = (0..sides.len())
        .map(|k| intersect(sides[(k + sides.len() - 1) % sides.len()], sides[k]))
        .collect::<Option<_>>()?;
    // Every corner must sit on the ink: a near-parallel pair of sides
    // would otherwise meet far away.
    let size = Bounds::of(pts).map(|b| b.width().hypot(b.height()))?;
    let on_ink = |v: &Pt| pts.iter().any(|p| p.distance(*v) <= 0.12 * size);
    if !vertices.iter().all(on_ink) {
        return None;
    }
    let geom = squared(vertices);
    let ShapeGeom::Polygon { points } = &geom else {
        return None;
    };
    let mut ring = points.clone();
    ring.push(points[0]);
    Some((polyline_error(pts, &ring)?, geom))
}

/// Turn at sample `i` of a closed stroke, looking across the seam.
fn turn_closed(pts: &[Pt], i: usize) -> f32 {
    let n = pts.len() - 1;
    let k = 2 * STRAW;
    let a = pts[(i + n - k) % n];
    let b = pts[(i + k) % n];
    let p = pts[i];
    let (u, v) = ((p.x - a.x, p.y - a.y), (b.x - p.x, b.y - p.y));
    u.0.mul_add(v.1, -(u.1 * v.0))
        .atan2(u.0.mul_add(v.0, u.1 * v.1))
        .abs()
}

/// Four nearly square corners become an exact (maybe turned) rectangle,
/// a square when its sides match.
fn squared(vertices: Vec<Pt>) -> ShapeGeom {
    if vertices.len() != 4 {
        return ShapeGeom::Polygon { points: vertices };
    }
    let edge = |k: usize| {
        let (a, b) = (vertices[k], vertices[(k + 1) % 4]);
        (b.y - a.y).atan2(b.x - a.x)
    };
    let square_corners = (0..4).all(|k| {
        let d = (edge((k + 1) % 4) - edge(k)).rem_euclid(PI);
        (d - FRAC_PI_2).abs() <= SQUARE_CORNER
    });
    if !square_corners {
        return ShapeGeom::Polygon { points: vertices };
    }
    // Mean orientation, folded onto a quarter turn.
    let (s, c) = (0..4).fold((0.0_f32, 0.0_f32), |(s, c), k| {
        let (sin, cos) = (4.0 * edge(k)).sin_cos();
        (s + sin, c + cos)
    });
    let angle = s.atan2(c) / 4.0;
    let (u, v) = ((angle.cos(), angle.sin()), (-angle.sin(), angle.cos()));
    let center = Pt::new(
        vertices.iter().map(|p| p.x).sum::<f32>() / 4.0,
        vertices.iter().map(|p| p.y).sum::<f32>() / 4.0,
    );
    let along =
        |p: &Pt, axis: (f32, f32)| (p.x - center.x).mul_add(axis.0, (p.y - center.y) * axis.1);
    let half = |axis: (f32, f32)| vertices.iter().map(|p| along(p, axis).abs()).sum::<f32>() / 4.0;
    let (mut w, mut h) = (half(u), half(v));
    if (w - h).abs() <= EQUAL * w.max(h) {
        let side = f32::midpoint(w, h);
        (w, h) = (side, side);
    }
    let corner = |sw: f32, sh: f32| {
        Pt::new(
            (sh * h).mul_add(v.0, (sw * w).mul_add(u.0, center.x)),
            (sh * h).mul_add(v.1, (sw * w).mul_add(u.1, center.y)),
        )
    };
    ShapeGeom::Polygon {
        points: vec![
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ],
    }
}

/// Ellipse from the second moments of evenly spread samples, scaled to
/// sit on the ink; a circle when the radii match.
fn fit_ellipse(pts: &[Pt]) -> Option<(f32, ShapeGeom)> {
    // The closing sample repeats the first.
    let ring = &pts[..pts.len() - 1];
    #[expect(clippy::cast_precision_loss, reason = "a few dozen samples")]
    let n = ring.len() as f32;
    if n < 8.0 {
        return None;
    }
    let center = Pt::new(
        ring.iter().map(|p| p.x).sum::<f32>() / n,
        ring.iter().map(|p| p.y).sum::<f32>() / n,
    );
    let (mut sxx, mut syy, mut sxy) = (0.0_f32, 0.0_f32, 0.0_f32);
    for p in ring {
        let (dx, dy) = (p.x - center.x, p.y - center.y);
        sxx = dx.mul_add(dx, sxx);
        syy = dy.mul_add(dy, syy);
        sxy = dx.mul_add(dy, sxy);
    }
    let (sxx, syy, sxy) = (sxx / n, syy / n, sxy / n);
    let rotation = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let spread = 0.25f32.mul_add((sxx - syy).powi(2), sxy * sxy).sqrt();
    let mean = f32::midpoint(sxx, syy);
    let (l1, l2) = (mean + spread, (mean - spread).max(0.0));
    let (rx, ry) = ((2.0 * l1).sqrt(), (2.0 * l2).sqrt());
    if ry < 0.15 * rx {
        return None;
    }
    let (sin, cos) = rotation.sin_cos();
    let radial = |p: &Pt, rx: f32, ry: f32| {
        let (dx, dy) = (p.x - center.x, p.y - center.y);
        let (u, v) = (dx.mul_add(cos, dy * sin), (-dx).mul_add(sin, dy * cos));
        (u / rx).hypot(v / ry)
    };
    // Radii by least squares in the ellipse frame: a·u² + b·v² = 1.
    let (mut suu, mut svv, mut suv, mut su, mut sv) = (0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32);
    for p in ring {
        let (dx, dy) = (p.x - center.x, p.y - center.y);
        let (u, v) = (dx.mul_add(cos, dy * sin), (-dx).mul_add(sin, dy * cos));
        let (u2, v2) = (u * u, v * v);
        suu = u2.mul_add(u2, suu);
        svv = v2.mul_add(v2, svv);
        suv = u2.mul_add(v2, suv);
        su += u2;
        sv += v2;
    }
    let det = suu.mul_add(svv, -(suv * suv));
    let (rx, ry) = if det.abs() > f32::EPSILON {
        let a = su.mul_add(svv, -(sv * suv)) / det;
        let b = sv.mul_add(suu, -(su * suv)) / det;
        if a > 0.0 && b > 0.0 {
            (a.sqrt().recip(), b.sqrt().recip())
        } else {
            (rx, ry)
        }
    } else {
        (rx, ry)
    };
    let error = ring
        .iter()
        .map(|p| (radial(p, rx, ry) - 1.0).abs())
        .sum::<f32>()
        / n
        * 0.5;
    let geom = if (rx - ry).abs() <= EQUAL * rx.max(ry) {
        let r = f32::midpoint(rx, ry);
        ShapeGeom::Ellipse {
            center,
            rx: r,
            ry: r,
            rotation: 0.0,
        }
    } else {
        ShapeGeom::Ellipse {
            center,
            rx,
            ry,
            rotation,
        }
    };
    Some((error, geom))
}

/// `angle` moved onto the nearest multiple of `unit` when close.
fn snapped(angle: f32, unit: f32) -> f32 {
    let nearest = (angle / unit).round() * unit;
    if (angle - nearest).abs() <= ANGLE_SNAP {
        nearest
    } else {
        angle
    }
}

/// Lines to 0/45/90°, rectangles and ellipses square to the page.
fn snap_angles(geom: ShapeGeom) -> ShapeGeom {
    let turn_end = |a: Pt, b: Pt| {
        let angle = (b.y - a.y).atan2(b.x - a.x);
        let (sin, cos) = snapped(angle, FRAC_PI_4).sin_cos();
        let len = a.distance(b);
        Pt::new(len.mul_add(cos, a.x), len.mul_add(sin, a.y))
    };
    match geom {
        ShapeGeom::Line { a, b } => ShapeGeom::Line {
            a,
            b: turn_end(a, b),
        },
        ShapeGeom::Arrow { a, b, heads } => ShapeGeom::Arrow {
            a,
            b: turn_end(a, b),
            heads,
        },
        ShapeGeom::Ellipse {
            center,
            rx,
            ry,
            rotation,
        } => ShapeGeom::Ellipse {
            center,
            rx,
            ry,
            rotation: snapped(rotation, FRAC_PI_2),
        },
        ShapeGeom::Polygon { points } if points.len() == 4 => {
            let center = Pt::new(
                points.iter().map(|p| p.x).sum::<f32>() / 4.0,
                points.iter().map(|p| p.y).sum::<f32>() / 4.0,
            );
            let edge = (points[1].y - points[0].y).atan2(points[1].x - points[0].x);
            let delta = snapped(edge, FRAC_PI_2) - edge;
            let (sin, cos) = delta.sin_cos();
            ShapeGeom::Polygon {
                points: points
                    .iter()
                    .map(|p| {
                        let (dx, dy) = (p.x - center.x, p.y - center.y);
                        Pt::new(
                            dx.mul_add(cos, -dy * sin) + center.x,
                            dx.mul_add(sin, dy * cos) + center.y,
                        )
                    })
                    .collect(),
            }
        }
        other @ ShapeGeom::Polygon { .. } => other,
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    /// Deterministic wobble.
    fn wobble(i: usize, amount: f32) -> f32 {
        #[expect(clippy::cast_precision_loss, reason = "test index")]
        let x = i as f32;
        (x * 1.7).sin() * amount
    }

    fn through(corners: &[(f32, f32)], per_side: usize, shake: f32) -> Vec<Pt> {
        let mut out = Vec::new();
        for pair in corners.windows(2) {
            for i in 0..per_side {
                #[expect(clippy::cast_precision_loss, reason = "test index")]
                let t = i as f32 / per_side as f32;
                out.push(Pt::new(
                    (pair[1].0 - pair[0].0).mul_add(t, pair[0].0) + wobble(out.len(), shake),
                    (pair[1].1 - pair[0].1).mul_add(t, pair[0].1) + wobble(out.len() + 7, shake),
                ));
            }
        }
        let last = corners[corners.len() - 1];
        out.push(Pt::new(last.0, last.1));
        out
    }

    fn ellipse(rx: f32, ry: f32, shake: f32, sweep: f32) -> Vec<Pt> {
        (0..=80)
            .map(|i| {
                #[expect(clippy::cast_precision_loss, reason = "test index")]
                let t = sweep * i as f32 / 80.0;
                Pt::new(
                    rx.mul_add(t.cos(), 200.0) + wobble(i, shake),
                    ry.mul_add(t.sin(), 150.0) + wobble(i + 3, shake),
                )
            })
            .collect()
    }

    #[test]
    fn a_shaky_line_is_a_line() {
        let ink = through(&[(10.0, 10.0), (210.0, 14.0)], 40, 1.5);
        let Some(ShapeGeom::Line { a, b }) = fit_shape(&ink, false) else {
            panic!("{:?}", fit_shape(&ink, false));
        };
        assert!((a.y - b.y).abs() < 1e-3, "snapped level");
    }

    #[test]
    fn a_rough_box_is_a_rectangle() {
        let ink = through(
            &[
                (0.0, 0.0),
                (160.0, 3.0),
                (158.0, 100.0),
                (2.0, 98.0),
                (1.0, 2.0),
            ],
            30,
            1.0,
        );
        let Some(ShapeGeom::Polygon { points }) = fit_shape(&ink, false) else {
            panic!("{:?}", fit_shape(&ink, false));
        };
        assert_eq!(points.len(), 4);
        assert!((points[0].y - points[1].y).abs() < 1e-3);
    }

    #[test]
    fn a_triangle_and_a_square() {
        let tri = through(
            &[(0.0, 100.0), (60.0, 0.0), (120.0, 100.0), (0.0, 100.0)],
            30,
            1.0,
        );
        assert!(matches!(
            fit_shape(&tri, false),
            Some(ShapeGeom::Polygon { points }) if points.len() == 3
        ));
        let square = through(
            &[
                (0.0, 0.0),
                (100.0, 0.0),
                (103.0, 97.0),
                (2.0, 101.0),
                (0.0, 0.0),
            ],
            30,
            1.0,
        );
        let Some(ShapeGeom::Polygon { points }) = fit_shape(&square, false) else {
            panic!();
        };
        let side = |k: usize| points[k].distance(points[(k + 1) % 4]);
        assert!((side(0) - side(1)).abs() < 1e-3);
    }

    #[test]
    fn circles_and_ellipses() {
        let circle = ellipse(60.0, 58.0, 1.0, TAU);
        assert!(matches!(
            fit_shape(&circle, false),
            Some(ShapeGeom::Ellipse { rx, ry, .. }) if (rx - ry).abs() < 1e-3
        ));
        let oval = ellipse(90.0, 45.0, 1.0, TAU);
        let Some(ShapeGeom::Ellipse {
            rx, ry, rotation, ..
        }) = fit_shape(&oval, false)
        else {
            panic!("{:?}", fit_shape(&oval, false));
        };
        assert!(
            (rx - 90.0).abs() < 6.0 && (ry - 45.0).abs() < 6.0,
            "{rx} {ry}"
        );
        assert!(rotation.abs() < 1e-3);
    }

    #[test]
    fn arrow_in_one_stroke() {
        let ink = through(
            &[
                (0.0, 0.0),
                (200.0, 0.0),
                (180.0, -15.0),
                (200.0, 0.0),
                (180.0, 15.0),
            ],
            20,
            0.5,
        );
        assert!(matches!(
            fit_shape(&ink, false),
            Some(ShapeGeom::Arrow {
                heads: Heads::End,
                ..
            })
        ));
    }

    #[test]
    fn head_drawn_after_the_line() {
        let head = through(&[(180.0, -15.0), (200.0, 0.0), (180.0, 15.0)], 10, 0.3);
        let a = Pt::new(0.0, 0.0);
        let b = Pt::new(200.0, 0.0);
        assert_eq!(arrow_head(a, b, &head), Some(Heads::End));
        let far = through(&[(80.0, -15.0), (100.0, 0.0), (80.0, 15.0)], 10, 0.3);
        assert_eq!(arrow_head(a, b, &far), None);
    }

    #[test]
    fn scribbles_are_not_figures() {
        let scribble: Vec<Pt> = (0..120)
            .map(|i| {
                #[expect(clippy::cast_precision_loss, reason = "test index")]
                let t = i as f32 * 0.37;
                Pt::new(
                    t.mul_add(10.0, 30.0 * (t * 2.3).sin()),
                    40.0 * (t * 1.1).cos(),
                )
            })
            .collect();
        assert_eq!(fit_shape(&scribble, false), None);
        let half_circle = ellipse(60.0, 60.0, 0.5, PI);
        assert_eq!(fit_shape(&half_circle, true), None);
    }

    #[test]
    fn ends_snap_to_nearby_points() {
        let line = ShapeGeom::Line {
            a: Pt::new(0.0, 0.0),
            b: Pt::new(98.0, 1.0),
        };
        let snapped = snap_ends(line, &[Pt::new(100.0, 0.0)], 5.0);
        assert_eq!(
            snapped,
            ShapeGeom::Line {
                a: Pt::new(0.0, 0.0),
                b: Pt::new(100.0, 0.0),
            }
        );
    }
}
