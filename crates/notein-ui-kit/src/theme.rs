//! Colour themes: the five shipped palettes, the user's accent and the
//! reactive context every component reads its colours from.
//!
//! Components call [`palette()`] while rendering; it subscribes them,
//! so switching the theme in Settings repaints the whole app.

mod context;
mod kind;
mod palette;

pub use context::{
    Appearance, ResolvedTheme, TextSize, ThemeHandle, palette, reduce_motion, try_theme,
    use_theme_root,
};
pub use kind::{ThemeChoice, ThemeKind};
pub use palette::{
    ACCENT_MIN_CONTRAST, Highlighters, KindColors, Palette, contrast_ratio, on_color,
    relative_luminance,
};

/// Accent swatches offered in Settings › Apariencia, before the free
/// colour picker.
pub const ACCENT_SWATCHES: [freya::prelude::Color; 8] = [
    freya::prelude::Color::new(0xFF7B_8CFF),
    freya::prelude::Color::new(0xFF5F_B38A),
    freya::prelude::Color::new(0xFFE0_A845),
    freya::prelude::Color::new(0xFFE8_735E),
    freya::prelude::Color::new(0xFFA8_8BF0),
    freya::prelude::Color::new(0xFF4F_B6C9),
    freya::prelude::Color::new(0xFFE0_7BB5),
    freya::prelude::Color::new(0xFF9A_958C),
];
