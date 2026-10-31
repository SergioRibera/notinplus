//! Markdown source <-> [`Document`] conversion.
//!
//! Parsing is done by [comrak] (`CommonMark` + GFM + extensions) and then
//! lowered to the flat block model. Writing is done by this crate so the
//! output can target two dialects:
//!
//! - [`Dialect::Rich`]: lossless for everything the editor can express.
//!   Inline styles travel as `<span style>`, alignment as `<div align>`,
//!   and extension syntax (`==mark==`, `||spoiler||`, `[TOC]`, `{#id}`,
//!   abbreviations) is kept.
//! - [`Dialect::Portable`]: plain `CommonMark` + GFM that renders anywhere.
//!   Editor only styling is dropped and the table of contents is expanded.

mod html;
mod parse;
mod serialize;

use std::fmt;
use std::rc::Rc;

use crate::document::{BlockKind, Document, Inline};

pub use serialize::slug;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Dialect {
    #[default]
    Rich,
    Portable,
}

/// Syntax extensions recognised while parsing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent feature switches mirroring comrak options"
)]
pub struct Extensions {
    pub tables: bool,
    pub task_lists: bool,
    pub strikethrough: bool,
    pub autolinks: bool,
    pub footnotes: bool,
    pub alerts: bool,
    pub math: bool,
    pub highlight: bool,
    pub superscript: bool,
    pub subscript: bool,
    pub spoiler: bool,
    pub description_lists: bool,
    pub front_matter: bool,
    pub heading_ids: bool,
    pub emoji_shortcodes: bool,
    pub abbreviations: bool,
    pub table_of_contents: bool,
}

impl Default for Extensions {
    fn default() -> Self {
        Self {
            tables: true,
            task_lists: true,
            strikethrough: true,
            autolinks: true,
            footnotes: true,
            alerts: true,
            math: true,
            highlight: true,
            superscript: true,
            subscript: true,
            spoiler: true,
            description_lists: true,
            front_matter: true,
            heading_ids: true,
            emoji_shortcodes: true,
            abbreviations: true,
            table_of_contents: true,
        }
    }
}

impl Extensions {
    /// Strict `CommonMark`, every extension off.
    #[must_use]
    pub const fn commonmark() -> Self {
        Self {
            tables: false,
            task_lists: false,
            strikethrough: false,
            autolinks: false,
            footnotes: false,
            alerts: false,
            math: false,
            highlight: false,
            superscript: false,
            subscript: false,
            spoiler: false,
            description_lists: false,
            front_matter: false,
            heading_ids: false,
            emoji_shortcodes: false,
            abbreviations: false,
            table_of_contents: false,
        }
    }

    /// GitHub Flavored Markdown.
    #[must_use]
    pub const fn gfm() -> Self {
        Self {
            tables: true,
            task_lists: true,
            strikethrough: true,
            autolinks: true,
            footnotes: true,
            alerts: true,
            math: true,
            ..Self::commonmark()
        }
    }
}

/// Decides whether a link URL becomes an [`Atom::Embed`].
///
/// [`Atom::Embed`]: crate::document::Atom::Embed
pub type EmbedFilter = Rc<dyn Fn(&str) -> bool>;

#[derive(Clone, Default)]
pub struct ParseOptions {
    pub extensions: Extensions,
    pub embed: Option<EmbedFilter>,
}

impl fmt::Debug for ParseOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParseOptions")
            .field("extensions", &self.extensions)
            .field("embed", &self.embed.is_some())
            .finish()
    }
}

impl From<Extensions> for ParseOptions {
    fn from(extensions: Extensions) -> Self {
        Self {
            extensions,
            embed: None,
        }
    }
}

impl Document {
    /// Parse markdown with every extension enabled.
    #[must_use]
    pub fn parse(markdown: &str) -> Self {
        Self::parse_with(markdown, &ParseOptions::default())
    }

    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("markdown::parse"))]
    pub fn parse_with(markdown: &str, options: &ParseOptions) -> Self {
        parse::parse(markdown, options)
    }

    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("markdown::serialize"))]
    pub fn to_markdown(&self, dialect: Dialect) -> String {
        serialize::serialize(self, dialect)
    }
}

/// Markdown source of a single run of inline content.
#[cfg_attr(feature = "sismo", sismo::function("markdown::inline_source"))]
pub(crate) fn inline_source(inline: &Inline) -> String {
    serialize::inline_source(inline)
}

/// Parse `source` as the content of one paragraph. `None` when it reads as
/// anything else (a heading, a list, several blocks...).
#[cfg_attr(feature = "sismo", sismo::function("markdown::parse_inline"))]
pub(crate) fn parse_inline(source: &str, options: &ParseOptions) -> Option<Inline> {
    if source.trim().is_empty() {
        return Some(Inline::from(source));
    }
    let document = Document::parse_with(source, options);
    match document.blocks() {
        [only] => match &only.kind {
            BlockKind::Paragraph(content) => Some(content.clone()),
            _ => None,
        },
        _ => None,
    }
}
