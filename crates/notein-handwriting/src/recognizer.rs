//! Character recognizer over a [`TemplateSet`].

use crate::error::HandwritingError;
use crate::ink::{Point, Trace};
use crate::marks::{Mark, Split};
use crate::template::{Extent, Label, Line, Template, TemplateSet};

/// One ranked guess.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub glyph: char,
    /// DTW distance to the closest template(s); lower is better. Use
    /// it to reject scribbles that resemble no character.
    pub distance: f32,
    /// `distance` adjusted by placement and composition weights;
    /// candidates are sorted by it.
    pub score: f32,
}

#[derive(Debug)]
struct Sample {
    trace: Trace,
    extent: Extent,
}

#[derive(Debug)]
struct Class<L> {
    label: L,
    samples: Vec<Sample>,
}

/// Penalties added on top of the DTW distance.
#[derive(Debug, Clone, Copy, Default)]
struct Weights {
    /// Per unit of [`Extent::gap`].
    extent: f32,
    /// Per stroke of difference between query and template.
    strokes: f32,
}

#[derive(Debug, Clone, Copy)]
struct Ranked<L> {
    label: L,
    distance: f32,
    score: f32,
}

/// Templates grouped by label.
#[derive(Debug)]
struct Bank<L> {
    classes: Vec<Class<L>>,
}

impl<L> Default for Bank<L> {
    fn default() -> Self {
        Self {
            classes: Vec::new(),
        }
    }
}

impl<L: Copy + PartialEq> Bank<L> {
    fn insert(&mut self, label: L, sample: Sample) {
        match self.classes.iter_mut().find(|class| class.label == label) {
            Some(class) => class.samples.push(sample),
            None => self.classes.push(Class {
                label,
                samples: vec![sample],
            }),
        }
    }

    /// The `limit` closest labels, best first. `extent` adds a
    /// placement penalty; stroke count differences add another.
    fn rank(
        &self,
        trace: &Trace,
        extent: Option<Extent>,
        weights: Weights,
        limit: usize,
    ) -> Vec<Ranked<L>> {
        let strokes = trace.starts().count_ones();
        let mut ranked: Vec<Ranked<L>> = Vec::with_capacity(limit + 1);
        for class in &self.classes {
            // Once `limit` labels are held a class must beat the worst
            // of them; that bound abandons most DTW matches early.
            let mut bound = if ranked.len() < limit {
                f32::INFINITY
            } else {
                ranked.last().map_or(f32::INFINITY, |worst| worst.score)
            };
            let mut best = None;
            for sample in &class.samples {
                let extent_penalty =
                    extent.map_or(0.0, |extent| weights.extent * extent.gap(sample.extent));
                #[allow(clippy::cast_precision_loss, reason = "stroke counts are tiny")]
                let stroke_penalty =
                    weights.strokes * strokes.abs_diff(sample.trace.starts().count_ones()) as f32;
                let penalty = extent_penalty + stroke_penalty;
                let remaining = bound - penalty;
                if remaining <= 0.0 {
                    continue;
                }
                let distance = trace.distance(&sample.trace, remaining);
                if distance < remaining {
                    bound = distance + penalty;
                    best = Some(Ranked {
                        label: class.label,
                        distance,
                        score: bound,
                    });
                }
            }
            if let Some(best) = best {
                let at = ranked.partition_point(|held| held.score <= best.score);
                ranked.insert(at, best);
                ranked.truncate(limit);
            }
        }
        ranked
    }
}

/// Nearest-template classifier with diacritics composed on top.
#[derive(Debug)]
pub struct Recognizer {
    glyphs: Bank<char>,
    marks: Bank<Mark>,
    extent_weight: f32,
    mark_weight: f32,
}

impl Default for Recognizer {
    fn default() -> Self {
        Self {
            glyphs: Bank::default(),
            marks: Bank::default(),
            extent_weight: Self::EXTENT_WEIGHT,
            mark_weight: Self::MARK_WEIGHT,
        }
    }
}

impl TryFrom<TemplateSet> for Recognizer {
    type Error = HandwritingError;

    fn try_from(set: TemplateSet) -> Result<Self, Self::Error> {
        if set.format != TemplateSet::FORMAT {
            return Err(HandwritingError::Format {
                expected: TemplateSet::FORMAT,
                found: set.format,
            });
        }
        let mut recognizer = Self::default();
        for template in &set.templates {
            recognizer.insert(template);
        }
        Ok(recognizer)
    }
}

