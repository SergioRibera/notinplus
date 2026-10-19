//! Diacritics recognized apart from the letter they sit on.
//!
//! An accent is a few resampled points out of [`crate::TRACE_POINTS`],
//! too little weight to tell `á` from `ú` when matched as one shape.
//! Splitting the sample into body and marks lets the body match the
//! plain letter and the mark match a handful of mark templates.

use crate::ink::{Bounds, Point};
use crate::template::Line;

/// Mark written above a letter body.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub enum Mark {
    /// Tittle of `i` / `j`.
    Dot,
    Acute,
    Diaeresis,
    Tilde,
}

/// `(base, mark) → composed`. Bases `ı` / `ȷ` are the dotless letters
/// left once the tittle is split off.
const COMPOSITIONS: [(char, Mark, char); 16] = [
    ('ı', Mark::Dot, 'i'),
    ('ȷ', Mark::Dot, 'j'),
    ('a', Mark::Acute, 'á'),
    ('e', Mark::Acute, 'é'),
    ('ı', Mark::Acute, 'í'),
    ('o', Mark::Acute, 'ó'),
    ('u', Mark::Acute, 'ú'),
    ('A', Mark::Acute, 'Á'),
    ('E', Mark::Acute, 'É'),
    ('I', Mark::Acute, 'Í'),
    ('O', Mark::Acute, 'Ó'),
    ('U', Mark::Acute, 'Ú'),
    ('n', Mark::Tilde, 'ñ'),
    ('N', Mark::Tilde, 'Ñ'),
    ('u', Mark::Diaeresis, 'ü'),
    ('U', Mark::Diaeresis, 'Ü'),
];

impl Mark {
    /// Letter drawn as `base` with this mark on top, if any.
    #[must_use]
    pub fn compose(self, base: char) -> Option<char> {
        COMPOSITIONS
            .iter()
            .find(|(b, mark, _)| *b == base && *mark == self)
            .map(|(_, _, composed)| *composed)
    }

    /// Dotless bases only exist inside the recognizer; a bare `ı`
    /// match is an `i` whose tittle merged into the stem.
    #[must_use]
    pub(crate) const fn surface(glyph: char) -> char {
        match glyph {
            'ı' => 'i',
            'ȷ' => 'j',
            other => other,
        }
    }

    /// Splits a composed letter into base and mark.
    #[must_use]
    pub fn decompose(glyph: char) -> Option<(char, Self)> {
        COMPOSITIONS
            .iter()
            .find(|(_, _, composed)| *composed == glyph)
            .map(|(base, mark, _)| (*base, *mark))
    }
}

/// Strokes of a sample split into the letter body and the marks above.
#[derive(Debug)]
pub(crate) struct Split<'a> {
    pub(crate) body: Vec<&'a [Point]>,
    pub(crate) marks: Vec<&'a [Point]>,
    /// Frame marks are measured in: "baseline" at the body top, unit =
    /// body height. Keeps a tittle (tiny, close) apart from an acute
    /// (taller, slanted) and two dots apart from one stroke.
    pub(crate) frame: Line,
}

impl<'a> Split<'a> {
    /// A mark must sit no lower than this fraction of the body's
    /// height below the body top.
    const TOP_BAND: f32 = 0.25;
    /// And be no taller than this fraction of the body: an acute over
    /// a short `i` stem is tall relative to it.
    const MAX_HEIGHT: f32 = 0.8;
    /// And keep at least this fraction of the body height away from it.
    /// Top bars of `E`, `T`, `F` sit just as high as accents but touch
    /// their stems; accents and tittles float.
    const MIN_GAP: f32 = 0.04;

    /// `None` when no stroke qualifies as a mark.
    pub(crate) fn of<S: AsRef<[Point]>>(strokes: &'a [S]) -> Option<Self> {
        let strokes: Vec<&[Point]> = strokes
            .iter()
            .map(AsRef::as_ref)
            .filter(|stroke| !stroke.is_empty())
            .collect();
        if strokes.len() < 2 {
            return None;
        }
        // The tallest stroke anchors the body.
        let bounds: Vec<Bounds> = strokes
            .iter()
            .filter_map(|stroke| Bounds::of(*stroke))
            .collect();
        let anchor = bounds
            .iter()
            .max_by(|a, b| a.height().total_cmp(&b.height()))?;
        let height = anchor.height().max(anchor.width());
        if height <= f32::EPSILON {
            return None;
        }
        let ceiling = Self::TOP_BAND.mul_add(height, anchor.min.y);
        let candidate: Vec<bool> = bounds
            .iter()
            .map(|bounds| bounds.max.y <= ceiling && bounds.height() <= Self::MAX_HEIGHT * height)
            .collect();
        let floats = |index: usize| {
            candidate[index]
                && strokes
                    .iter()
                    .zip(&candidate)
                    .filter(|(_, other)| !**other)
                    .all(|(other, _)| gap(strokes[index], other) > Self::MIN_GAP * height)
        };
        let (marks, body): (Vec<_>, Vec<_>) = strokes
            .iter()
            .enumerate()
            .partition(|(index, _)| floats(*index));
        if marks.is_empty() || body.is_empty() {
            return None;
        }
        let body_bounds = Bounds::of(body.iter().flat_map(|(_, stroke)| stroke.iter()))?;
        Some(Self {
            body: body.into_iter().map(|(_, stroke)| *stroke).collect(),
            marks: marks.into_iter().map(|(_, stroke)| *stroke).collect(),
            frame: Line {
                baseline: body_bounds.min.y,
                x_height: body_bounds.height().max(body_bounds.width()),
            },
        })
    }
}

/// Smallest distance between two polylines.
fn gap(a: &[Point], b: &[Point]) -> f32 {
    let to_polyline = |point: Point, line: &[Point]| match line {
        [single] => point.distance(*single),
        _ => line
            .windows(2)
            .map(|pair| point.distance_to_segment(pair[0], pair[1]))
            .fold(f32::INFINITY, f32::min),
    };
    let one_way = |from: &[Point], to: &[Point]| {
        from.iter()
            .map(|point| to_polyline(*point, to))
            .fold(f32::INFINITY, f32::min)
    };
    one_way(a, b).min(one_way(b, a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touching_top_bar_is_not_a_mark() {
        let stem = vec![Point::new(50.0, 0.0), Point::new(50.0, 100.0)];
        let bar = vec![Point::new(0.0, 0.0), Point::new(100.0, 0.0)];
        assert!(Split::of(&[stem, bar]).is_none());
    }

    #[test]
    fn composition_round_trips() {
        for glyph in ['á', 'Ü', 'ñ', 'i', 'j', 'Í'] {
            let (base, mark) = Mark::decompose(glyph).unwrap();
            assert_eq!(mark.compose(base), Some(glyph));
        }
    }

    #[test]
    fn tittle_is_split_off_and_period_is_not() {
        let stem = vec![Point::new(0.0, 0.0), Point::new(0.0, 100.0)];
        let above = vec![Point::new(0.0, -30.0)];
        let below = vec![Point::new(0.0, 130.0)];
        let dotted = [stem.clone(), above];
        let split = Split::of(&dotted).unwrap();
        assert_eq!((split.body.len(), split.marks.len()), (1, 1));
        assert!(Split::of(&[stem, below]).is_none());
    }
}
