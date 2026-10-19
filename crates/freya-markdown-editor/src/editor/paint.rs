//! Turns an [`Inline`] into paragraph spans plus the decorations that Skia
//! spans cannot express (backgrounds and stacked underlines).

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::ops::Range;

use freya::prelude::*;

use crate::decorator::{Decorators, SpanStyle};
use crate::document::{Abbreviation, Inline, MarkKind, OBJECT_REPLACEMENT, byte_index, utf16_len};
use crate::theme::EditorTheme;

/// Something drawn behind or under a text range.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Decor {
    pub(crate) range: Range<usize>,
    pub(crate) kind: DecorKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum DecorKind {
    Fill(u32),
    Underline(u32),
    Dotted(u32),
}

pub(crate) enum Piece {
    Text(Span<'static>),
    /// Index into [`Inline::atoms`].
    Atom(usize),
}

pub(crate) struct Painted {
    pub(crate) pieces: Vec<Piece>,
    pub(crate) decor: Vec<Decor>,
}

pub(crate) struct PaintContext<'a> {
    pub(crate) theme: &'a EditorTheme,
    pub(crate) decorators: &'a Decorators,
    pub(crate) abbreviations: &'a [Abbreviation],
    pub(crate) base_size: f32,
    pub(crate) read_only: bool,
    /// Plain role: no marks, monospace handled by the caller.
    pub(crate) plain: bool,
}

#[derive(Clone, Default)]
struct Look {
    style: SpanStyle,
    strike: bool,
}