impl Recognizer {
    /// Default weight of the [`Extent::gap`] term when a [`Line`] is
    /// known. Tuned with `glyph-bake eval`.
    pub const EXTENT_WEIGHT: f32 = 1.0;
    /// Default weight of the mark distance in a composed candidate.
    pub const MARK_WEIGHT: f32 = 0.25;
    /// Weight of a mark's placement over its body (see
    /// [`Split::frame`](crate::marks::Split)) against its shape.
    const MARK_WEIGHTS: Weights = Weights {
        extent: 1.0,
        // Two dots against one acute stroke is the main diaeresis cue.
        strokes: 0.5,
    };
    /// Body labels tried when composing with marks.
    const BODY_SHORTLIST: usize = 6;
    const MARK_SHORTLIST: usize = 2;

    /// Decodes a [`TemplateSet`] archive and loads it.
    ///
    /// # Errors
    ///
    /// As [`TemplateSet::from_bytes`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, HandwritingError> {
        Self::try_from(TemplateSet::from_bytes(bytes)?)
    }

    #[must_use]
    pub const fn with_extent_weight(mut self, weight: f32) -> Self {
        self.extent_weight = weight;
        self
    }

    #[must_use]
    pub const fn with_mark_weight(mut self, weight: f32) -> Self {
        self.mark_weight = weight;
        self
    }

    const fn glyph_weights(&self) -> Weights {
        Weights {
            extent: self.extent_weight,
            strokes: 0.0,
        }
    }

    /// Adds a reference sample.
    pub fn insert(&mut self, template: &Template) {
        let sample = Sample {
            trace: template.trace(),
            extent: template.extent,
        };
        match template.label {
            Label::Glyph(glyph) => self.glyphs.insert(glyph, sample),
            Label::Mark(mark) => self.marks.insert(mark, sample),
        }
    }

    /// The `limit` most likely characters for `strokes`, best first.
    /// With a `line`, placement relative to it helps separate shapes
    /// that only differ in size (`o`/`O`, `,`/`'`). Empty when the
    /// strokes hold no points.
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("handwriting::recognize"))]
    pub fn recognize<S: AsRef<[Point]>>(
        &self,
        strokes: &[S],
        line: Option<Line>,
        limit: usize,
    ) -> Vec<Candidate> {
        let Some(trace) = Trace::from_strokes(strokes) else {
            return Vec::new();
        };
        let measure = |strokes: &[&[Point]]| line.and_then(|line| Extent::measure(strokes, line));
        let all: Vec<&[Point]> = strokes.iter().map(AsRef::as_ref).collect();
        let mut candidates: Vec<Candidate> = self
            .glyphs
            .rank(&trace, measure(&all), self.glyph_weights(), limit)
            .into_iter()
            .map(|ranked| Candidate {
                glyph: Mark::surface(ranked.label),
                distance: ranked.distance,
                score: ranked.score,
            })
            .collect();
        if let Some(split) = Split::of(strokes) {
            candidates.extend(self.composed(&split, measure(&split.body)));
        }
        candidates.sort_by(|a, b| a.score.total_cmp(&b.score));
        let mut seen = Vec::with_capacity(limit);
        candidates.retain(|candidate| {
            let fresh = !seen.contains(&candidate.glyph);
            seen.push(candidate.glyph);
            fresh
        });
        candidates.truncate(limit);
        candidates
    }

    /// Body × mark combinations that form a known letter.
    fn composed(&self, split: &Split<'_>, body_extent: Option<Extent>) -> Vec<Candidate> {
        let (Some(body), Some(marks)) = (
            Trace::from_strokes(&split.body),
            Trace::from_strokes(&split.marks),
        ) else {
            return Vec::new();
        };
        let bodies = self.glyphs.rank(
            &body,
            body_extent,
            self.glyph_weights(),
            Self::BODY_SHORTLIST,
        );
        let mark_extent = Extent::measure(&split.marks, split.frame);
        let marks = self.marks.rank(
            &marks,
            mark_extent,
            Self::MARK_WEIGHTS,
            Self::MARK_SHORTLIST,
        );
        bodies
            .iter()
            .flat_map(|body| marks.iter().map(move |mark| (body, mark)))
            .filter_map(|(body, mark)| {
                Some(Candidate {
                    glyph: mark.label.compose(body.label)?,
                    distance: body.distance + mark.distance,
                    score: self.mark_weight.mul_add(mark.score, body.score),
                })
            })
            .collect()
    }
}
