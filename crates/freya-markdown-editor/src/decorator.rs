//! Hooks that let an integrator map markdown constructs to their own visuals.
//!
//! A [`Decorator`] can:
//!
//! - claim links by URL so they render as atomic widgets ([`Atom::Embed`]),
//!   e.g. `app://note/42` drawn as a chip;
//! - restyle links or arbitrary text patterns (`#tags`, `@mentions`);
//! - replace the rendering of fenced code blocks (`mermaid`, `math`, ...)
//!   and images.
//!
//! [`Atom::Embed`]: crate::document::Atom::Embed

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;
use std::rc::Rc;

use freya::prelude::*;

use crate::markdown::EmbedFilter;
use crate::theme::EditorTheme;

/// Visual override for a run of text. Unset fields keep the base style.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpanStyle {
    pub color: Option<Color>,
    pub background: Option<Color>,
    pub font_weight: Option<FontWeight>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub font_family: Option<Cow<'static, str>>,
    pub font_size: Option<f32>,
}

impl SpanStyle {
    #[must_use]
    pub fn color(mut self, color: impl Into<Color>) -> Self {
        self.color = Some(color.into());
        self
    }

    #[must_use]
    pub fn background(mut self, color: impl Into<Color>) -> Self {
        self.background = Some(color.into());
        self
    }

    #[must_use]
    pub const fn bold(mut self) -> Self {
        self.font_weight = Some(FontWeight::BOLD);
        self
    }

    #[must_use]
    pub const fn italic(mut self) -> Self {
        self.italic = Some(true);
        self
    }

    #[must_use]
    pub const fn underline(mut self) -> Self {
        self.underline = Some(true);
        self
    }

    #[must_use]
    pub fn font_family(mut self, family: impl Into<Cow<'static, str>>) -> Self {
        self.font_family = Some(family.into());
        self
    }

    #[must_use]
    pub const fn font_size(mut self, size: f32) -> Self {
        self.font_size = Some(size);
        self
    }

    /// Fields set in `other` win.
    #[must_use]
    pub fn merge(mut self, other: &Self) -> Self {
        self.color = other.color.or(self.color);
        self.background = other.background.or(self.background);
        self.font_weight = other.font_weight.or(self.font_weight);
        self.italic = other.italic.or(self.italic);
        self.underline = other.underline.or(self.underline);
        self.font_family = other.font_family.clone().or(self.font_family);
        self.font_size = other.font_size.or(self.font_size);
        self
    }
}

/// A styled match returned by [`Decorator::match_text`].
#[derive(Clone, Debug, PartialEq)]
pub struct TextMatch {
    /// Byte range inside the text handed to the decorator.
    pub range: Range<usize>,
    pub style: SpanStyle,
}

/// Data available when rendering an embed.
#[derive(Clone, Copy, Debug)]
pub struct EmbedContext<'a> {
    pub url: &'a str,
    pub label: &'a str,
    pub title: Option<&'a str>,
    pub read_only: bool,
    pub theme: &'a EditorTheme,
}

/// Data available when rendering a fenced code block.
#[derive(Clone, Copy, Debug)]
pub struct CodeContext<'a> {
    pub language: &'a str,
    pub code: &'a str,
    /// Widest a drawing of it may be, in logical pixels.
    pub max_width: f32,
    pub theme: &'a EditorTheme,
}

/// Data available when rendering a [`Transclusion`] block.
///
/// [`Transclusion`]: crate::document::Transclusion
#[derive(Clone, Copy, Debug)]
pub struct TransclusionContext<'a> {
    pub kind: &'a str,
    pub url: &'a str,
    pub label: &'a str,
    /// Markdown of the content as last seen.
    pub fallback: &'a str,
    pub read_only: bool,
    pub theme: &'a EditorTheme,
}

/// Data available when drawing a formula.
#[derive(Clone, Copy, Debug)]
pub struct MathContext<'a> {
    pub tex: &'a str,
    /// A block of its own (`$$`) rather than inside a line (`$`).
    pub display: bool,
    /// Size of the text around it, in logical pixels.
    pub font_size: f32,
    /// Widest it may be drawn, in logical pixels.
    pub max_width: f32,
    pub theme: &'a EditorTheme,
}

