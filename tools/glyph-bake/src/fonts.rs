//! Font outlines → [`FontInk`].

use std::path::Path;

use notein_handwriting::{FontInk, GlyphInk, Verb};
use ttf_parser::{Face, OutlineBuilder, name_id};

use crate::error::BakeError;

/// Collects verbs and font-unit coordinates, Y flipped to grow
/// downwards.
#[derive(Debug, Default)]
struct Pen {
    verbs: Vec<Verb>,
    coords: Vec<[i16; 2]>,
}

impl Pen {
    #[allow(
        clippy::cast_possible_truncation,
        reason = "outline coordinates are font units, well inside i16"
    )]
    fn push(&mut self, verb: Verb, points: &[(f32, f32)]) {
        self.verbs.push(verb);
        self.coords.extend(
            points
                .iter()
                .map(|(x, y)| [x.round() as i16, (-y).round() as i16]),
        );
    }
}

impl OutlineBuilder for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.push(Verb::Move, &[(x, y)]);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.push(Verb::Line, &[(x, y)]);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.push(Verb::Quad, &[(x1, y1), (x, y)]);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.push(Verb::Cubic, &[(x1, y1), (x2, y2), (x, y)]);
    }

    fn close(&mut self) {
        self.push(Verb::Close, &[]);
    }
}

/// Bakes `charset` from the font at `path`. Returns the ink and the
/// characters the font lacks.
pub(crate) fn bake(
    path: &Path,
    charset: impl Iterator<Item = char>,
) -> Result<(FontInk, Vec<char>), BakeError> {
    let data = std::fs::read(path).map_err(BakeError::io(path))?;
    let face = Face::parse(&data, 0).map_err(|source| BakeError::Font {
        path: path.to_path_buf(),
        source,
    })?;
    let top_of = |glyph: char| {
        face.glyph_index(glyph)
            .and_then(|id| face.glyph_bounding_box(id))
            .map_or(0, |rect| rect.y_max)
    };
    let mut glyphs = Vec::new();
    let mut missing = Vec::new();
    for glyph in charset {
        let mut pen = Pen::default();
        let Some((id, rect)) = face
            .glyph_index(glyph)
            .and_then(|id| Some((id, face.outline_glyph(id, &mut pen)?)))
        else {
            missing.push(glyph);
            continue;
        };
        glyphs.push(GlyphInk {
            glyph,
            advance: i16::try_from(face.glyph_hor_advance(id).unwrap_or_default())
                .unwrap_or(i16::MAX),
            min: [rect.x_min, -rect.y_max],
            max: [rect.x_max, -rect.y_min],
            verbs: pen.verbs,
            coords: pen.coords,
        });
    }
    glyphs.sort_by_key(|ink| ink.glyph);
    let name = face
        .names()
        .into_iter()
        .filter(|name| name.name_id == name_id::FAMILY)
        .find_map(|name| name.to_string())
        .unwrap_or_else(|| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let ink = FontInk {
        format: FontInk::FORMAT,
        name,
        units_per_em: face.units_per_em(),
        ascender: face.ascender(),
        descender: face.descender().saturating_neg(),
        x_height: face.x_height().unwrap_or_else(|| top_of('x')),
        cap_height: face.capital_height().unwrap_or_else(|| top_of('H')),
        glyphs,
    };
    Ok((ink, missing))
}

/// `GochiHand` → `gochi-hand`.
pub(crate) fn kebab(stem: &str) -> String {
    let mut out = String::with_capacity(stem.len() + 4);
    for (index, ch) in stem.chars().enumerate() {
        if ch.is_uppercase() && index > 0 {
            out.push('-');
        }
        out.extend(ch.to_lowercase());
    }
    out
}