impl PaintContext<'_> {
    pub(crate) fn paint(&self, inline: &Inline) -> Painted {
        let text = inline.text();
        let len = inline.len();
        let matches = if self.plain {
            Vec::new()
        } else {
            self.text_matches(text)
        };

        let mut bounds: BTreeSet<usize> = BTreeSet::from([0, len]);
        if !self.plain {
            for mark in inline.marks() {
                bounds.insert(mark.range.start);
                bounds.insert(mark.range.end);
            }
        }
        for (range, _) in &matches {
            bounds.insert(range.start);
            bounds.insert(range.end);
        }
        let mut units = 0;
        for ch in text.chars() {
            if ch == OBJECT_REPLACEMENT {
                bounds.insert(units);
                bounds.insert(units + 1);
            }
            units += ch.len_utf16();
        }

        let mut pieces = Vec::new();
        let mut decor = Vec::new();
        let mut atom_index = 0;
        let bounds: Vec<usize> = bounds.into_iter().collect();
        for window in bounds.windows(2) {
            let range = window[0]..window[1];
            let segment = &text[byte_index(text, range.start)..byte_index(text, range.end)];
            if segment.starts_with(OBJECT_REPLACEMENT) {
                pieces.push(Piece::Atom(atom_index));
                atom_index += 1;
                continue;
            }
            let look = self.look_for(inline, &range, &matches);
            if let Some(background) = look.style.background {
                decor.push(Decor {
                    range: range.clone(),
                    kind: DecorKind::Fill(background.into()),
                });
            }
            let underline = look.style.underline == Some(true);
            if underline && look.strike {
                let color = look.style.color.unwrap_or(self.theme.colors.text);
                decor.push(Decor {
                    range: range.clone(),
                    kind: DecorKind::Underline(color.into()),
                });
            }
            pieces.push(Piece::Text(span(segment, &look, underline)));
        }
        if !self.plain {
            decor.extend(self.abbreviation_decor(text));
        }
        Painted {
            pieces,
            decor: merge_adjacent(decor),
        }
    }

    fn text_matches(&self, text: &str) -> Vec<(Range<usize>, SpanStyle)> {
        let to_units = |range: Range<usize>| {
            utf16_len(&text[..range.start.min(text.len())])
                ..utf16_len(&text[..range.end.min(text.len())])
        };
        let mut matches: Vec<(Range<usize>, SpanStyle)> = self
            .decorators
            .match_text(text)
            .into_iter()
            .filter(|found| {
                found.range.start < found.range.end
                    && text.is_char_boundary(found.range.start)
                    && text.is_char_boundary(found.range.end.min(text.len()))
            })
            .map(|found| (to_units(found.range), found.style))
            .collect();
        matches.sort_by_key(|(range, _)| range.start);
        matches
    }

    /// Abbreviation occurrences, underlined with dots like `<abbr>`.
    fn abbreviation_decor(&self, text: &str) -> Vec<Decor> {
        let color: u32 = self.theme.colors.muted.into();
        let boundary = |ch: Option<char>| ch.is_none_or(|ch| !ch.is_alphanumeric());
        self.abbreviations
            .iter()
            .flat_map(|abbreviation| {
                text.match_indices(abbreviation.term.as_str())
                    .filter(|(start, term)| {
                        let end = start + term.len();
                        boundary(text[..*start].chars().next_back())
                            && boundary(text[end..].chars().next())
                    })
                    .map(|(start, term)| Decor {
                        range: utf16_len(&text[..start])..utf16_len(&text[..start + term.len()]),
                        kind: DecorKind::Dotted(color),
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn look_for(
        &self,
        inline: &Inline,
        range: &Range<usize>,
        matches: &[(Range<usize>, SpanStyle)],
    ) -> Look {
        let colors = &self.theme.colors;
        let mut look = Look::default();
        if self.plain {
            return look;
        }
        let covering = inline
            .marks()
            .iter()
            .filter(|mark| mark.range.start <= range.start && range.end <= mark.range.end);
        for mark in covering {
            let style = &mut look.style;
            match &mark.kind {
                MarkKind::Strong => style.font_weight = Some(FontWeight::BOLD),
                MarkKind::Emphasis => style.italic = Some(true),
                MarkKind::Underline => style.underline = Some(true),
                MarkKind::Strikethrough => look.strike = true,
                MarkKind::Code => {
                    style.font_family = Some(self.theme.monospace.clone());
                    style.color = style.color.or(Some(colors.code_text));
                    style.background = style.background.or(Some(colors.code_background));
                }
                MarkKind::Highlight => style.background = Some(colors.highlight),
                MarkKind::Superscript | MarkKind::Subscript => {
                    style.font_size = Some(self.base_size * 0.72);
                }
                MarkKind::Spoiler => {
                    style.background = Some(colors.spoiler);
                    if self.read_only {
                        style.color = Some(colors.spoiler);
                    }
                }
                MarkKind::Link(link) => {
                    style.color = style.color.or(Some(colors.accent));
                    style.underline = Some(true);
                    if let Some(extra) = self.decorators.link_style(&link.url) {
                        *style = style.clone().merge(&extra);
                    }
                }
                MarkKind::Color(color) => style.color = Some((*color).into()),
                MarkKind::Background(color) => style.background = Some((*color).into()),
                MarkKind::FontFamily(family) => {
                    style.font_family = Some(Cow::Owned(family.clone()));
                }
                MarkKind::FontSize(size) => {
                    style.font_size = Some(self.theme.px(f32::from(*size)));
                }
            }
        }
        for (_, style) in matches
            .iter()
            .filter(|(found, _)| found.start <= range.start && range.end <= found.end)
        {
            look.style = look.style.clone().merge(style);
        }
        look
    }
}

fn span(text: &str, look: &Look, underline: bool) -> Span<'static> {
    let style = &look.style;
    let decoration = if look.strike {
        TextDecoration::LineThrough
    } else if underline {
        TextDecoration::Underline
    } else {
        TextDecoration::None
    };
    Span::new(text.to_owned())
        .map(style.color, freya::prelude::TextStyleExt::color)
        .map(style.font_weight, freya::prelude::TextStyleExt::font_weight)
        .map(style.font_size, freya::prelude::TextStyleExt::font_size)
        .map(style.font_family.clone(), |span, family| {
            span.font_family(family)
        })
        .maybe(style.italic == Some(true), |span| {
            span.font_slant(FontSlant::Italic)
        })
        .maybe(decoration != TextDecoration::None, |span| {
            span.text_decoration(decoration)
        })
}

/// Join touching ranges with the same decoration so rounded fills do not
/// show seams between spans.
fn merge_adjacent(decor: Vec<Decor>) -> Vec<Decor> {
    let mut merged: Vec<Decor> = Vec::with_capacity(decor.len());
    for item in decor {
        match merged
            .iter_mut()
            .rev()
            .find(|existing| existing.kind == item.kind)
        {
            Some(existing) if existing.range.end == item.range.start => {
                existing.range.end = item.range.end;
            }
            _ => merged.push(item),
        }
    }
    merged
}

trait SpanExt: Sized {
    fn map<T>(self, value: Option<T>, apply: impl FnOnce(Self, T) -> Self) -> Self;
    fn maybe(self, condition: bool, apply: impl FnOnce(Self) -> Self) -> Self;
}

impl SpanExt for Span<'static> {
    fn map<T>(self, value: Option<T>, apply: impl FnOnce(Self, T) -> Self) -> Self {
        match value {
            Some(value) => apply(self, value),
            None => self,
        }
    }

    fn maybe(self, condition: bool, apply: impl FnOnce(Self) -> Self) -> Self {
        if condition { apply(self) } else { self }
    }
}
