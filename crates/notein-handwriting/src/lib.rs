//! Handwritten character recognition and font ink for notinplus.
//!
//! Recognition matches dynamic-time-warped traces against a
//! [`TemplateSet`]; accents are split off and matched apart. Recognized
//! characters are redrawn from [`FontInk`] outlines so the result stays
//! ink on the canvas.
//!
//! The crate ships no data: callers decode whatever archives they
//! bundle or download (`tools/glyph-bake` produces both kinds) with
//! [`Recognizer::from_bytes`] and [`FontInk::from_bytes`].

mod archive;
mod error;
mod font;
mod ink;
mod marks;
mod recognizer;
mod template;

pub use error::HandwritingError;
pub use font::{FontInk, GlyphInk, PathCmd, Verb};
pub use ink::{Point, TRACE_POINTS, Trace};
pub use marks::Mark;
pub use recognizer::{Candidate, Recognizer};
pub use template::{Extent, Label, Line, Template, TemplateSet};
