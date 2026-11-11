//! Resolved colour tokens for one theme.

use freya::prelude::Color;

/// Colour of each document kind. Fixed per theme: the accent never
/// tints them, so a PDF reads as a PDF in every appearance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KindColors {
    pub canvas: Color,
    pub pdf: Color,
    pub note: Color,
    pub bible: Color,
    pub folder: Color,
    pub scratch: Color,
}

/// The five highlighter inks, in the order the selection bar shows
/// them: yellow, green, pink, blue, violet.
pub type Highlighters = [Color; 5];

/// Every colour token the shell and plugins paint with.
///
/// Values come from [`super::ThemeKind::base`] and are then adjusted by
/// [`Palette::with_accent`]. Nothing outside this module hard-codes a
/// chrome colour; user content (ink, paper) keeps its own colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Whether the surfaces are dark. Drives inverse-text choices.
    pub dark: bool,
    /// App background behind every surface.
    pub bg: Color,
    /// Bars and side panels.
    pub s1: Color,
    /// Cards, sheets, popovers.
    pub s2: Color,
    /// Inputs, chips, hover.
    pub s3: Color,
    /// Pressed state, selected segment.
    pub s4: Color,
    pub line: Color,
    pub line_strong: Color,
    /// Primary text.
    pub t1: Color,
    /// Secondary text.
    pub t2: Color,
    /// Meta text and placeholders.
    pub t3: Color,
    /// Interface accent: primary button, selection, focus, links,
    /// switches, active tab.
    pub accent: Color,
    /// Accent tint used behind selected rows and tonal buttons.
    pub accent_soft: Color,
    /// Accent-coloured text drawn over [`Palette::accent_soft`].
    pub accent_text: Color,
    /// Text drawn over a filled [`Palette::accent`].
    pub on_accent: Color,
    pub kinds: KindColors,
    pub highlighters: Highlighters,
    /// Default paper behind ink.
    pub paper: Color,
    pub paper_line: Color,
    /// Text painted on paper.
    pub paper_ink: Color,
    pub danger: Color,
    pub ok: Color,
    pub warn: Color,
    /// Surface of toasts and selection bars (inverse of the theme).
    pub inv: Color,
    pub inv_text: Color,
    pub inv_line: Color,
    /// Focus ring colour.
    pub focus: Color,
    /// Focus ring width in logical pixels.
    pub focus_width: f32,
    /// Scrim behind modal sheets.
    pub scrim: Color,
    pub shadow: Color,
}

/// Minimum contrast an accent must reach against the background.
pub const ACCENT_MIN_CONTRAST: f32 = 4.5;

impl Palette {
    /// Replace the interface accent with `accent`, deriving its soft
    /// tint and text colours from the theme.
    ///
    /// When `accent` does not reach [`ACCENT_MIN_CONTRAST`] against the
    /// background, the same hue is lightened (dark themes) or darkened
    /// (light themes) until it does. The returned flag tells the
    /// picker to say so.
    #[must_use]
    pub fn with_accent(mut self, accent: Color) -> (Self, bool) {
        let fitted = fit_contrast(accent, self.bg, self.dark);
        let adjusted = fitted != accent;
        self.accent = fitted;
        self.accent_soft = Color::lerp(self.bg, fitted, if self.dark { 0.18 } else { 0.14 });
        self.accent_text = if self.dark {
            Color::lerp(fitted, Color::WHITE, 0.6)
        } else {
            Color::lerp(fitted, Color::BLACK, 0.35)
        };
        self.on_accent = on_color(fitted);
        if !self.keeps_own_focus() {
            self.focus = fitted;
        }
        (self, adjusted)
    }

    /// The high-contrast theme keeps its yellow focus ring whatever the
    /// accent is: the ring must stay findable at a glance.
    fn keeps_own_focus(&self) -> bool {
        self.focus_width > 2.0
    }

    /// Highlighter ink at `index`, wrapping around the five inks.
    #[must_use]
    pub const fn highlighter(&self, index: usize) -> Color {
        self.highlighters[index % self.highlighters.len()]
    }
}

/// Text colour that reads on top of `fill`.
#[must_use]
pub fn on_color(fill: Color) -> Color {
    if relative_luminance(fill) >= 0.25 {
        Color::from_rgb(14, 16, 36)
    } else {
        Color::WHITE
    }
}

/// WCAG relative luminance of an opaque colour.
#[must_use]
pub fn relative_luminance(color: Color) -> f32 {
    let channel = |c: u8| {
        let v = f32::from(c) / 255.0;
        if v <= 0.039_28 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.0722f32.mul_add(
        channel(color.b()),
        0.2126f32.mul_add(channel(color.r()), 0.7152 * channel(color.g())),
    )
}

/// WCAG contrast ratio between two opaque colours (1.0 to 21.0).
#[must_use]
pub fn contrast_ratio(a: Color, b: Color) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Move `color` towards white (dark background) or black (light
/// background) in small steps until it reaches the minimum contrast.
fn fit_contrast(color: Color, bg: Color, dark: bool) -> Color {
    let target = if dark { Color::WHITE } else { Color::BLACK };
    (0..=20u8)
        .map(|step| Color::lerp(color, target, f32::from(step) * 0.05))
        .find(|candidate| contrast_ratio(*candidate, bg) >= ACCENT_MIN_CONTRAST)
        .unwrap_or(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeKind;

    #[test]
    fn default_accents_already_fit_their_theme() {
        for kind in ThemeKind::ALL {
            let base = kind.base();
            let (_, adjusted) = base.with_accent(base.accent);
            assert!(!adjusted, "{kind:?} default accent needed adjusting");
        }
    }

    #[test]
    fn pale_accent_is_darkened_on_day() {
        let (palette, adjusted) = ThemeKind::Dia
            .base()
            .with_accent(Color::from_rgb(224, 168, 69));
        assert!(adjusted);
        assert!(contrast_ratio(palette.accent, palette.bg) >= ACCENT_MIN_CONTRAST);
    }

    #[test]
    fn contrast_of_black_on_white_is_21() {
        let ratio = contrast_ratio(Color::BLACK, Color::WHITE);
        assert!((ratio - 21.0).abs() < 0.01);
    }
}
