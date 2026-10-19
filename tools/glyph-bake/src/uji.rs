//! Reader for `ujipenchars2.txt` (`UJIpenchars2`, CC BY 4.0,
//! <https://doi.org/10.24432/C5FG8S>).
//!
//! Each sample was written inside a 13.6 × 20.4 mm box with guides at
//! 7.5 mm (x-height) and 12.7 mm (baseline) from the top, origin at
//! the box's top-left corner. UJI recorded 100 ink units per mm, UPV
//! 152, so the guides are rebuilt per site.

use std::path::Path;

use notein_handwriting::{Line, Point};

use crate::error::BakeError;

/// The 97 characters in the dataset, i.e. everything the recognizer
/// can answer with.
pub(crate) const GLYPHS: &str = "!\"$%'(),-.:;<>?@0123456789\
    ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz\
    ¡¿ªº€ÁÉÍÑÓÚÜáéíñóúü";

const X_HEIGHT_GUIDE_MM: f32 = 7.5;
const BASELINE_GUIDE_MM: f32 = 12.7;

/// The dataset's own writer split, kept for writer-independent
/// evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Split {
    Train,
    Test,
}

#[derive(Debug, Clone)]
pub(crate) struct Sample {
    pub(crate) glyph: char,
    pub(crate) split: Split,
    pub(crate) strokes: Vec<Vec<Point>>,
    pub(crate) line: Line,
}

/// Session id like `trn_UPV_W23-02`.
struct Session {
    split: Split,
    units_per_mm: f32,
}

impl Session {
    fn parse(id: &str) -> Option<Self> {
        let mut parts = id.split('_');
        let split = match parts.next()? {
            "trn" => Split::Train,
            "tst" => Split::Test,
            _ => return None,
        };
        let units_per_mm = match parts.next()? {
            "UJI" => 100.0,
            "UPV" => 152.0,
            _ => return None,
        };
        Some(Self {
            split,
            units_per_mm,
        })
    }

    fn line(&self) -> Line {
        Line {
            baseline: BASELINE_GUIDE_MM * self.units_per_mm,
            x_height: (BASELINE_GUIDE_MM - X_HEIGHT_GUIDE_MM) * self.units_per_mm,
        }
    }
}

pub(crate) fn load(path: &Path) -> Result<Vec<Sample>, BakeError> {
    let text = std::fs::read_to_string(path).map_err(BakeError::io(path))?;
    let malformed = |line: usize, reason| BakeError::Malformed {
        path: path.to_path_buf(),
        line,
        reason,
    };
    let mut samples: Vec<Sample> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("WORD") => {
                let mut glyph = fields
                    .next()
                    .ok_or_else(|| malformed(number, "missing character"))?
                    .chars();
                let (Some(glyph), None) = (glyph.next(), glyph.next()) else {
                    return Err(malformed(number, "label is not a single character"));
                };
                let session = fields
                    .next()
                    .and_then(Session::parse)
                    .ok_or_else(|| malformed(number, "unrecognized session id"))?;
                samples.push(Sample {
                    glyph,
                    split: session.split,
                    strokes: Vec::new(),
                    line: session.line(),
                });
            }
            Some("NUMSTROKES") => {}
            Some("POINTS") => {
                let sample = samples
                    .last_mut()
                    .ok_or_else(|| malformed(number, "POINTS before WORD"))?;
                let coords: Vec<f32> = fields
                    .skip_while(|field| *field != "#")
                    .skip(1)
                    .map(str::parse::<f32>)
                    .collect::<Result<_, _>>()
                    .map_err(|_| malformed(number, "non-numeric coordinate"))?;
                if coords.len() % 2 != 0 {
                    return Err(malformed(number, "odd number of coordinates"));
                }
                sample.strokes.push(
                    coords
                        .chunks_exact(2)
                        .map(|xy| Point::new(xy[0], xy[1]))
                        .collect(),
                );
            }
            _ => return Err(malformed(number, "unknown record")),
        }
    }
    Ok(samples)
}
