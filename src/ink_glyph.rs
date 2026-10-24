//! Handwritten letters redrawn from a font ("smooth handwriting").
//!
//! An [`InkGlyph`] is a recognized character placed on a layer. It is
//! drawn as a filled outline in the pen colour, from the baked
//! [`FontInk`] of its [`GlyphFont`], so it stays ink: erasable,
//! coloured, layered like strokes.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use freya_engine::prelude::{Path, PathBuilder};
use notein_handwriting::{FontInk, PathCmd};

/// Globally unique glyph identifier.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct InkGlyphId(pub [u8; 16]);

impl InkGlyphId {
    #[must_use]
    pub fn new_v4() -> Self {
        Self(*uuid::Uuid::new_v4().as_bytes())
    }
}

/// Fonts recognized letters can be redrawn in.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub enum GlyphFont {
    #[default]
    Excalifont,
    Caveat,
    GochiHand,
    Handlee,
}

impl GlyphFont {
    pub const ALL: [Self; 4] = [
        Self::Excalifont,
        Self::Caveat,
        Self::GochiHand,
        Self::Handlee,
    ];

    /// Human readable name, for settings.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Excalifont => "Excalifont",
            Self::Caveat => "Caveat",
            Self::GochiHand => "Gochi Hand",
            Self::Handlee => "Handlee",
        }
    }

    /// Font new glyphs are drawn in, shared by every canvas.
    #[must_use]
    pub fn selected() -> Self {
        Self::ALL
            .get(usize::from(SELECTED.load(Ordering::Relaxed)))
            .copied()
            .unwrap_or_default()
    }

    /// Change [`Self::selected`]. Persisting it is the caller's call.
    pub fn select(self) {
        #[allow(clippy::cast_possible_truncation, reason = "four variants")]
        SELECTED.store(self.index() as u8, Ordering::Relaxed);
    }

    const fn index(self) -> usize {
        match self {
            Self::Excalifont => 0,
            Self::Caveat => 1,
            Self::GochiHand => 2,
            Self::Handlee => 3,
        }
    }

    const fn bytes(self) -> &'static [u8] {
        match self {
            Self::Excalifont => include_bytes!("../assets/handwriting/fonts/excalifont.rkyv"),
            Self::Caveat => include_bytes!("../assets/handwriting/fonts/caveat.rkyv"),
            Self::GochiHand => include_bytes!("../assets/handwriting/fonts/gochi-hand.rkyv"),
            Self::Handlee => include_bytes!("../assets/handwriting/fonts/handlee.rkyv"),
        }
    }

    /// Baked outlines, decoded once. `None` (logged) if the bundled
    /// archive is stale; re-run `glyph-bake fonts`.
    #[must_use]
    pub fn ink(self) -> Option<&'static FontInk> {
        static INKS: [OnceLock<Option<FontInk>>; 4] = [const { OnceLock::new() }; 4];
        INKS[self.index()]
            .get_or_init(|| {
                FontInk::from_bytes(self.bytes())
                    .inspect_err(|err| log::error!("font ink {}: {err}", self.name()))
                    .ok()
            })
            .as_ref()
    }
}

impl GlyphFont {
    /// `text` laid out on one line, its ink fitted to `height` inside
    /// `(x, y)`, for previews of the font itself. Returns the outline
    /// and its width.
    #[must_use]
    pub fn sample(self, text: &str, (x, y): (f32, f32), height: f32) -> Option<(Path, f32)> {
        let ink = self.ink()?;
        // Y grows downwards: `min` is the top of the tallest letter.
        let (top, bottom) = text
            .chars()
            .filter_map(|c| ink.glyph(c))
            .fold((i16::MAX, i16::MIN), |(top, bottom), g| {
                (top.min(g.min[1]), bottom.max(g.max[1]))
            });
        let span = f32::from(bottom) - f32::from(top);
        if span <= 0.0 {
            return None;
        }
        let scale = height / span;
        let baseline = f32::from(top).mul_add(-scale, y);
        let space = f32::from(ink.units_per_em) / 4.0;
        let mut builder = PathBuilder::new();
        let mut pen = x;
        for glyph in text.chars() {
            let Some(outline) = ink.glyph(glyph) else {
                pen = space.mul_add(scale, pen);
                continue;
            };
            let glyph = InkGlyph {
                id: InkGlyphId::default(),
                glyph,
                font: self,
                x: pen,
                y: baseline,
                scale,
                color: [0; 4],
            };
            if let Some(path) = glyph.path() {
                builder.add_path(&path, None);
            }
            pen = f32::from(outline.advance).mul_add(scale, pen);
        }
        Some((builder.detach(), pen - x))
    }
}

static SELECTED: AtomicU8 = AtomicU8::new(0);

/// A recognized character drawn as ink.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct InkGlyph {
    pub id: InkGlyphId,
    pub glyph: char,
    pub font: GlyphFont,
    /// World position of the glyph origin (left end of the baseline).
    pub x: f32,
    pub y: f32,
    /// World units per font unit.
    pub scale: f32,
    pub color: [u8; 4],
}

impl InkGlyph {
    /// World-space `(min_x, min_y, max_x, max_y)` of the outline.
    #[must_use]
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let ink = self.font.ink()?.glyph(self.glyph)?;
        let world = |[x, y]: [i16; 2]| {
            (
                f32::from(x).mul_add(self.scale, self.x),
                f32::from(y).mul_add(self.scale, self.y),
            )
        };
        let (min_x, min_y) = world(ink.min);
        let (max_x, max_y) = world(ink.max);
        Some((min_x, min_y, max_x, max_y))
    }

    /// Filled outline in world units.
    #[must_use]
    pub fn path(&self) -> Option<Path> {
        let ink = self.font.ink()?.glyph(self.glyph)?;
        let world = |point: notein_handwriting::Point| (point.x + self.x, point.y + self.y);
        let mut builder = PathBuilder::new();
        for cmd in ink.commands(self.scale) {
            match cmd {
                PathCmd::MoveTo(p) => {
                    builder.move_to(world(p));
                }
                PathCmd::LineTo(p) => {
                    builder.line_to(world(p));
                }
                PathCmd::QuadTo(c, p) => {
                    builder.quad_to(world(c), world(p));
                }
                PathCmd::CubicTo(c1, c2, p) => {
                    builder.cubic_to(world(c1), world(c2), world(p));
                }
                PathCmd::Close => {
                    builder.close();
                }
            }
        }
        Some(builder.detach())
    }

    /// Whether the outline's bounds come within `radius` of `(x, y)`.
    #[must_use]
    pub fn touches_circle(&self, x: f32, y: f32, radius: f32) -> bool {
        self.bounds().is_some_and(|(x0, y0, x1, y1)| {
            let dx = x - x.clamp(x0, x1);
            let dy = y - y.clamp(y0, y1);
            dx.hypot(dy) <= radius
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_fonts_cover_the_alphabet() {
        for font in GlyphFont::ALL {
            let ink = font.ink().expect("bundled font ink decodes");
            for glyph in ['a', 'Z', '0', 'ñ', '¿', '€'] {
                assert!(ink.glyph(glyph).is_some(), "{} lacks {glyph}", font.name());
            }
        }
    }
}
