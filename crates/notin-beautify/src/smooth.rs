//! Jitter smoothing. Deterministic: the same samples and level give the
//! same output on every device, so only the level travels.

use crate::{Pt, Smooth};

/// Savitzky–Golay (quadratic) weights over 5 samples, sum 35.
const SG5: [f32; 5] = [-3.0, 12.0, 17.0, 12.0, -3.0];
/// Savitzky–Golay (quadratic) weights over 9 samples, sum 231.
const SG9: [f32; 9] = [-21.0, 14.0, 39.0, 54.0, 59.0, 54.0, 39.0, 14.0, -21.0];

impl Smooth {
    /// Half-width of the widest smoothing window.
    const fn reach(self) -> usize {
        match self {
            Self::Off => 0,
            Self::Light => 2,
            Self::Normal => 4,
        }
    }

    /// Blend of the zero-phase pressure filter; lower is smoother.
    const fn pressure_alpha(self) -> f32 {
        match self {
            Self::Off => 1.0,
            Self::Light => 0.5,
            Self::Normal => 0.35,
        }
    }
}

/// Weighted average of the window centred on `i` with `weights`.
fn window(points: &[Pt], i: usize, weights: &[f32]) -> Pt {
    let half = weights.len() / 2;
    let sum: f32 = weights.iter().sum();
    let (x, y) = points
        .iter()
        .skip(i - half)
        .zip(weights)
        .fold((0.0_f32, 0.0_f32), |(x, y), (p, w)| {
            (w.mul_add(p.x, x), w.mul_add(p.y, y))
        });
    Pt::new(x / sum, y / sum)
}

/// Positions with hand tremor smoothed out. Endpoints never move and the
/// window narrows near them, so a stroke keeps its length and corners
/// stay where they were drawn.
#[must_use]
pub fn smooth_points(points: &[Pt], level: Smooth) -> Vec<Pt> {
    let reach = level.reach();
    let last = points.len().saturating_sub(1);
    points
        .iter()
        .enumerate()
        .map(|(i, &p)| match reach.min(i).min(last - i) {
            4.. => window(points, i, &SG9),
            2..=3 => window(points, i, &SG5),
            _ => p,
        })
        .collect()
}

/// Pressure with sensor noise smoothed out: an exponential average run
/// forward and backward, so peaks do not shift along the stroke.
#[must_use]
pub fn smooth_pressure(pressure: &[u8], level: Smooth) -> Vec<u8> {
    let alpha = level.pressure_alpha();
    if level == Smooth::Off || pressure.len() < 3 {
        return pressure.to_vec();
    }
    let ema = |values: &mut dyn Iterator<Item = f32>| -> Vec<f32> {
        let mut acc: Option<f32> = None;
        values
            .map(|v| {
                let next = acc.map_or(v, |a| (v - a).mul_add(alpha, a));
                acc = Some(next);
                next
            })
            .collect()
    };
    let forward = ema(&mut pressure.iter().map(|&p| f32::from(p)));
    let mut back = ema(&mut forward.iter().rev().copied());
    back.reverse();
    back.into_iter()
        .map(|p| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "an average of u8 samples stays in 0..=255"
            )]
            let byte = p.round().clamp(0.0, 255.0) as u8;
            byte
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jittery_line() -> Vec<Pt> {
        (0..40u8)
            .map(|i| {
                let jitter = if i % 2 == 0 { 0.6 } else { -0.6 };
                Pt::new(f32::from(i) * 3.0, 50.0 + jitter)
            })
            .collect()
    }

    fn roughness(points: &[Pt]) -> f32 {
        points.windows(2).map(|w| (w[1].y - w[0].y).abs()).sum()
    }

    #[test]
    fn smoothing_calms_jitter_and_keeps_ends() {
        let raw = jittery_line();
        let light = smooth_points(&raw, Smooth::Light);
        let normal = smooth_points(&raw, Smooth::Normal);
        assert!(roughness(&light) < roughness(&raw));
        assert!(roughness(&normal) < roughness(&light));
        for out in [&light, &normal] {
            assert_eq!(out.len(), raw.len());
            assert_eq!(out.first(), raw.first());
            assert_eq!(out.last(), raw.last());
        }
    }

    #[test]
    fn off_is_identity() {
        let raw = jittery_line();
        assert_eq!(smooth_points(&raw, Smooth::Off), raw);
        assert_eq!(smooth_pressure(&[1, 200, 3], Smooth::Off), vec![1, 200, 3]);
    }

    #[test]
    fn short_strokes_survive() {
        assert!(smooth_points(&[], Smooth::Normal).is_empty());
        let one = [Pt::new(1.0, 2.0)];
        assert_eq!(smooth_points(&one, Smooth::Normal), one);
    }

    #[test]
    fn pressure_spike_is_damped() {
        let mut raw = vec![100_u8; 21];
        raw[10] = 250;
        let out = smooth_pressure(&raw, Smooth::Normal);
        assert!(out[10] < 200);
        assert!(out[10] > 100);
    }
}
