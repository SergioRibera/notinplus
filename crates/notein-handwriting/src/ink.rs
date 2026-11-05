//! Handwritten samples reduced to fixed-length traces, compared with
//! dynamic time warping over position and pen direction.
//!
//! Order and direction matter: writers mostly agree on how a
//! character is drawn, and that is what separates `q` from `9` or `y`
//! from `g`. An order-free matcher (`$Q`) scored ~11 points lower on
//! `UJIpenchars2` (see `glyph-bake eval`).

/// Points every trace is resampled to.
pub const TRACE_POINTS: usize = 32;

/// Weight of the direction term against the position term.
const DIRECTION_WEIGHT: f32 = 0.15;

/// 2D point. Y grows downwards, matching screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    #[must_use]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub(crate) fn distance(self, other: Self) -> f32 {
        (self.x - other.x).hypot(self.y - other.y)
    }

    pub(crate) fn distance_to_segment(self, from: Self, to: Self) -> f32 {
        let (dx, dy) = (to.x - from.x, to.y - from.y);
        let length = dx.mul_add(dx, dy * dy);
        if length <= f32::EPSILON {
            return self.distance(from);
        }
        let t = ((self.x - from.x).mul_add(dx, (self.y - from.y) * dy) / length).clamp(0.0, 1.0);
        self.distance(from.lerp(to, t))
    }

    fn lerp(self, other: Self, t: f32) -> Self {
        Self::new(
            (other.x - self.x).mul_add(t, self.x),
            (other.y - self.y).mul_add(t, self.y),
        )
    }
}

/// Axis-aligned bounds of a set of points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Bounds {
    pub(crate) min: Point,
    pub(crate) max: Point,
}

impl Bounds {
    /// `None` for an empty iterator.
    pub(crate) fn of<'a>(points: impl IntoIterator<Item = &'a Point>) -> Option<Self> {
        points.into_iter().fold(None, |bounds, point| {
            Some(bounds.map_or(
                Self {
                    min: *point,
                    max: *point,
                },
                |bounds: Self| Self {
                    min: Point::new(bounds.min.x.min(point.x), bounds.min.y.min(point.y)),
                    max: Point::new(bounds.max.x.max(point.x), bounds.max.y.max(point.y)),
                },
            ))
        })
    }

    pub(crate) fn width(self) -> f32 {
        self.max.x - self.min.x
    }

    pub(crate) fn height(self) -> f32 {
        self.max.y - self.min.y
    }
}

/// Position plus unit pen direction at one trace point.
type Feature = [f32; 4];

/// Sample ready for matching: [`TRACE_POINTS`] points in writing
/// order, scaled to unit size and centred on their centroid.
#[derive(Debug, Clone)]
pub struct Trace {
    points: [Point; TRACE_POINTS],
    /// Bit `i` set when point `i` begins a new stroke.
    starts: u32,
    features: [Feature; TRACE_POINTS],
}

