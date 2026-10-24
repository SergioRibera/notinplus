//! Spacing, radius, size and type scale shared by every component.
//!
//! Layout numbers live here so a screen never invents its own gap.

/// Spacing scale (logical pixels).
pub mod space {
    pub const XXS: f32 = 2.0;
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 24.0;
    pub const XXL: f32 = 32.0;
    pub const XXXL: f32 = 48.0;
    /// Side gutter of phone-width layouts.
    pub const GUTTER_COMPACT: f32 = 16.0;
    /// Side gutter of tablet and desktop layouts.
    pub const GUTTER: f32 = 32.0;
}

/// Corner radius scale.
pub mod radius {
    pub const XS: f32 = 6.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 10.0;
    pub const LG: f32 = 12.0;
    pub const XL: f32 = 16.0;
    pub const SHEET: f32 = 20.0;
    pub const FULL: f32 = 999.0;
}

/// Component sizes.
pub mod size {
    /// Minimum touch target.
    pub const HIT: f32 = 44.0;
    /// Minimum pointer target on desktop.
    pub const HIT_DESKTOP: f32 = 36.0;
    pub const ICON: f32 = 20.0;
    pub const ICON_SM: f32 = 16.0;
    pub const ICON_XS: f32 = 14.0;
    pub const BUTTON_SM: f32 = 32.0;
    pub const BUTTON: f32 = 40.0;
    pub const BUTTON_LG: f32 = 48.0;
    pub const BAR: f32 = 52.0;
    pub const TAB_BAR: f32 = 44.0;
    pub const ROW: f32 = 48.0;
    pub const ROW_DENSE: f32 = 40.0;
    pub const SHEET_WIDTH: f32 = 560.0;
    pub const SHEET_WIDTH_SM: f32 = 440.0;
    pub const POPOVER_WIDTH: f32 = 420.0;
    pub const PANEL_WIDTH: f32 = 296.0;
    pub const CARD_WIDTH: f32 = 168.0;
}

/// Type scale (font sizes in logical pixels) and line heights.
pub mod text {
    pub const CAPTION: f32 = 11.0;
    pub const LABEL: f32 = 12.0;
    pub const META: f32 = 13.0;
    pub const BODY: f32 = 14.0;
    pub const BODY_LG: f32 = 16.0;
    pub const TITLE: f32 = 18.0;
    pub const HEADING: f32 = 22.0;
    pub const DISPLAY: f32 = 28.0;
    pub const LINE_TIGHT: f32 = 1.2;
    pub const LINE_BODY: f32 = 1.45;
    /// Letter spacing of upper-case section labels.
    pub const LABEL_TRACKING: f32 = 1.0;
}

/// Motion durations (milliseconds). Components skip them when
/// [`crate::theme::reduce_motion`] is on.
pub mod motion {
    pub const FAST: u64 = 120;
    pub const BASE: u64 = 180;
    pub const SLOW: u64 = 250;
}

/// Toast durations (milliseconds).
pub mod toast {
    /// Default toast with an undo action.
    pub const UNDO: u64 = 6_000;
    /// Borrador close toast: content is lost when it expires.
    pub const SCRATCH: u64 = 8_000;
    /// Informational toast without action.
    pub const INFO: u64 = 3_500;
}
