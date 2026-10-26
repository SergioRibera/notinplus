//! Font family + size constants.
//!
//! Lora (SIL OFL) is bundled under `assets/fonts/`. Freya resolves
//! family names against the font cache seeded by the host; the host
//! must load the bundled face at startup for `font_family: FAMILY` to
//! resolve. If the face is unavailable the system serif is used, which
//! is close enough for a visual fallback.

pub const FAMILY: &str = "Lora";
pub const FAMILY_FALLBACK: &str = "serif";

pub const BODY_PX: f32 = 22.0;
pub const BODY_LINE_HEIGHT: f32 = 1.65;
/// Widest the reading column grows: about 70 characters of body text,
/// so lines stay easy to follow on a wide window.
pub const MEASURE_PX: f32 = 720.0;
pub const VERSE_NUM_PX: f32 = 13.0;
pub const SECTION_HEADER_PX: f32 = 18.0;
pub const DROP_CAP_PX: f32 = 110.0;
pub const TOOLBAR_LABEL_PX: f32 = 14.0;
