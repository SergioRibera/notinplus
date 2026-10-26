//! Bundled Lora font family (SIL OFL).
//!
//! The TTF bytes are compiled into the crate. The host registers them
//! with Freya's font collection at startup so the reader's
//! `font_family: "Lora"` references resolve correctly.
//!
//! Every face registers under [`FAMILY`], so one family name reaches
//! the italic and bold faces too:
//!
//! ```ignore
//! for face in notein_bible::fonts::FACES {
//!     config = config.with_font(notein_bible::fonts::FAMILY, face);
//! }
//! ```

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
