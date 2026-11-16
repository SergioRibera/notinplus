//! Measurements of handwriting: baseline, x-height, slant.
//!
//! World coordinates grow downwards, so the bottom of a letter is a
//! local *maximum* of `y`.

use crate::Pt;

/// Width of one slant histogram bin.
const SLANT_BIN: f32 = 2.0 * std::f32::consts::PI / 180.0;
/// Segments steeper than this from vertical do not vote on the slant.
const SLANT_RANGE: f32 = 40.0 * std::f32::consts::PI / 180.0;

/// World-space bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

impl Bounds {
    /// Bounds of `points`, `None` when empty.
    #[must_use]
    pub fn of<'a>(points: impl IntoIterator<Item = &'a Pt>) -> Option<Self> {
        points.into_iter().fold(None, |acc, p| {
            Some(acc.map_or(
                Self {
                    min_x: p.x,
                    min_y: p.y,
                    max_x: p.x,
                    max_y: p.y,
                },
                |b: Self| Self {
                    min_x: b.min_x.min(p.x),
                    min_y: b.min_y.min(p.y),
                    max_x: b.max_x.max(p.x),
                    max_y: b.max_y.max(p.y),
                },
            ))
        })
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }

    #[must_use]
    pub fn width(self) -> f32 {
        self.max_x - self.min_x
    }

    #[must_use]
    pub fn height(self) -> f32 {
        self.max_y - self.min_y
    }

    #[must_use]
    pub const fn center(self) -> Pt {
        Pt::new(
            f32::midpoint(self.min_x, self.max_x),
            f32::midpoint(self.min_y, self.max_y),
        )
    }
}

/// `y = slope·x + offset`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub slope: f32,
    pub offset: f32,
}

impl Line {
    #[must_use]
    pub const fn horizontal(y: f32) -> Self {
        Self {
            slope: 0.0,
            offset: y,
        }
    }

    #[must_use]
    pub fn y_at(self, x: f32) -> f32 {
        self.slope.mul_add(x, self.offset)
    }

    /// Angle from +x, radians.
    #[must_use]
    pub fn angle(self) -> f32 {
        self.slope.atan()
    }

    /// How far `p` sits above the line (positive above).
    #[must_use]
    pub fn height_of(self, p: Pt) -> f32 {
        self.y_at(p.x) - p.y
    }

    /// Robust fit through `points` (Theil–Sen: median of pairwise
    /// slopes, median intercept). One point gives a horizontal line.
    #[must_use]
    pub fn theil_sen(points: &[Pt]) -> Option<Self> {
        let first = points.first()?;
        let mut slopes = Vec::new();
        for (i, a) in points.iter().enumerate() {
            for b in &points[i + 1..] {
                let dx = b.x - a.x;
                if dx.abs() > f32::EPSILON {
                    slopes.push((b.y - a.y) / dx);
                }
            }
        }
        let slope = median(&mut slopes).unwrap_or(0.0);
        let mut offsets: Vec<f32> = points.iter().map(|p| slope.mul_add(-p.x, p.y)).collect();
        let offset = median(&mut offsets).unwrap_or(first.y);
        Some(Self { slope, offset })
    }
}

/// Median, `None` when empty. Reorders `values`.
#[must_use]
pub fn median(values: &mut [f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f32::total_cmp);
    let mid = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        f32::midpoint(values[mid - 1], values[mid])
    } else {
        values[mid]
    })
}

/// The `q` quantile (0..=1), nearest rank. Reorders `values`.
fn quantile(values: &mut [f32], q: f32) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f32::total_cmp);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "rank into a non-empty slice, clamped"
    )]
    let rank = ((values.len() - 1) as f32 * q.clamp(0.0, 1.0)).round() as usize;
    values.get(rank).copied()
}

/// Turning points of a stroke in `y`: `(tops, bottoms)`. A turn counts
/// once the pen has come back by a fifth of the stroke's height, so
/// tremor does not add turns. Stroke ends count too: a single dash still
/// has a top and a bottom.
#[must_use]
pub(crate) fn extrema(points: &[Pt]) -> (Vec<Pt>, Vec<Pt>) {
    let (mut tops, mut bottoms) = (Vec::new(), Vec::new());
    let Some(bounds) = Bounds::of(points) else {
        return (tops, bottoms);
    };
    let back = (0.2 * bounds.height()).max(0.5);
    // Highest and lowest point since the last turn.
    let (mut high, mut low) = (points[0], points[0]);
    // `Some(true)` going down, `Some(false)` going up.
    let mut down: Option<bool> = None;
    for &p in &points[1..] {
        if p.y < high.y {
            high = p;
        }
        if p.y > low.y {
            low = p;
        }
        match down {
            None | Some(true) if low.y - p.y > back => {
                bottoms.push(low);
                down = Some(false);
                high = p;
            }
            None | Some(false) if p.y - high.y > back => {
                tops.push(high);
                down = Some(true);
                low = p;
            }
            _ => {}
        }
    }
    match down {
        Some(true) => bottoms.push(low),
        Some(false) => tops.push(high),
        None => {
            tops.push(high);
            bottoms.push(low);
        }
    }
    (tops, bottoms)
}

/// What a group of handwritten strokes measures.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WordMetrics {
    pub baseline: Line,
    pub x_height: f32,
    /// Lean from vertical, radians, positive leaning right.
    pub slant: f32,
    pub bounds: Bounds,
}

