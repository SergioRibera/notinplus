//! Font glyph outlines baked for redrawing recognized characters as
//! ink.
//!
//! Stored the way TrueType and Skia store paths: one byte per verb and
//! `i16` font-unit coordinates, Y flipped to grow downwards with the
//! baseline at 0.

use rkyv::{Archive, Deserialize, Serialize};

use crate::archive;
use crate::error::HandwritingError;
use crate::ink::Point;

/// Path verb; consumes [`Verb::points`] coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub enum Verb {
    Move,
    Line,
    Quad,
    Cubic,
    Close,
}

impl Verb {
    #[must_use]
    pub const fn points(self) -> usize {
        match self {
            Self::Move | Self::Line => 1,
            Self::Quad => 2,
            Self::Cubic => 3,
            Self::Close => 0,
        }
    }
}

/// Outline command in caller units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCmd {
    MoveTo(Point),
    LineTo(Point),
    QuadTo(Point, Point),
    CubicTo(Point, Point, Point),
    Close,
}

/// Outline of one character, in font units.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct GlyphInk {
    pub glyph: char,
    pub advance: i16,
    pub min: [i16; 2],
    pub max: [i16; 2],
    pub verbs: Vec<Verb>,
    pub coords: Vec<[i16; 2]>,
}

impl GlyphInk {
    /// Commands with every coordinate multiplied by `scale` (e.g.
    /// [`FontInk::scale_for_x_height`]). Filled with non-zero winding.
    pub fn commands(&self, scale: f32) -> impl Iterator<Item = PathCmd> + '_ {
        let mut coords = self
            .coords
            .iter()
            .map(move |[x, y]| Point::new(f32::from(*x) * scale, f32::from(*y) * scale));
        self.verbs.iter().map_while(move |verb| {
            Some(match verb {
                Verb::Move => PathCmd::MoveTo(coords.next()?),
                Verb::Line => PathCmd::LineTo(coords.next()?),
                Verb::Quad => PathCmd::QuadTo(coords.next()?, coords.next()?),
                Verb::Cubic => PathCmd::CubicTo(coords.next()?, coords.next()?, coords.next()?),
                Verb::Close => PathCmd::Close,
            })
        })
    }
}

/// A font's outlines for every recognizable character.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct FontInk {
    pub format: u32,
    pub name: String,
    pub units_per_em: u16,
    /// Positive distance above the baseline.
    pub ascender: i16,
    /// Positive distance below the baseline.
    pub descender: i16,
    pub x_height: i16,
    pub cap_height: i16,
    /// Sorted by `glyph`.
    pub glyphs: Vec<GlyphInk>,
}

impl archive::Versioned for FontInk {
    const FORMAT: u32 = Self::FORMAT;

    fn format(&self) -> u32 {
        self.format
    }
}

impl FontInk {
    /// Bumped whenever the archived layout changes.
    pub const FORMAT: u32 = 2;

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

    /// Outline for `glyph`, if the font has it.
    #[must_use]
    pub fn glyph(&self, glyph: char) -> Option<&GlyphInk> {
        self.glyphs
            .binary_search_by_key(&glyph, |ink| ink.glyph)
            .ok()
            .map(|index| &self.glyphs[index])
    }

    /// Scale from font units to a target x-height (in caller units).
    #[must_use]
    pub fn scale_for_x_height(&self, x_height: f32) -> f32 {
        x_height / f32::from(self.x_height.max(1))
    }
}
