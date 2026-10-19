use std::borrow::Cow;

use freya::prelude::Color;

use crate::document::{CalloutKind, HeadingLevel, Rgba};

/// Palette used to paint the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorColors {
    pub text: Color,
    pub muted: Color,
    pub placeholder: Color,
    pub accent: Color,
    pub caret: Color,
    pub selection: Color,
    pub border: Color,
    pub surface: Color,
    pub code_background: Color,
    pub code_text: Color,
    pub quote_bar: Color,
    pub highlight: Color,
    pub spoiler: Color,
    pub popup_background: Color,
    pub popup_selected: Color,
    pub note: Color,
    pub tip: Color,
    pub important: Color,
    pub warning: Color,
    pub caution: Color,
}

impl EditorColors {
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            text: Color::from_rgb(228, 230, 235),
            muted: Color::from_rgb(150, 156, 168),
            placeholder: Color::from_rgb(110, 116, 128),
            accent: Color::from_rgb(110, 168, 254),
            caret: Color::from_rgb(228, 230, 235),
            selection: Color::from_argb(90, 110, 168, 254),
            border: Color::from_rgb(58, 62, 72),
            surface: Color::from_rgb(30, 32, 38),
            code_background: Color::from_rgb(40, 43, 51),
            code_text: Color::from_rgb(235, 188, 140),
            quote_bar: Color::from_rgb(90, 96, 110),
            highlight: Color::from_argb(110, 250, 204, 21),
            spoiler: Color::from_rgb(70, 74, 86),
            popup_background: Color::from_rgb(38, 41, 49),
            popup_selected: Color::from_rgb(56, 62, 78),
            note: Color::from_rgb(88, 166, 255),
            tip: Color::from_rgb(63, 185, 80),
            important: Color::from_rgb(171, 125, 248),
            warning: Color::from_rgb(210, 153, 34),
            caution: Color::from_rgb(248, 81, 73),
        }
    }

    #[must_use]
    pub const fn light() -> Self {
        Self {
            text: Color::from_rgb(31, 35, 40),
            muted: Color::from_rgb(101, 109, 118),
            placeholder: Color::from_rgb(140, 148, 158),
            accent: Color::from_rgb(9, 105, 218),
            caret: Color::from_rgb(31, 35, 40),
            selection: Color::from_argb(70, 9, 105, 218),
            border: Color::from_rgb(208, 215, 222),
            surface: Color::from_rgb(255, 255, 255),
            code_background: Color::from_rgb(240, 242, 245),
            code_text: Color::from_rgb(160, 60, 40),
            quote_bar: Color::from_rgb(208, 215, 222),
            highlight: Color::from_argb(140, 255, 223, 93),
            spoiler: Color::from_rgb(200, 205, 212),
            popup_background: Color::from_rgb(255, 255, 255),
            popup_selected: Color::from_rgb(234, 238, 242),
            note: Color::from_rgb(9, 105, 218),
            tip: Color::from_rgb(26, 127, 55),
            important: Color::from_rgb(130, 80, 223),
            warning: Color::from_rgb(154, 103, 0),
            caution: Color::from_rgb(207, 34, 46),
        }
    }

    #[must_use]
    pub const fn callout(&self, kind: CalloutKind) -> Color {
        match kind {
            CalloutKind::Note => self.note,
            CalloutKind::Tip => self.tip,
            CalloutKind::Important => self.important,
            CalloutKind::Warning => self.warning,
            CalloutKind::Caution => self.caution,
        }
    }
}

/// Colors, sizes and fonts of the editor. Pass it with
/// [`MarkdownEditor::theme`](crate::MarkdownEditor::theme), or provide it as
/// context so every editor (and toolbar) below picks it up.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorTheme {
    pub colors: EditorColors,
    pub body_size: f32,
    pub line_height: f32,
    pub block_spacing: f32,
    /// Font sizes for `#` to `######`.
    pub heading_sizes: [f32; 6],
    pub monospace: Cow<'static, str>,
    /// Swatches offered by the toolbar color pickers.
    pub palette: Vec<Rgba>,
    /// Families offered by the toolbar font picker.
    pub fonts: Vec<Cow<'static, str>>,
    /// Sizes offered by the toolbar font size picker.
    pub font_sizes: Vec<u16>,
    /// Factor applied to every length the editor lays out (fonts, paddings,
    /// markers, embeds). Hosts that zoom the editor, like a canvas, set it
    /// with [`Self::zoomed`] so layout and hit testing stay native instead
    /// of going through a render-only scale effect.
    pub zoom: f32,
}

impl Default for EditorTheme {
    fn default() -> Self {
        Self::from(EditorColors::dark())
    }
}

impl From<EditorColors> for EditorTheme {
    fn from(colors: EditorColors) -> Self {
        Self {
            colors,
            body_size: 15.0,
            line_height: 1.45,
            block_spacing: 6.0,
            heading_sizes: [28.0, 23.0, 19.0, 17.0, 15.5, 14.5],
            monospace: Cow::Borrowed("monospace"),
            palette: vec![
                Rgba::rgb(0, 0, 0),
                Rgba::rgb(239, 68, 68),
                Rgba::rgb(249, 115, 22),
                Rgba::rgb(234, 179, 8),
                Rgba::rgb(34, 197, 94),
                Rgba::rgb(6, 182, 212),
                Rgba::rgb(59, 130, 246),
                Rgba::rgb(168, 85, 247),
                Rgba::rgb(236, 72, 153),
                Rgba::rgb(148, 163, 184),
            ],
            fonts: vec![
                Cow::Borrowed("sans-serif"),
                Cow::Borrowed("serif"),
                Cow::Borrowed("monospace"),
            ],
            font_sizes: vec![10, 12, 14, 15, 16, 18, 20, 24, 30, 36],
            zoom: 1.0,
        }
    }
}

impl EditorTheme {
    #[must_use]
    pub fn dark() -> Self {
        Self::from(EditorColors::dark())
    }

    #[must_use]
    pub fn light() -> Self {
        Self::from(EditorColors::light())
    }

    #[must_use]
    pub const fn heading_size(&self, level: HeadingLevel) -> f32 {
        self.heading_sizes[level.get() as usize - 1]
    }

    /// Copy of the theme with every length multiplied by `zoom`.
    #[must_use]
    pub fn zoomed(mut self, zoom: f32) -> Self {
        let factor = zoom / self.zoom;
        self.body_size *= factor;
        self.block_spacing *= factor;
        for size in &mut self.heading_sizes {
            *size *= factor;
        }
        self.zoom = zoom;
        self
    }

    /// `length` scaled by [`Self::zoom`].
    #[must_use]
    pub const fn px(&self, length: f32) -> f32 {
        length * self.zoom
    }
}

impl From<Rgba> for Color {
    fn from(color: Rgba) -> Self {
        Self::from_argb(color.a, color.r, color.g, color.b)
    }
}

impl From<Color> for Rgba {
    fn from(color: Color) -> Self {
        Self::new(color.r(), color.g(), color.b(), color.a())
    }
}