impl WordMetrics {
    /// Measure `strokes`. `None` without ink or without anything to fit
    /// a baseline on.
    #[must_use]
    pub fn measure(strokes: &[&[Pt]]) -> Option<Self> {
        let bounds = Bounds::of(strokes.iter().flat_map(|s| s.iter()))?;
        let (mut tops, mut bottoms) = (Vec::new(), Vec::new());
        for stroke in strokes {
            let (t, b) = extrema(stroke);
            tops.extend(t);
            bottoms.extend(b);
        }
        let baseline = baseline(&bottoms)?;
        let x_height = x_height(baseline, &tops).unwrap_or_else(|| bounds.height());
        Some(Self {
            baseline,
            x_height: x_height.max(1.0),
            slant: slant(strokes).unwrap_or(0.0),
            bounds,
        })
    }
}

/// Baseline through letter bottoms, ignoring descenders: fit, drop the
/// bottoms that hang well below the fit, fit again.
fn baseline(bottoms: &[Pt]) -> Option<Line> {
    let first = Line::theil_sen(bottoms)?;
    if bottoms.len() < 4 {
        return Some(first);
    }
    let mut depths: Vec<f32> = bottoms.iter().map(|p| -first.height_of(*p)).collect();
    let cut = quantile(&mut depths, 0.75)?;
    let spread = quantile(&mut depths, 0.5).map_or(0.0, |m| (cut - m).abs());
    let kept: Vec<Pt> = bottoms
        .iter()
        .copied()
        .filter(|p| -first.height_of(*p) <= cut + spread)
        .collect();
    Line::theil_sen(&kept).or(Some(first))
}

/// Typical height of letter bodies above `baseline`, ascenders and
/// dots left out.
fn x_height(baseline: Line, tops: &[Pt]) -> Option<f32> {
    let mut heights: Vec<f32> = tops
        .iter()
        .map(|p| baseline.height_of(*p))
        .filter(|h| *h > 0.0)
        .collect();
    let rough = median(&mut heights)?;
    let mut bodies: Vec<f32> = heights
        .into_iter()
        .filter(|h| *h >= 0.5 * rough && *h <= 1.5 * rough)
        .collect();
    median(&mut bodies).or(Some(rough))
}

/// Dominant lean of near-vertical segments, weighted by length.
fn slant(strokes: &[&[Pt]]) -> Option<f32> {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "bin count from two small constants"
    )]
    let bins = (2.0 * SLANT_RANGE / SLANT_BIN).ceil() as usize + 1;
    let mut histogram = vec![0.0_f32; bins];
    let mut any = false;
    for stroke in strokes {
        for pair in stroke.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let length = dx.hypot(dy);
            if length <= f32::EPSILON {
                continue;
            }
            // Orient every segment upwards, then measure from vertical.
            let (dx, dy) = if dy > 0.0 { (-dx, -dy) } else { (dx, dy) };
            let lean = dx.atan2(-dy);
            if lean.abs() > SLANT_RANGE {
                continue;
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "lean is within ±SLANT_RANGE"
            )]
            let bin = ((lean + SLANT_RANGE) / SLANT_BIN).round() as usize;
            if let Some(slot) = histogram.get_mut(bin) {
                *slot += length;
                any = true;
            }
        }
    }
    if !any {
        return None;
    }
    // Smooth over neighbours so a lean between two bins is not split.
    let smoothed: Vec<f32> = (0..bins)
        .map(|i| {
            let left = i.checked_sub(1).and_then(|j| histogram.get(j)).copied();
            let right = histogram.get(i + 1).copied();
            0.5f32.mul_add(left.unwrap_or(0.0) + right.unwrap_or(0.0), histogram[i])
        })
        .collect();
    let peak = smoothed
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)?;
    #[expect(clippy::cast_precision_loss, reason = "a few dozen bins")]
    let lean = (peak as f32).mul_add(SLANT_BIN, -SLANT_RANGE);
    Some(lean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theil_sen_ignores_an_outlier() {
        let mut points: Vec<Pt> = (0..10u8)
            .map(|i| Pt::new(f32::from(i) * 10.0, f32::from(i).mul_add(2.0, 50.0)))
            .collect();
        points[4].y += 40.0;
        let line = Line::theil_sen(&points).unwrap();
        assert!((line.slope - 0.2).abs() < 1e-4);
        assert!((line.offset - 50.0).abs() < 1e-3);
    }

    #[test]
    fn extrema_of_a_zigzag() {
        let zigzag = [
            Pt::new(0.0, 10.0),
            Pt::new(1.0, 0.0),
            Pt::new(2.0, 10.0),
            Pt::new(3.0, 0.0),
            Pt::new(4.0, 10.0),
        ];
        let (tops, bottoms) = extrema(&zigzag);
        assert_eq!(tops.len(), 2);
        assert_eq!(bottoms.len(), 3);
    }

    #[test]
    fn slant_of_leaning_strokes() {
        let lean = 12.0_f32.to_radians();
        let strokes: Vec<Vec<Pt>> = (0..5u8)
            .map(|i| {
                let x = f32::from(i) * 20.0;
                vec![Pt::new(x, 40.0), Pt::new(lean.tan().mul_add(40.0, x), 0.0)]
            })
            .collect();
        let refs: Vec<&[Pt]> = strokes.iter().map(Vec::as_slice).collect();
        let measured = slant(&refs).unwrap();
        assert!((measured - lean).abs() < 2.0_f32.to_radians(), "{measured}");
    }
}
