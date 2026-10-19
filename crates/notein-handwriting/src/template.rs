//! Recognition templates and their baked container.

use rkyv::{Archive, Deserialize, Serialize};

use crate::archive;
use crate::error::HandwritingError;
use crate::ink::{Bounds, Point, TRACE_POINTS, Trace};
use crate::marks::{Mark, Split};

/// Writing line a sample sits on, in the sample's own units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    /// Y of the baseline.
    pub baseline: f32,
    /// Distance from the baseline up to the top of a lowercase `x`.
    pub x_height: f32,
}

/// Where a sample sits relative to its [`Line`], in x-height units.
/// Y grows downwards with the baseline at 0, so `top` is negative for
/// anything rising above the baseline.
///
/// Traces are scaled to unit size, which erases the only cue that
/// separates `o`/`O` or `,`/`'`; the extent brings it back when the
/// caller knows the line.
#[derive(Debug, Clone, Copy, PartialEq, Default, Archive, Serialize, Deserialize)]
pub struct Extent {
    pub top: f32,
    pub bottom: f32,
    pub width: f32,
}

impl Extent {
    /// Measures `strokes` against `line`. `None` when there are no
    /// points or the line has no height.
    #[must_use]
    pub fn measure<S: AsRef<[Point]>>(strokes: &[S], line: Line) -> Option<Self> {
        if line.x_height <= f32::EPSILON {
            return None;
        }
        let bounds = Bounds::of(strokes.iter().flat_map(AsRef::as_ref))?;
        Some(Self {
            top: (bounds.min.y - line.baseline) / line.x_height,
            bottom: (bounds.max.y - line.baseline) / line.x_height,
            width: bounds.width() / line.x_height,
        })
    }

    /// Dissimilarity between two placements. Vertical placement is
    /// what separates case pairs; width only nudges.
    #[must_use]
    pub fn gap(self, other: Self) -> f32 {
        0.5f32.mul_add(
            (self.width - other.width).abs(),
            (self.top - other.top).abs() + (self.bottom - other.bottom).abs(),
        )
    }
}

/// What a template stands for.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Archive, Serialize, Deserialize,
)]
pub enum Label {
    /// A whole character, or the base of a composed one (`a` for `á`,
    /// dotless `ı` for `i`).
    Glyph(char),
    /// A diacritic on its own.
    Mark(Mark),
}

/// One reference sample.
#[derive(Debug, Clone, PartialEq, Archive, Serialize, Deserialize)]
pub struct Template {
    pub label: Label,
    /// Normalized trace points, quantized by [`Template::QUANTUM`].
    pub points: [[i16; 2]; TRACE_POINTS],
    /// Stroke starts bitmask, see [`Trace::starts`].
    pub starts: u32,
    pub extent: Extent,
}

impl Template {
    /// Normalized coordinates stay within ±1, so 2¹⁴ steps per unit
    /// keep sub-0.0001 precision in half the bytes of `f32`.
    pub const QUANTUM: f32 = 16384.0;

    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "normalized coordinates are within ±2, far inside i16 once quantized"
    )]
    pub fn new(label: Label, trace: &Trace, extent: Extent) -> Self {
        let quantize = |value: f32| (value * Self::QUANTUM).round() as i16;
        Self {
            label,
            points: trace
                .points()
                .map(|point| [quantize(point.x), quantize(point.y)]),
            starts: trace.starts(),
            extent,
        }
    }

    /// The whole sample as one template.
    #[must_use]
    pub fn whole<S: AsRef<[Point]>>(glyph: char, strokes: &[S], line: Line) -> Option<Self> {
        Some(Self::new(
            Label::Glyph(glyph),
            &Trace::from_strokes(strokes)?,
            Extent::measure(strokes, line)?,
        ))
    }

    /// Templates for a labelled sample: base + mark when `glyph` is a
    /// composed letter and its marks split off cleanly, the whole
    /// sample otherwise. Mirrors how [`crate::Recognizer`] reads ink.
    #[must_use]
    pub fn extract<S: AsRef<[Point]>>(glyph: char, strokes: &[S], line: Line) -> Vec<Self> {
        let decomposed = Mark::decompose(glyph).and_then(|(base, mark)| {
            let split = Split::of(strokes)?;
            Some(vec![
                Self::new(
                    Label::Glyph(base),
                    &Trace::from_strokes(&split.body)?,
                    Extent::measure(&split.body, line)?,
                ),
                Self::new(
                    Label::Mark(mark),
                    &Trace::from_strokes(&split.marks)?,
                    Extent::measure(&split.marks, split.frame)?,
                ),
            ])
        });
        decomposed
            .or_else(|| Self::whole(glyph, strokes, line).map(|template| vec![template]))
            .unwrap_or_default()
    }

    /// Rebuilds the trace for matching.
    #[must_use]
    pub fn trace(&self) -> Trace {
        let points = self
            .points
            .map(|[x, y]| Point::new(f32::from(x) / Self::QUANTUM, f32::from(y) / Self::QUANTUM));
        Trace::from_normalized(points, self.starts)
    }
}

/// Everything a recognizer loads. Produced by `tools/glyph-bake`, or
/// by any caller with its own samples.
#[derive(Debug, Clone, Default, PartialEq, Archive, Serialize, Deserialize)]
pub struct TemplateSet {
    pub format: u32,
    pub templates: Vec<Template>,
}

impl archive::Versioned for TemplateSet {
    const FORMAT: u32 = Self::FORMAT;

    fn format(&self) -> u32 {
        self.format
    }
}

impl TemplateSet {
    /// Bumped whenever the archived layout or the normalization in
    /// [`Trace`] changes; stale bakes are rejected on load.
    pub const FORMAT: u32 = 2;

    #[must_use]
    pub const fn new(templates: Vec<Template>) -> Self {
        Self {
            format: Self::FORMAT,
            templates,
        }
    }

    /// Decodes an archive produced by [`Self::to_bytes`]. Any
    /// alignment is accepted.
    ///
    /// # Errors
    ///
    /// [`HandwritingError::Archive`] for bytes that are not a valid
    /// archive, [`HandwritingError::Format`] for one baked by an
    /// incompatible version.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, HandwritingError> {
        archive::decode(bytes)
    }

    /// Serializes for [`Self::from_bytes`].
    ///
    /// # Errors
    ///
    /// [`HandwritingError::Archive`] if rkyv fails to serialize.
    pub fn to_bytes(&self) -> Result<Vec<u8>, HandwritingError> {
        Ok(archive::encode(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_round_trips_from_unaligned_bytes() {
        let stem = vec![Point::new(0.0, 0.0), Point::new(0.0, 100.0)];
        let line = Line {
            baseline: 100.0,
            x_height: 50.0,
        };
        let set = TemplateSet::new(Template::whole('l', &[stem], line).into_iter().collect());
        let bytes = set.to_bytes().unwrap();
        let mut shifted = vec![0u8];
        shifted.extend_from_slice(&bytes);
        assert_eq!(TemplateSet::from_bytes(&shifted[1..]).unwrap(), set);
    }

    #[test]
    fn stale_format_is_rejected() {
        let set = TemplateSet {
            format: TemplateSet::FORMAT + 1,
            templates: Vec::new(),
        };
        let bytes = set.to_bytes().unwrap();
        assert!(matches!(
            TemplateSet::from_bytes(&bytes),
            Err(HandwritingError::Format { .. })
        ));
    }
}