impl Trace {
    /// Resamples, scales and centres raw strokes. `None` when they
    /// hold no points at all.
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("handwriting::trace_from_strokes"))]
    pub fn from_strokes<S: AsRef<[Point]>>(strokes: &[S]) -> Option<Self> {
        let strokes: Vec<&[Point]> = strokes
            .iter()
            .map(AsRef::as_ref)
            .filter(|stroke| !stroke.is_empty())
            .collect();
        if strokes.is_empty() {
            return None;
        }
        let (mut points, starts) = resample(&strokes);
        normalize(&mut points);
        Some(Self::from_normalized(points, starts))
    }

    /// Rebuilds a trace from stored, already normalized points.
    #[must_use]
    pub fn from_normalized(points: [Point; TRACE_POINTS], starts: u32) -> Self {
        let features = std::array::from_fn(|index| {
            let starts_at = |index: usize| starts & (1 << index) != 0;
            let segment = if index + 1 < TRACE_POINTS && !starts_at(index + 1) {
                Some((points[index], points[index + 1]))
            } else if index > 0 && !starts_at(index) {
                Some((points[index - 1], points[index]))
            } else {
                None
            };
            let (cos, sin) = segment
                .map(|(from, to)| (to.x - from.x, to.y - from.y))
                .filter(|(dx, dy)| dx.hypot(*dy) > 1e-6)
                .map_or((0.0, 0.0), |(dx, dy)| {
                    let length = dx.hypot(dy);
                    (dx / length, dy / length)
                });
            [points[index].x, points[index].y, cos, sin]
        });
        Self {
            points,
            starts,
            features,
        }
    }

    #[must_use]
    pub const fn points(&self) -> &[Point; TRACE_POINTS] {
        &self.points
    }

    /// Stroke starts as a bitmask (bit `i` = point `i` starts a stroke).
    #[must_use]
    pub const fn starts(&self) -> u32 {
        self.starts
    }

    /// DTW distance. Stops as soon as no alignment can get under
    /// `bound`, returning a value `>= bound`.
    #[must_use]
    pub fn distance(&self, other: &Self, bound: f32) -> f32 {
        let cost = |a: &Feature, b: &Feature| {
            let position = (a[0] - b[0]).mul_add(a[0] - b[0], (a[1] - b[1]).powi(2));
            let direction = (a[2] - b[2]).mul_add(a[2] - b[2], (a[3] - b[3]).powi(2));
            DIRECTION_WEIGHT.mul_add(direction, position)
        };
        let mut previous = [f32::INFINITY; TRACE_POINTS];
        let mut current = [f32::INFINITY; TRACE_POINTS];
        for (row, a) in self.features.iter().enumerate() {
            let mut row_min = f32::INFINITY;
            for (column, b) in other.features.iter().enumerate() {
                let reach = match (row, column) {
                    (0, 0) => 0.0,
                    (0, _) => current[column - 1],
                    (_, 0) => previous[0],
                    _ => previous[column]
                        .min(current[column - 1])
                        .min(previous[column - 1]),
                };
                current[column] = reach + cost(a, b);
                row_min = row_min.min(current[column]);
            }
            // Costs are non-negative: every alignment crosses this row.
            if row_min >= bound {
                return row_min;
            }
            std::mem::swap(&mut previous, &mut current);
        }
        previous[TRACE_POINTS - 1]
    }
}

fn stroke_length(stroke: &[Point]) -> f32 {
    stroke
        .windows(2)
        .map(|pair| pair[0].distance(pair[1]))
        .sum()
}

/// Spreads [`TRACE_POINTS`] across strokes proportionally to their
/// length. Every stroke keeps at least one point so dots (tittles,
/// accents, periods) survive resampling.
#[cfg_attr(feature = "sismo", sismo::function("handwriting::resample"))]
fn resample(strokes: &[&[Point]]) -> ([Point; TRACE_POINTS], u32) {
    let mut points = Vec::with_capacity(TRACE_POINTS);
    let mut starts = 0;
    for (stroke, count) in strokes.iter().zip(allocate(strokes)) {
        if count > 0 {
            starts |= 1 << points.len();
        }
        resample_stroke(stroke, count, &mut points);
    }
    // `allocate` hands out exactly TRACE_POINTS and each stroke emits
    // its count, so the padding never triggers in practice.
    let last = points.last().copied().unwrap_or_default();
    points.resize(TRACE_POINTS, last);
    (std::array::from_fn(|index| points[index]), starts)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "shares are non-negative and bounded by TRACE_POINTS"
)]
fn allocate(strokes: &[&[Point]]) -> Vec<usize> {
    let guaranteed = strokes.len().min(TRACE_POINTS);
    let mut counts: Vec<usize> = (0..strokes.len())
        .map(|index| usize::from(index < guaranteed))
        .collect();
    let spare = TRACE_POINTS - guaranteed;
    let lengths: Vec<f32> = strokes.iter().map(|stroke| stroke_length(stroke)).collect();
    let total: f32 = lengths.iter().sum();
    if spare == 0 {
        return counts;
    }
    if total <= f32::EPSILON {
        counts[0] += spare;
        return counts;
    }
    // Largest-remainder apportionment of the spare points.
    let shares: Vec<f32> = lengths
        .iter()
        .map(|length| length / total * spare as f32)
        .collect();
    let mut given = 0;
    for (count, share) in counts.iter_mut().zip(&shares) {
        let whole = share.floor() as usize;
        *count += whole;
        given += whole;
    }
    let mut by_remainder: Vec<usize> = (0..strokes.len()).collect();
    by_remainder.sort_by(|a, b| shares[*b].fract().total_cmp(&shares[*a].fract()));
    for index in by_remainder.into_iter().cycle().take(spare - given) {
        counts[index] += 1;
    }
    counts
}

