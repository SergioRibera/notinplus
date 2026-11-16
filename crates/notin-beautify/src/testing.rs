//! Synthetic handwriting for tests: simple letter shapes, slanted,
//! tilted and shaken deterministically.

use std::f32::consts::{PI, TAU};

use crate::Pt;

/// Letter shapes, in x-height units, baseline at y = 0, up negative.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Letter {
    /// Stem and one arch.
    N,
    /// Closed loop.
    O,
    /// Tall stem.
    L,
    /// Loop with a descender.
    G,
    /// Short stem plus a dot (two strokes).
    I,
}

impl Letter {
    const fn width(self) -> f32 {
        match self {
            Self::N | Self::O | Self::G => 0.8,
            Self::L | Self::I => 0.25,
        }
    }

    fn strokes(self) -> Vec<Vec<(f32, f32)>> {
        let arc = |cx: f32, cy: f32, r: f32, from: f32, to: f32| -> Vec<(f32, f32)> {
            (0..=16u8)
                .map(|i| {
                    let t = (to - from).mul_add(f32::from(i) / 16.0, from);
                    (r.mul_add(t.cos(), cx), r.mul_add(t.sin(), cy))
                })
                .collect()
        };
        let line = |a: (f32, f32), b: (f32, f32)| -> Vec<(f32, f32)> {
            (0..=8u8)
                .map(|i| {
                    let t = f32::from(i) / 8.0;
                    ((b.0 - a.0).mul_add(t, a.0), (b.1 - a.1).mul_add(t, a.1))
                })
                .collect()
        };
        match self {
            Self::N => {
                let mut s = line((0.0, 0.0), (0.0, -1.0));
                s.extend(arc(0.4, -0.6, 0.4, PI, TAU));
                s.extend(line((0.8, -0.6), (0.8, 0.0)));
                vec![s]
            }
            Self::O => vec![arc(0.4, -0.5, 0.45, 0.0, TAU)],
            Self::L => vec![line((0.1, 0.0), (0.1, -2.0))],
            Self::G => {
                let mut s = arc(0.4, -0.5, 0.45, 0.0, TAU);
                s.extend(line((0.85, -0.5), (0.85, 0.8)));
                vec![s]
            }
            Self::I => vec![
                line((0.1, 0.0), (0.1, -1.0)),
                line((0.1, -1.5), (0.12, -1.45)),
            ],
        }
    }
}

/// Deterministic noise in -1..1.
struct Shake(u32);

impl Shake {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        f32::from(u16::try_from(self.0 >> 16).unwrap_or(0)) / 32_768.0 - 1.0
    }
}

/// A word: `letters` written at `origin` (left end of the baseline),
/// `x_height` tall, leaning `slant_deg` to the right, the whole word
/// turned by `tilt` radians and shaken by `seed`.
pub(crate) fn word(
    letters: &[Letter],
    origin: (f32, f32),
    x_height: f32,
    slant_deg: f32,
    tilt: f32,
    seed: u32,
) -> Vec<Vec<Pt>> {
    let lean = slant_deg.to_radians().tan();
    let (sin, cos) = tilt.sin_cos();
    let mut shake = Shake(seed);
    let mut pen = 0.0;
    let mut out = Vec::new();
    for letter in letters {
        for stroke in letter.strokes() {
            out.push(
                stroke
                    .into_iter()
                    .map(|(x, y)| {
                        let x = lean.mul_add(-y, x + pen) * x_height;
                        let y = y * x_height;
                        let (x, y) = (x.mul_add(cos, -y * sin), x.mul_add(sin, y * cos));
                        Pt::new(
                            shake.next().mul_add(0.3, origin.0 + x),
                            shake.next().mul_add(0.3, origin.1 + y),
                        )
                    })
                    .collect(),
            );
        }
        pen += letter.width() + 0.2;
    }
    out
}
