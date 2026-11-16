//! Beautifying ink after the fact: a selection of strokes, old or
//! received, written with the switch off or not.
//!
//! Strokes the size of writing are grouped into words and lines (inside
//! the selection only) and corrected like freshly written words. Loose
//! larger strokes become figures, with a stricter bar than holding the
//! pen: here nobody asked for a figure.

use crate::layout::{Row, group_lines, group_words, is_writing};
use crate::metrics::{Bounds, Line, WordMetrics, median};
use crate::shape::fit_shape;
use crate::word::{Target, correct_word};
use crate::{Pt, ShapeGeom, StrokeFix};

/// What beautifying a selection does to its strokes (indices into the
/// strokes given).
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// These strokes are one word; each gets `fix`.
    Word { strokes: Vec<usize>, fix: StrokeFix },
    /// This stroke is a figure.
    Shape { stroke: usize, geom: ShapeGeom },
}

/// Hints from the surroundings.
#[derive(Clone, Copy, Default)]
pub struct Context<'a> {
    /// The writer's usual slant.
    pub slant: Option<f32>,
    /// Spacing of the sheet's rules at a point, if ruled there.
    pub rules: Option<&'a dyn Fn(Pt) -> Option<f32>>,
}

impl std::fmt::Debug for Context<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("slant", &self.slant)
            .field("ruled", &self.rules.is_some())
            .finish()
    }
}

/// Beautify `strokes` (in writing order).
#[must_use]
#[cfg_attr(feature = "sismo", sismo::function("beautify::beautify_all"))]
pub fn beautify_all(strokes: &[&[Pt]], context: &Context<'_>) -> Vec<Outcome> {
    let bounds: Vec<Option<Bounds>> = strokes.iter().map(|s| Bounds::of(*s)).collect();
    let x_height = typical_x_height(strokes, &bounds);
    let (writing, loose): (Vec<usize>, Vec<usize>) = (0..strokes.len())
        .filter(|&i| bounds[i].is_some())
        .partition(|&i| bounds[i].is_some_and(|b| is_writing(b, x_height)));

    let mut out: Vec<Outcome> = loose
        .into_iter()
        .filter_map(|i| fit_shape(strokes[i], true).map(|geom| Outcome::Shape { stroke: i, geom }))
        .collect();

    let writing_bounds: Vec<Bounds> = writing.iter().filter_map(|&i| bounds[i]).collect();
    let words: Vec<Vec<usize>> = group_words(&writing_bounds, x_height)
        .into_iter()
        .map(|members| members.into_iter().map(|k| writing[k]).collect())
        .collect();
    let word_bounds: Vec<Bounds> = words
        .iter()
        .filter_map(|w| w.iter().filter_map(|&i| bounds[i]).reduce(Bounds::union))
        .collect();
    for line in group_lines(&word_bounds) {
        let mut row: Option<Row> = None;
        for w in line {
            let members = &words[w];
            let ink: Vec<&[Pt]> = members.iter().map(|&i| strokes[i]).collect();
            let Some(measured) = WordMetrics::measure(&ink) else {
                continue;
            };
            let target = row.as_ref().map_or_else(
                || first_target(&measured, context),
                |row| Target {
                    baseline: Some(row.baseline),
                    x_height: row.x_height(),
                    slant: context.slant,
                    after: row
                        .gap()
                        .zip(row.last_word())
                        .map(|(gap, last)| (last.max_x, gap)),
                },
            );
            let Some(placed) = correct_word(&ink, &target) else {
                continue;
            };
            row.get_or_insert_with(|| Row::new(placed.baseline, target.baseline.is_some()))
                .place(placed.bounds, placed.x_height);
            out.push(Outcome::Word {
                strokes: members.clone(),
                fix: placed.fix,
            });
        }
    }
    out
}

/// Target of a line's first word: onto the sheet rule if one is close,
/// else levelled where it stands.
fn first_target(m: &WordMetrics, context: &Context<'_>) -> Target {
    let center = m.bounds.center();
    let own = m.baseline.y_at(center.x);
    let rule = context
        .rules
        .and_then(|rules| rules(center))
        .map(|spacing| (own / spacing).round() * spacing)
        .filter(|y| (y - own).abs() <= 0.6 * m.x_height);
    Target {
        baseline: rule.map(Line::horizontal),
        slant: context.slant,
        ..Target::default()
    }
}

/// X-height of the writing in the selection: the median over small
/// strokes grouped into words.
fn typical_x_height(strokes: &[&[Pt]], bounds: &[Option<Bounds>]) -> Option<f32> {
    let small: Vec<usize> = (0..strokes.len())
        .filter(|&i| bounds[i].is_some_and(|b| is_writing(b, None)))
        .collect();
    let small_bounds: Vec<Bounds> = small.iter().filter_map(|&i| bounds[i]).collect();
    let mut heights: Vec<f32> = group_words(&small_bounds, None)
        .into_iter()
        .filter_map(|members| {
            let ink: Vec<&[Pt]> = members.iter().map(|&k| strokes[small[k]]).collect();
            WordMetrics::measure(&ink).map(|m| m.x_height)
        })
        .collect();
    median(&mut heights)
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;
    use crate::testing::{Letter, word};

    #[test]
    fn small_o_in_a_word_stays_writing_and_a_big_circle_is_a_figure() {
        let mut ink = word(
            &[Letter::N, Letter::O, Letter::N],
            (0.0, 100.0),
            12.0,
            5.0,
            0.05,
            9,
        );
        let circle: Vec<Pt> = (0..=80u8)
            .map(|i| {
                let t = TAU * f32::from(i) / 80.0;
                Pt::new(
                    60.0f32.mul_add(t.cos(), 300.0),
                    60.0f32.mul_add(t.sin(), 100.0),
                )
            })
            .collect();
        ink.push(circle);
        let refs: Vec<&[Pt]> = ink.iter().map(Vec::as_slice).collect();
        let outcomes = beautify_all(&refs, &Context::default());
        let shapes: Vec<usize> = outcomes
            .iter()
            .filter_map(|o| match o {
                Outcome::Shape { stroke, .. } => Some(*stroke),
                Outcome::Word { .. } => None,
            })
            .collect();
        assert_eq!(shapes, vec![3], "only the big circle");
        assert!(
            outcomes
                .iter()
                .any(|o| matches!(o, Outcome::Word { strokes, .. } if strokes.contains(&1)))
        );
    }

    #[test]
    fn a_line_of_words_is_set_on_one_baseline() {
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
        ];
        let ink: Vec<Vec<Pt>> = words.into_iter().flatten().collect();
        let refs: Vec<&[Pt]> = ink.iter().map(Vec::as_slice).collect();
        let outcomes = beautify_all(&refs, &Context::default());
        let fixed: usize = outcomes
            .iter()
            .map(|o| match o {
                Outcome::Word { strokes, .. } => strokes.len(),
                Outcome::Shape { .. } => 0,
            })
            .sum();
        assert_eq!(fixed, ink.len());
    }
}