#[allow(
    clippy::cast_precision_loss,
    reason = "counts are bounded by TRACE_POINTS"
)]
fn resample_stroke(stroke: &[Point], count: usize, out: &mut Vec<Point>) {
    let length = stroke_length(stroke);
    if count == 0 {
        return;
    }
    if count == 1 || length <= f32::EPSILON {
        out.extend(std::iter::repeat_n(centroid(stroke), count));
        return;
    }
    let spacing = length / (count - 1) as f32;
    let mut emitted = 0;
    let mut travelled = 0.0;
    for pair in stroke.windows(2) {
        let segment = pair[0].distance(pair[1]);
        while emitted < count {
            let target = spacing * emitted as f32;
            if target > travelled + segment {
                break;
            }
            let t = if segment > 0.0 {
                (target - travelled) / segment
            } else {
                0.0
            };
            out.push(pair[0].lerp(pair[1], t));
            emitted += 1;
        }
        travelled += segment;
    }
    // Float drift can leave the final endpoint unemitted.
    let last = stroke[stroke.len() - 1];
    out.extend(std::iter::repeat_n(last, count - emitted));
}

#[allow(
    clippy::cast_precision_loss,
    reason = "point counts are far below f32's exact integer range"
)]
fn centroid(points: &[Point]) -> Point {
    let n = points.len().max(1) as f32;
    let (x, y) = points
        .iter()
        .fold((0.0, 0.0), |(x, y), point| (x + point.x, y + point.y));
    Point::new(x / n, y / n)
}

/// Uniform scale to unit size (aspect ratio preserved), then centroid
/// to the origin.
#[cfg_attr(feature = "sismo", sismo::function("handwriting::normalize"))]
fn normalize(points: &mut [Point; TRACE_POINTS]) {
    let Some(bounds) = Bounds::of(points.iter()) else {
        return;
    };
    let size = bounds.width().max(bounds.height());
    let scale = if size > f32::EPSILON {
        size.recip()
    } else {
        1.0
    };
    for point in points.iter_mut() {
        *point = Point::new(
            (point.x - bounds.min.x) * scale,
            (point.y - bounds.min.y) * scale,
        );
    }
    let center = centroid(points);
    for point in points.iter_mut() {
        *point = Point::new(point.x - center.x, point.y - center.y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::cast_precision_loss, reason = "test fixtures")]
    fn line(from: Point, to: Point, steps: usize) -> Vec<Point> {
        (0..=steps)
            .map(|step| from.lerp(to, step as f32 / steps as f32))
            .collect()
    }

    #[test]
    fn allocation_hands_out_every_point_and_keeps_dots() {
        let stem = line(Point::new(0.0, 0.0), Point::new(0.0, 100.0), 20);
        let dot = vec![Point::new(0.0, -20.0); 3];
        let counts = allocate(&[stem.as_slice(), dot.as_slice()]);
        assert_eq!(counts.iter().sum::<usize>(), TRACE_POINTS);
        assert_eq!(counts[1], 1);
    }

    #[test]
    fn stroke_starts_are_tracked() {
        let stem = line(Point::new(0.0, 0.0), Point::new(0.0, 100.0), 20);
        let dot = vec![Point::new(0.0, -20.0)];
        let trace = Trace::from_strokes(&[stem, dot]).unwrap();
        assert_eq!(trace.starts().count_ones(), 2);
        assert_eq!(trace.starts() & 1, 1);
    }

    #[test]
    fn identical_samples_have_zero_distance() {
        let trace =
            Trace::from_strokes(&[line(Point::new(0.0, 0.0), Point::new(50.0, 80.0), 10)]).unwrap();
        assert!(trace.distance(&trace, f32::INFINITY) < 1e-6);
    }

    #[test]
    fn direction_separates_mirrored_strokes() {
        let down =
            Trace::from_strokes(&[line(Point::new(0.0, 0.0), Point::new(0.0, 100.0), 10)]).unwrap();
        let up =
            Trace::from_strokes(&[line(Point::new(0.0, 100.0), Point::new(0.0, 0.0), 10)]).unwrap();
        let wobbly = Trace::from_strokes(&[vec![
            Point::new(0.0, 0.0),
            Point::new(2.0, 48.0),
            Point::new(0.0, 100.0),
        ]])
        .unwrap();
        let near = down.distance(&wobbly, f32::INFINITY);
        let far = down.distance(&up, f32::INFINITY);
        assert!(near < far, "near {near} far {far}");
    }

    #[test]
    fn abandoned_distance_never_undercuts_bound() {
        let a =
            Trace::from_strokes(&[line(Point::new(0.0, 0.0), Point::new(100.0, 0.0), 10)]).unwrap();
        let b =
            Trace::from_strokes(&[line(Point::new(0.0, 0.0), Point::new(0.0, 100.0), 10)]).unwrap();
        let exact = a.distance(&b, f32::INFINITY);
        assert!(a.distance(&b, exact * 0.5) >= exact * 0.5);
    }
}
