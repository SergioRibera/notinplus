//! Styled text roles.

use std::borrow::Cow;

use freya::prelude::*;

use crate::theme::palette;
use crate::tokens::text;

/// Typographic role of a [`Text`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextRole {
    /// Screen title in sheets (18, bold).
    Title,
    /// Section heading inside a screen (22, bold).
    Heading,
    /// Large display numbers (28, bold).
    Display,
    /// Body copy (14).
    #[default]
    Body,
    /// Emphasised body (14, semibold).
    Strong,
    /// Secondary line under a title (12, tertiary colour).
    Meta,
    /// Help and hint text (12, tertiary colour, wraps).
    Hint,
    /// Upper-case section label (11, bold, tracked).
    Label,
}

impl TextRole {
    const fn size(self) -> f32 {
        match self {
            Self::Title => text::TITLE,
            Self::Heading => text::HEADING,
            Self::Display => text::DISPLAY,
            Self::Body | Self::Strong => text::BODY,
            Self::Meta | Self::Hint => text::LABEL,
            Self::Label => text::CAPTION,
        }
    }

    const fn weight(self) -> FontWeight {
        match self {
            Self::Title | Self::Heading | Self::Display | Self::Label => FontWeight::BOLD,
            Self::Strong => FontWeight::SEMI_BOLD,
            Self::Body | Self::Meta | Self::Hint => FontWeight::NORMAL,
        }
    }
}

/// A run of text in one of the kit roles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    content: Cow<'static, str>,
    role: TextRole,
    color: Option<Color>,
    lines: Option<usize>,
    fill: bool,
    align: Option<TextAlign>,
}

impl Text {
    #[must_use]
    pub fn new(text: impl Into<Cow<'static, str>>) -> Self {
        Self {
            content: text.into(),
            role: TextRole::Body,
            color: None,
            lines: None,
            fill: false,
            align: None,
        }
    }

    #[must_use]
    pub const fn role(mut self, role: TextRole) -> Self {
        self.role = role;
        self
    }

    #[must_use]
    pub const fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Truncate after `lines` lines with an ellipsis.
    #[must_use]
    pub const fn lines(mut self, lines: usize) -> Self {
        self.lines = Some(lines);
        self
    }

    /// Take the remaining width of a flex row (parent needs
    /// `Content::Flex`).
    #[must_use]
    pub const fn fill(mut self) -> Self {
        self.fill = true;
        self
    }

    #[must_use]
    pub const fn align(mut self, align: TextAlign) -> Self {
        self.align = Some(align);
        self
    }
}

impl Component for Text {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let color = self.color.unwrap_or(match self.role {
            TextRole::Meta | TextRole::Hint | TextRole::Label => p.t3,
            _ => p.t1,
        });
        let shown = if self.role == TextRole::Label {
            Cow::Owned(self.content.to_uppercase())
        } else {
            self.content.clone()
        };
        label()
            .text(shown)
            .font_size(self.role.size())
            .font_weight(self.role.weight())
            .color(color)
            .maybe(self.role == TextRole::Label, |l| {
                l.letter_spacing(text::LABEL_TRACKING)
            })
            .maybe(
                self.role == TextRole::Hint || self.role == TextRole::Body,
                |l| l.line_height(text::LINE_BODY),
            )
            .map(self.lines, |l, n| {
                l.max_lines(n).text_overflow(TextOverflow::Ellipsis)
            })
            .map(self.align, Label::text_align)
            .maybe(self.fill, |l| l.width(Size::flex(1.0)))
    }
}