/// Data available when rendering an image.
#[derive(Clone, Copy, Debug)]
pub struct ImageContext<'a> {
    pub url: &'a str,
    pub alt: &'a str,
    pub title: Option<&'a str>,
    /// Widest the image may be drawn, in logical pixels: the width of the
    /// text it sits in.
    pub max_width: f32,
    pub read_only: bool,
    pub theme: &'a EditorTheme,
}

/// Customization hooks. Every method has a no-op default so implementors
/// only override what they need.
pub trait Decorator {
    /// Links whose URL is claimed become atomic embeds.
    fn claims_link(&self, _url: &str) -> bool {
        false
    }

    /// Widget for a claimed link. Returning `None` falls back to a default
    /// pill showing the label.
    fn render_embed(&self, _embed: &EmbedContext<'_>) -> Option<Element> {
        None
    }

    /// Extra styling for a regular (unclaimed) link.
    fn link_style(&self, _url: &str) -> Option<SpanStyle> {
        None
    }

    /// Style arbitrary matches inside the text of a block.
    fn match_text(&self, _text: &str) -> Vec<TextMatch> {
        Vec::new()
    }

    /// Replace a fenced code block when the editor is read only.
    fn render_code_block(&self, _code: &CodeContext<'_>) -> Option<Element> {
        None
    }

    /// Replace the default image placeholder.
    fn render_image(&self, _image: &ImageContext<'_>) -> Option<Element> {
        None
    }

    /// Draw a formula. Without one, formulas show their TeX source.
    fn render_math(&self, _math: &MathContext<'_>) -> Option<Element> {
        None
    }

    /// Draw what a code block describes, like a diagram, under its source
    /// while editing and instead of it while reading. `None` for languages
    /// the decorator does not draw.
    fn render_preview(&self, _code: &CodeContext<'_>) -> Option<Element> {
        None
    }

    /// Show a transcluded quote as its source holds it now. `None` keeps
    /// the default: the content as last seen.
    fn render_transclusion(&self, _transclusion: &TransclusionContext<'_>) -> Option<Element> {
        None
    }
}

/// Ordered set of decorators. The first decorator that answers wins for
/// rendering hooks; text matches and link styles accumulate.
#[derive(Clone, Default)]
pub struct Decorators(Rc<Vec<Rc<dyn Decorator>>>);

impl PartialEq for Decorators {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for Decorators {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Decorators").field(&self.0.len()).finish()
    }
}

impl<D: Decorator + 'static> From<D> for Decorators {
    fn from(decorator: D) -> Self {
        Self::default().with(decorator)
    }
}

