//! Bundled fonts (SIL OFL): Lora for the reader, Noto Serif Hebrew and Noto
//! Serif for the original Hebrew and Greek.
//!
//! The TTF bytes are compiled into the crate. The plugin hands them to
//! the shell through `Plugin::fonts`, which registers them before the
//! first window opens, so the reader's `font_family: "Lora"` resolves.
//!
//! Every face registers under [`FAMILY`], so one family name reaches
//! the italic and bold faces too.

pub const FAMILY: &str = "Lora";

pub const REGULAR: &[u8] = include_bytes!("../assets/fonts/Lora-Regular.ttf");
pub const ITALIC: &[u8] = include_bytes!("../assets/fonts/Lora-Italic.ttf");
pub const MEDIUM: &[u8] = include_bytes!("../assets/fonts/Lora-Medium.ttf");
pub const MEDIUM_ITALIC: &[u8] = include_bytes!("../assets/fonts/Lora-MediumItalic.ttf");
pub const SEMI_BOLD: &[u8] = include_bytes!("../assets/fonts/Lora-SemiBold.ttf");
pub const SEMI_BOLD_ITALIC: &[u8] = include_bytes!("../assets/fonts/Lora-SemiBoldItalic.ttf");
pub const BOLD: &[u8] = include_bytes!("../assets/fonts/Lora-Bold.ttf");
pub const BOLD_ITALIC: &[u8] = include_bytes!("../assets/fonts/Lora-BoldItalic.ttf");

/// Every bundled face, from regular to bold italic.
pub const FACES: [&[u8]; 8] = [
    REGULAR,
    ITALIC,
    MEDIUM,
    MEDIUM_ITALIC,
    SEMI_BOLD,
    SEMI_BOLD_ITALIC,
    BOLD,
    BOLD_ITALIC,
];

/// Hebrew text of the Old Testament (Noto Serif Hebrew, SIL OFL, see
/// `assets/fonts/OFL-Noto.txt`).
pub const HEBREW_FAMILY: &str = "Noto Serif Hebrew";
pub const HEBREW: &[u8] = include_bytes!("../assets/fonts/NotoSerifHebrew.ttf");

/// Polytonic Greek of the New Testament (Noto Serif, SIL OFL). A variable
/// font: one file holds every weight.
pub const GREEK_FAMILY: &str = "Noto Serif";
pub const GREEK: &[u8] = include_bytes!("../assets/fonts/NotoSerif.ttf");

/// Family the original text of `testament` is written in.
#[must_use]
pub const fn original_family(old_testament: bool) -> &'static str {
    if old_testament {
        HEBREW_FAMILY
    } else {
        GREEK_FAMILY
    }
}
