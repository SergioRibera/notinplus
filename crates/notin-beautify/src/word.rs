//! Correction of one handwritten word on its line.
//!
//! The goal is the line, not perfection: the word is turned onto the
//! line's baseline, set on it, its slant evened towards the writer's own
//! and its size towards the line's. Every change is small and bounded;
//! a word that would need more is left alone (it is probably not
//! writing).

use crate::metrics::{Bounds, Line, WordMetrics};
use crate::{Affine, Pt, Smooth, StrokeFix};

/// Largest turn onto the baseline.
const MAX_ROTATION: f32 = 8.0 * std::f32::consts::PI / 180.0;
/// Largest change of slant.
const MAX_SHEAR: f32 = 10.0 * std::f32::consts::PI / 180.0;
/// Size change range.
const SCALE: (f32, f32) = (0.85, 1.15);
/// Largest move, in x-heights.
const MAX_SHIFT: f32 = 0.6;

/// Where a word should go.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Target {
    /// Baseline of the line (or the sheet's rule). `None`: a horizontal
    /// line through the word's own baseline.
    pub baseline: Option<Line>,
    /// X-height to approach. `None`: keep the word's own.
    pub x_height: Option<f32>,
    /// Slant to approach, radians from vertical. `None`: keep it.
    pub slant: Option<f32>,
    /// Right edge of the previous word on the line and the gap to leave
    /// after it.
    pub after: Option<(f32, f32)>,
}

/// A corrected word.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The fix for every stroke of the word.
    pub fix: StrokeFix,
    /// Bounds as shown.
    pub bounds: Bounds,
    /// Baseline as shown.
    pub baseline: Line,
    /// X-height as shown.
    pub x_height: f32,
    /// What the word measured before the fix (its slant feeds the
    /// writer's profile).
    pub measured: WordMetrics,
}

impl Affine {
    /// Turn by `angle` radians about the origin (clockwise on screen,
    /// where y grows downwards).
    #[must_use]
    pub fn rotation(angle: f32) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self {
            a: cos,
            b: sin,
            c: -sin,
            d: cos,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// Uniform scale about the origin.
    #[must_use]
    pub const fn scale(factor: f32) -> Self {
        Self {
            a: factor,
            d: factor,
            ..Self::IDENTITY
        }
    }

    /// Lean ink above the x axis to the right by `k` per unit of height.
    #[must_use]
    pub const fn shear(k: f32) -> Self {
        Self {
            c: -k,
            ..Self::IDENTITY
        }
    }
}

/// Correct the word made of `strokes` towards `target`. `None` when the
/// word cannot be measured or would need more than the bounded change.
#[must_use]
#[cfg_attr(feature = "sismo", sismo::function("beautify::correct_word"))]
pub fn correct_word(strokes: &[&[Pt]], target: &Target) -> Option<Placement> {
    let measured = WordMetrics::measure(strokes)?;
    let own = measured.baseline;
    let pivot_x = measured.bounds.center().x;
    let pivot = Pt::new(pivot_x, own.y_at(pivot_x));
    let goal = target.baseline.unwrap_or_else(|| Line::horizontal(pivot.y));

    let (from_angle, to_angle) = (own.angle(), goal.angle());
    if (to_angle - from_angle).abs() > MAX_ROTATION {
        return None;
    }
    let scale = target
        .x_height
        .map_or(1.0, |h| h / measured.x_height.max(f32::EPSILON));
    if !(SCALE.0..=SCALE.1).contains(&scale) {
        return None;
    }
    // Slant relative to the baseline once it is level.
    let lean = measured.slant - from_angle;
    let wanted = target
        .slant
        .map_or(lean, |s| lean + (s - lean).clamp(-MAX_SHEAR, MAX_SHEAR));
    let shear = wanted.tan() - lean.tan();

    // Level frame: pivot at the origin, baseline along +x.
    let level = Affine::translation(-pivot.x, -pivot.y)
        .then(&Affine::rotation(-from_angle))
        .then(&Affine::shear(shear))
        .then(&Affine::scale(scale))
        .then(&Affine::rotation(to_angle));
    let x_height = measured.x_height * scale;
    let max_shift = MAX_SHIFT * x_height.max(measured.x_height);

    // Left edge after the reshape, to place the word after the previous.
    let shaped_left = strokes
        .iter()
        .flat_map(|s| s.iter())
        .map(|p| level.apply(*p).x)
        .fold(f32::INFINITY, f32::min);
    let mut land_x = pivot.x;
    if let Some((prev_right, gap)) = target.after {
        let dx = (prev_right + gap) - (shaped_left + pivot.x);
        if dx.abs() <= max_shift {
            land_x += dx;
        }
    }
    let land = Pt::new(land_x, goal.y_at(land_x));
    if (land.y - pivot.y).abs() > max_shift {
        return None;
    }
    let affine = level.then(&Affine::translation(land.x, land.y));
    if !affine.is_finite() {
        return None;
    }
    let fix = StrokeFix {
        affine,
        smooth: Smooth::Normal,
    };
    let shown: Vec<Pt> = strokes.iter().flat_map(|s| fix.apply(s)).collect();
    let bounds = Bounds::of(&shown)?;
    let baseline = Line {
        slope: to_angle.tan(),
        offset: to_angle.tan().mul_add(-land.x, land.y),
    };
    Some(Placement {
        fix,
        bounds,
        baseline,
        x_height,
        measured,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Letter, word};

    fn refs(strokes: &[Vec<Pt>]) -> Vec<&[Pt]> {
        strokes.iter().map(Vec::as_slice).collect()
    }

    #[test]
    fn tilted_word_lands_level_on_the_target() {
        let ink = word(
            &[Letter::N, Letter::O, Letter::N],
            (100.0, 200.0),
            12.0,
            5.0,
            0.1,
            7,
        );
        let target = Target {
            baseline: Some(Line::horizontal(203.0)),
            ..Target::default()
        };
        let placed = correct_word(&refs(&ink), &target).unwrap();
        // The geometry, without the smoothing (which nudges turning points).
        let shown: Vec<Vec<Pt>> = ink
            .iter()
            .map(|s| s.iter().map(|p| placed.fix.affine.apply(*p)).collect())
            .collect();
        let after = WordMetrics::measure(&refs(&shown)).unwrap();
        assert!(after.baseline.angle().abs() < 1.0_f32.to_radians());
        assert!((after.baseline.y_at(after.bounds.center().x) - 203.0).abs() < 1.0);
    }

    #[test]
    fn far_off_words_are_left_alone() {
        let ink = word(&[Letter::N, Letter::N], (0.0, 100.0), 12.0, 0.0, 0.0, 1);
        let too_low = Target {
            baseline: Some(Line::horizontal(130.0)),
            ..Target::default()
        };
        assert!(correct_word(&refs(&ink), &too_low).is_none());
        let too_small = Target {
            x_height: Some(6.0),
            ..Target::default()
        };
        assert!(correct_word(&refs(&ink), &too_small).is_none());
    }

    #[test]
    fn crooked_line_ends_up_on_one_baseline() {
        use crate::Row;
        let words = [
            word(&[Letter::N, Letter::O], (0.0, 100.0), 12.0, 8.0, 0.06, 1),
            word(
                &[Letter::L, Letter::O, Letter::G],
                (45.0, 103.0),
                13.0,
                10.0,
                -0.05,
                2,
            ),
            word(&[Letter::I, Letter::N], (100.0, 98.0), 11.0, 6.0, 0.09, 3),
            word(
                &[Letter::O, Letter::N, Letter::O],
                (140.0, 104.0),
                12.5,
                9.0,
                -0.08,
                4,
            ),
        ];
        let mut row: Option<Row> = None;
        let mut baselines = Vec::new();
        for ink in &words {
            let target = row.as_ref().map_or_else(Target::default, |row| Target {
                baseline: Some(row.baseline),
                x_height: row.x_height(),
                ..Target::default()
            });
            let placed = correct_word(&refs(ink), &target).unwrap();
            let shown: Vec<Vec<Pt>> = ink
                .iter()
                .map(|s| s.iter().map(|p| placed.fix.affine.apply(*p)).collect())
                .collect();
            let after = WordMetrics::measure(&refs(&shown)).unwrap();
            baselines.push(after.baseline.y_at(after.bounds.center().x));
            row.get_or_insert_with(|| Row::new(placed.baseline, false))
                .place(placed.bounds, placed.x_height);
        }
        let (lo, hi) = baselines
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), y| (lo.min(*y), hi.max(*y)));
        assert!(hi - lo < 1.0, "{baselines:?}");
    }

