//! Strokes → words → lines.

use crate::metrics::{Bounds, Line, median};

/// A stroke joins the open word across a horizontal gap up to this
/// share of the x-height.
const WORD_GAP: f32 = 0.6;
/// Vertical slack above the open word, in x-heights: dots and accents.
const ABOVE_SLACK: f32 = 1.4;
/// Vertical slack below the open word, in x-heights.
const BELOW_SLACK: f32 = 0.6;
/// Handwriting is at most this many x-heights tall (ascender to
/// descender with room to spare); taller ink is drawing.
const WRITING_HEIGHT: f32 = 3.0;
/// Without an x-height yet, ink taller than this (world units) is drawing.
const WRITING_HEIGHT_FALLBACK: f32 = 120.0;
/// Two words share a line when their vertical bands overlap by this
/// share of the shorter one.
const LINE_OVERLAP: f32 = 0.5;

/// Whether a stroke with `stroke` bounds could be handwriting on a line
/// whose x-height is `x_height` (if known).
#[must_use]
pub fn is_writing(stroke: Bounds, x_height: Option<f32>) -> bool {
    let limit = x_height.map_or(WRITING_HEIGHT_FALLBACK, |h| WRITING_HEIGHT * h);
    stroke.height() <= limit && stroke.width() <= limit.max(WRITING_HEIGHT_FALLBACK) * 4.0
}

/// Whether a stroke with `stroke` bounds continues the word with `word`
/// bounds, written at `x_height`.
#[must_use]
pub fn joins_word(word: Bounds, stroke: Bounds, x_height: f32) -> bool {
    let gap = WORD_GAP * x_height;
    stroke.min_x <= word.max_x + gap
        && stroke.max_x >= word.min_x - gap
        && stroke.max_y >= ABOVE_SLACK.mul_add(-x_height, word.min_y)
        && stroke.min_y <= BELOW_SLACK.mul_add(x_height, word.max_y)
}

/// Strokes (bounds, in writing order) grouped into words. `x_height`
/// sizes the gaps; without one, each group's own height stands in.
#[must_use]
pub fn group_words(strokes: &[Bounds], x_height: Option<f32>) -> Vec<Vec<usize>> {
    let mut words: Vec<(Bounds, Vec<usize>)> = Vec::new();
    for (i, &stroke) in strokes.iter().enumerate() {
        let joined = words.iter_mut().rev().take(2).find(|(bounds, _)| {
            let h = x_height.unwrap_or_else(|| bounds.height().max(stroke.height()) * 0.5);
            joins_word(*bounds, stroke, h)
        });
        match joined {
            Some((bounds, members)) => {
                *bounds = bounds.union(stroke);
                members.push(i);
            }
            None => words.push((stroke, vec![i])),
        }
    }
    words.into_iter().map(|(_, members)| members).collect()
}

/// Whether two word bands share a line.
#[must_use]
pub fn same_line(a: Bounds, b: Bounds) -> bool {
    let overlap = a.max_y.min(b.max_y) - a.min_y.max(b.min_y);
    overlap >= LINE_OVERLAP * a.height().min(b.height())
}

/// Words (bounds) grouped into lines, each line left to right, lines
/// top to bottom.
#[must_use]
pub fn group_lines(words: &[Bounds]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..words.len()).collect();
    order.sort_by(|&a, &b| words[a].center().y.total_cmp(&words[b].center().y));
    let mut lines: Vec<(Bounds, Vec<usize>)> = Vec::new();
    for i in order {
        match lines
            .iter_mut()
            .find(|(band, _)| same_line(*band, words[i]))
        {
            Some((band, members)) => {
                *band = band.union(words[i]);
                members.push(i);
            }
            None => lines.push((words[i], vec![i])),
        }
    }
    lines.sort_by(|a, b| a.0.center().y.total_cmp(&b.0.center().y));
    lines
        .into_iter()
        .map(|(_, mut members)| {
            members.sort_by(|&a, &b| words[a].min_x.total_cmp(&words[b].min_x));
            members
        })
        .collect()
}

