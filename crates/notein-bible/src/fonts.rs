//! Bundled Lora font family (SIL OFL).
//!
//! The TTF bytes are compiled into the crate. The host registers them
//! with Freya's font collection at startup so the reader's
//! `font_family: "Lora"` references resolve correctly.
//!
//! Typical host wiring:
//!
//! ```ignore
//! for (name, bytes) in notein_bible::fonts::all() {
//!     app.register_font(name, bytes);
//! }
//! ```
//!
//! The exact registration call depends on the host's font API — the
//! bytes themselves are opaque TrueType and work with any skia-backed
//! loader.

pub const FAMILY: &str = "Lora";

pub const REGULAR: &[u8] = include_bytes!("../assets/fonts/Lora-Regular.ttf");
pub const ITALIC: &[u8] = include_bytes!("../assets/fonts/Lora-Italic.ttf");
pub const MEDIUM: &[u8] = include_bytes!("../assets/fonts/Lora-Medium.ttf");
pub const MEDIUM_ITALIC: &[u8] = include_bytes!("../assets/fonts/Lora-MediumItalic.ttf");
pub const SEMI_BOLD: &[u8] = include_bytes!("../assets/fonts/Lora-SemiBold.ttf");
pub const SEMI_BOLD_ITALIC: &[u8] = include_bytes!("../assets/fonts/Lora-SemiBoldItalic.ttf");
pub const BOLD: &[u8] = include_bytes!("../assets/fonts/Lora-Bold.ttf");
pub const BOLD_ITALIC: &[u8] = include_bytes!("../assets/fonts/Lora-BoldItalic.ttf");

/// Every bundled face paired with a descriptive name. Hosts that only
/// want one weight should reach for [`REGULAR`] directly.
#[must_use]
pub const fn all() -> &'static [(&'static str, &'static [u8])] {
    &[
        ("Lora-Regular", REGULAR),
        ("Lora-Italic", ITALIC),
        ("Lora-Medium", MEDIUM),
        ("Lora-MediumItalic", MEDIUM_ITALIC),
        ("Lora-SemiBold", SEMI_BOLD),
        ("Lora-SemiBoldItalic", SEMI_BOLD_ITALIC),
        ("Lora-Bold", BOLD),
        ("Lora-BoldItalic", BOLD_ITALIC),
    ]
}