    #[test]
    fn corrections_stay_within_bounds() {
        let letters = [Letter::N, Letter::O, Letter::L, Letter::G, Letter::I];
        let mut corrected = 0;
        for seed in 0..200_u32 {
            let pick = |k: u32| letters[((seed * 7 + k * 13) % 5) as usize];
            let tilt = f32::from(u16::try_from(seed % 21).unwrap_or(0)).mul_add(0.02, -0.2);
            let ink = word(
                &[pick(0), pick(1), pick(2)],
                (0.0, 50.0),
                12.0,
                15.0,
                tilt,
                seed,
            );
            let target = Target {
                baseline: Some(Line::horizontal(52.0)),
                x_height: Some(11.0),
                slant: Some(0.05),
                after: Some((-8.0, 5.0)),
            };
            let Some(placed) = correct_word(&refs(&ink), &target) else {
                continue;
            };
            let a = placed.fix.affine;
            let det = a.a.mul_add(a.d, -(a.b * a.c));
            assert!(
                (SCALE.0.mul_add(SCALE.0, -1e-3)..=SCALE.1.mul_add(SCALE.1, 1e-3)).contains(&det)
            );
            assert!(a.b.atan2(a.a).abs() <= MAX_ROTATION + MAX_SHEAR);
            assert!(a.is_finite());
            corrected += 1;
        }
        assert!(corrected > 50, "only {corrected} words corrected");
    }

    #[test]
    fn spacing_follows_the_line() {
        let ink = word(&[Letter::O, Letter::N], (60.0, 100.0), 12.0, 0.0, 0.0, 3);
        let measured = WordMetrics::measure(&refs(&ink)).unwrap();
        let target = Target {
            after: Some((measured.bounds.min_x - 10.0, 6.0)),
            ..Target::default()
        };
        let placed = correct_word(&refs(&ink), &target).unwrap();
        assert!((placed.bounds.min_x - (measured.bounds.min_x - 4.0)).abs() < 0.5);
    }
}