/// A line of handwriting as it builds up: where it sits, how tall and
/// how far apart its words are. Words already placed are never moved
/// again; the row only steers the next one.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Baseline every word of the row is set on.
    pub baseline: Line,
    /// Line of the sheet it follows, if the paper is ruled.
    pub ruled: bool,
    x_heights: Vec<f32>,
    gaps: Vec<f32>,
    /// Bounds of the placed words, as shown.
    words: Vec<Bounds>,
}

impl Row {
    /// A row starting with a word placed on `baseline`.
    #[must_use]
    pub const fn new(baseline: Line, ruled: bool) -> Self {
        Self {
            baseline,
            ruled,
            x_heights: Vec::new(),
            gaps: Vec::new(),
            words: Vec::new(),
        }
    }

    /// Record a word placed on the row, as shown.
    pub fn place(&mut self, bounds: Bounds, x_height: f32) {
        if let Some(prev) = self.last_word() {
            let gap = bounds.min_x - prev.max_x;
            if gap > 0.0 {
                self.gaps.push(gap);
            }
        }
        self.x_heights.push(x_height);
        self.words.push(bounds);
    }

    #[must_use]
    pub fn last_word(&self) -> Option<Bounds> {
        self.words.last().copied()
    }

    /// Median x-height of the placed words.
    #[must_use]
    pub fn x_height(&self) -> Option<f32> {
        median(&mut self.x_heights.clone())
    }

    /// Median gap between placed words; needs two gaps to mean anything.
    #[must_use]
    pub fn gap(&self) -> Option<f32> {
        (self.gaps.len() >= 2)
            .then(|| median(&mut self.gaps.clone()))
            .flatten()
    }

    /// Whether a word with `bounds` continues this row: next to the last
    /// word and around the baseline.
    #[must_use]
    pub fn accepts(&self, bounds: Bounds, x_height: f32) -> bool {
        let Some(last) = self.last_word() else {
            return false;
        };
        let h = self.x_height().unwrap_or(x_height).max(1.0);
        let base = self.baseline.y_at(bounds.min_x);
        let near_line =
            bounds.max_y >= 1.5f32.mul_add(-h, base) && bounds.max_y <= 1.5f32.mul_add(h, base);
        let after = bounds.min_x >= last.min_x && bounds.min_x <= 8.0f32.mul_add(h, last.max_x);
        near_line && after
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn b(min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> Bounds {
        Bounds {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    #[test]
    fn dot_above_joins_its_word_and_far_strokes_do_not() {
        let word = b(0.0, 10.0, 30.0, 20.0);
        assert!(joins_word(word, b(25.0, 0.0, 26.0, 1.0), 10.0));
        assert!(!joins_word(word, b(50.0, 10.0, 70.0, 20.0), 10.0));
        assert!(!joins_word(word, b(0.0, 60.0, 30.0, 70.0), 10.0));
    }

    #[test]
    fn words_and_lines() {
        let strokes = [
            b(0.0, 10.0, 10.0, 20.0),
            b(11.0, 10.0, 20.0, 20.0),
            b(40.0, 10.0, 50.0, 20.0),
            b(0.0, 50.0, 10.0, 60.0),
        ];
        let words = group_words(&strokes, Some(10.0));
        assert_eq!(words, vec![vec![0, 1], vec![2], vec![3]]);
        let bounds = [
            b(0.0, 10.0, 20.0, 20.0),
            b(40.0, 10.0, 50.0, 20.0),
            b(0.0, 50.0, 10.0, 60.0),
        ];
        assert_eq!(group_lines(&bounds), vec![vec![0, 1], vec![2]]);
    }

    #[test]
    fn big_ink_is_drawing() {
        assert!(is_writing(b(0.0, 0.0, 40.0, 25.0), Some(10.0)));
        assert!(!is_writing(b(0.0, 0.0, 40.0, 45.0), Some(10.0)));
        assert!(!is_writing(b(0.0, 0.0, 300.0, 300.0), None));
    }
}