impl Decorators {
    #[must_use]
    pub fn with(self, decorator: impl Decorator + 'static) -> Self {
        let mut list = Vec::clone(&self.0);
        list.push(Rc::new(decorator));
        Self(Rc::new(list))
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn claims_link(&self, url: &str) -> bool {
        self.0.iter().any(|decorator| decorator.claims_link(url))
    }

    /// Parser hook built from [`Self::claims_link`].
    #[must_use]
    pub fn embed_filter(&self) -> Option<EmbedFilter> {
        if self.0.is_empty() {
            return None;
        }
        let decorators = self.clone();
        Some(Rc::new(move |url: &str| decorators.claims_link(url)))
    }

    #[must_use]
    pub fn render_embed(&self, embed: &EmbedContext<'_>) -> Option<Element> {
        self.0
            .iter()
            .filter(|decorator| decorator.claims_link(embed.url))
            .find_map(|decorator| decorator.render_embed(embed))
    }

    #[must_use]
    pub fn link_style(&self, url: &str) -> Option<SpanStyle> {
        self.0
            .iter()
            .filter_map(|decorator| decorator.link_style(url))
            .reduce(|acc, style| acc.merge(&style))
    }

    #[must_use]
    pub fn match_text(&self, text: &str) -> Vec<TextMatch> {
        self.0
            .iter()
            .flat_map(|decorator| decorator.match_text(text))
            .collect()
    }

    #[must_use]
    pub fn render_code_block(&self, code: &CodeContext<'_>) -> Option<Element> {
        self.0
            .iter()
            .find_map(|decorator| decorator.render_code_block(code))
    }

    #[must_use]
    pub fn render_image(&self, image: &ImageContext<'_>) -> Option<Element> {
        self.0
            .iter()
            .find_map(|decorator| decorator.render_image(image))
    }

    #[must_use]
    pub fn render_math(&self, math: &MathContext<'_>) -> Option<Element> {
        self.0
            .iter()
            .find_map(|decorator| decorator.render_math(math))
    }

    #[must_use]
    pub fn render_preview(&self, code: &CodeContext<'_>) -> Option<Element> {
        self.0
            .iter()
            .find_map(|decorator| decorator.render_preview(code))
    }

    #[must_use]
    pub fn render_transclusion(&self, transclusion: &TransclusionContext<'_>) -> Option<Element> {
        self.0
            .iter()
            .find_map(|decorator| decorator.render_transclusion(transclusion))
    }
}

type EmbedRenderer = Rc<dyn Fn(&EmbedContext<'_>) -> Element>;

/// Claims every link using a URL scheme (`app://...`) and renders it with a
/// closure. The building block for deep link chips.
#[derive(Clone)]
pub struct SchemeEmbed {
    prefix: String,
    render: EmbedRenderer,
}

impl fmt::Debug for SchemeEmbed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SchemeEmbed")
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

impl SchemeEmbed {
    /// `scheme` without the `://` suffix, e.g. `"notein"`.
    #[must_use]
    pub fn new(
        scheme: impl AsRef<str>,
        render: impl Fn(&EmbedContext<'_>) -> Element + 'static,
    ) -> Self {
        Self {
            prefix: format!("{}://", scheme.as_ref()),
            render: Rc::new(render),
        }
    }
}

impl Decorator for SchemeEmbed {
    fn claims_link(&self, url: &str) -> bool {
        url.starts_with(&self.prefix)
    }

    fn render_embed(&self, embed: &EmbedContext<'_>) -> Option<Element> {
        Some((self.render)(embed))
    }
}

/// Styles words starting with a prefix char, like `#tags` or `@mentions`.
#[derive(Clone, Debug, PartialEq)]
pub struct PrefixTokens {
    prefix: char,
    style: SpanStyle,
}

impl PrefixTokens {
    #[must_use]
    pub const fn new(prefix: char, style: SpanStyle) -> Self {
        Self { prefix, style }
    }
}

impl Decorator for PrefixTokens {
    fn match_text(&self, text: &str) -> Vec<TextMatch> {
        let is_token = |ch: char| ch.is_alphanumeric() || matches!(ch, '_' | '-' | '/' | '.');
        let mut matches = Vec::new();
        let mut previous: Option<char> = None;
        let mut chars = text.char_indices().peekable();
        while let Some((start, ch)) = chars.next() {
            let at_word_start = previous.is_none_or(char::is_whitespace);
            previous = Some(ch);
            if ch != self.prefix || !at_word_start {
                continue;
            }
            let mut end = start + ch.len_utf8();
            while let Some((index, next)) = chars.peek().copied() {
                if !is_token(next) {
                    break;
                }
                end = index + next.len_utf8();
                previous = Some(next);
                chars.next();
            }
            let trimmed = text[..end].trim_end_matches(['.', '/', '-']).len();
            if trimmed > start + ch.len_utf8() {
                matches.push(TextMatch {
                    range: start..trimmed,
                    style: self.style.clone(),
                });
            }
        }
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_tokens_find_words() {
        let tags = PrefixTokens::new('#', SpanStyle::default().bold());
        let found: Vec<_> = tags
            .match_text("a #tag, b#no #two-parts. #")
            .into_iter()
            .map(|m| m.range)
            .collect();
        assert_eq!(found, vec![2..6, 13..23]);
    }

    #[test]
    fn scheme_embed_claims_by_prefix() {
        let embed = SchemeEmbed::new("app", |_| rect().into_element());
        assert!(embed.claims_link("app://x"));
        assert!(!embed.claims_link("https://x"));
        let decorators = Decorators::from(embed);
        let filter = decorators.embed_filter().expect("filter");
        assert!(filter("app://y"));
    }
}
