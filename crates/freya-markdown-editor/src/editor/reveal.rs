//! Showing the markdown source of the formatting around the caret.
//!
//! While the caret touches text formatted with markdown syntax of its own
//! (bold, italic, code, links...), that run is shown and edited as its
//! source, `**like this**`. Every edit re-parses the run, so deleting a `*`
//! turns the bold back into plain text and typing after the closing `**`
//! never inherits it. Styling that only exists as HTML (underline, colors,
//! fonts) keeps being edited visually.

use std::ops::Range;

use crate::document::{Inline, MarkFamily, MarkKind, Rgba, byte_index, diff, utf16_len};
use crate::markdown::{ParseOptions, inline_source, parse_inline};

const VISUAL_FAMILIES: [MarkFamily; 5] = [
    MarkFamily::Underline,
    MarkFamily::Color,
    MarkFamily::Background,
    MarkFamily::FontFamily,
    MarkFamily::FontSize,
];

const SYNTAX_FAMILIES: [MarkFamily; 9] = [
    MarkFamily::Strong,
    MarkFamily::Emphasis,
    MarkFamily::Strikethrough,
    MarkFamily::Code,
    MarkFamily::Highlight,
    MarkFamily::Superscript,
    MarkFamily::Subscript,
    MarkFamily::Spoiler,
    MarkFamily::Link,
];

/// Whether the mark is written with markdown syntax rather than HTML.
const fn has_syntax(kind: &MarkKind) -> bool {
    !matches!(
        kind,
        MarkKind::Underline
            | MarkKind::Color(_)
            | MarkKind::Background(_)
            | MarkKind::FontFamily(_)
            | MarkKind::FontSize(_)
    )
}

/// A run of an [`Inline`] currently shown as markdown source.
///
/// The text before the run is the same in the model and on screen, the text
/// after it is shifted by the difference in length.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reveal {
    start: usize,
    /// The model content the source stands for.
    model: Inline,
    source: String,
    /// Model offset (inside `model`) for every UTF-16 position of `source`,
    /// end inclusive.
    to_model: Vec<usize>,
    /// Model offset of the content char at each source position, `None` for
    /// syntax.
    content: Vec<Option<usize>>,
}

impl Reveal {
    /// Reveal the formatting touching `cursor`, if any.
    pub(crate) fn open(content: &Inline, cursor: usize) -> Option<Self> {
        let range = region(content, cursor)?;
        let model = content.slice(range.clone());
        if !model.atoms().is_empty() {
            return None;
        }
        let mut syntax_only = model.clone();
        for family in VISUAL_FAMILIES {
            syntax_only.remove_family(0..syntax_only.len(), family);
        }
        let source = inline_source(&syntax_only);
        Some(Self::new(range.start, model, source))
    }

    fn new(start: usize, model: Inline, source: String) -> Self {
        let (to_model, content) = align(&source, model.text());
        Self {
            start,
            model,
            source,
            to_model,
            content,
        }
    }

    fn source_len(&self) -> usize {
        self.to_model.len() - 1
    }

    fn model_range(&self) -> Range<usize> {
        self.start..self.start + self.model.len()
    }

    pub(crate) fn display_range(&self) -> Range<usize> {
        self.start..self.start + self.source_len()
    }

    /// Whether `content` still holds what this source was derived from.
    pub(crate) fn is_current(&self, content: &Inline) -> bool {
        let range = self.model_range();
        range.end <= content.len() && content.slice(range) == self.model
    }

    pub(crate) fn contains(&self, display: usize) -> bool {
        self.display_range().contains(&display) || display == self.display_range().end
    }

    /// Model offset for a position in the shown text.
    pub(crate) fn to_model(&self, display: usize) -> usize {
        let range = self.display_range();
        if display <= range.start {
            display
        } else if display >= range.end {
            display - range.len() + self.model.len()
        } else {
            self.start + self.to_model[display - range.start]
        }
    }

    /// Position in the shown text for a model offset.
    pub(crate) fn to_display(&self, model: usize) -> usize {
        let range = self.model_range();
        if model <= range.start {
            model
        } else if model >= range.end {
            model - range.len() + self.source_len()
        } else {
            let inner = model - range.start;
            let position = self
                .content
                .iter()
                .position(|offset| *offset == Some(inner))
                .or_else(|| self.to_model.iter().position(|offset| *offset >= inner))
                .unwrap_or_else(|| self.source_len());
            self.start + position
        }
    }

    /// The text shown on screen for `content`.
    pub(crate) fn display_text(&self, content: &Inline) -> String {
        let text = content.text();
        let range = self.model_range();
        let start = byte_index(text, range.start);
        let end = byte_index(text, range.end);
        format!("{}{}{}", &text[..start], self.source, &text[end..])
    }

    /// `content` with the run replaced by its source: content chars keep
    /// their formatting and syntax is drawn in `syntax_color`.
    pub(crate) fn display_inline(&self, content: &Inline, syntax_color: Rgba) -> Inline {
        let mut shown = Inline::from(self.source.as_str());
        for mark in self.model.marks() {
            let covered = self
                .content
                .iter()
                .enumerate()
                .filter(|(_, offset)| offset.is_some_and(|offset| mark.range.contains(&offset)))
                .map(|(position, _)| position);
            let (first, last) = covered.fold((None, None), |(first, _), position| {
                (first.or(Some(position)), Some(position))
            });
            if let (Some(first), Some(last)) = (first, last) {
                shown.add_mark(first..last + 1, mark.kind.clone());
            }
        }
        let mut position = 0;
        while position < self.content.len() {
            if self.content[position].is_some() {
                position += 1;
                continue;
            }
            let start = position;
            while position < self.content.len() && self.content[position].is_none() {
                position += 1;
            }
            shown.add_mark(start..position, MarkKind::Color(syntax_color));
        }
        let range = self.model_range();
        let mut out = content.slice(0..range.start);
        out.append(shown);
        out.append(content.slice(range.end..content.len()));
        out
    }
}

/// Outcome of an edit made on the shown text.
pub(crate) enum RevealEdit {
    /// The edit happened outside the source: apply it to the model text.
    Outside { model_text: String, reveal: Reveal },
    /// The source changed: this is the new model content.
    Source { content: Inline, reveal: Reveal },
}

impl Reveal {
    /// Map an edit of the shown text back to the model. `shown` is the new
    /// text on screen, `hint` the selection start before the edit.
    pub(crate) fn edit(
        &self,
        content: &Inline,
        shown: &str,
        hint: usize,
        options: &ParseOptions,
    ) -> Option<RevealEdit> {
        let before = self.display_text(content);
        let change = diff(&before, shown, hint)?;
        let display = self.display_range();
        let inserted = utf16_len(&change.inserted);
        if change.range.end < display.start || change.range.start > display.end {
            let model_range = self.to_model(change.range.start)..self.to_model(change.range.end);
            let text = content.text();
            let model_text = format!(
                "{}{}{}",
                &text[..byte_index(text, model_range.start)],
                change.inserted,
                &text[byte_index(text, model_range.end)..]
            );
            let mut reveal = self.clone();
            if change.range.end < display.start {
                reveal.start = reveal.start + inserted - change.range.len();
            }
            return Some(RevealEdit::Outside { model_text, reveal });
        }

        let start = change.range.start.min(display.start);
        let end = change.range.end.max(display.end);
        let source_end = end + inserted - change.range.len();
        let source = shown[byte_index(shown, start)..byte_index(shown, source_end)].to_owned();
        let model_range = self.to_model(start)..self.to_model(end);
        let old = content.slice(model_range.clone());
        let run = reparse(&source, &old, options);
        let mut updated = content.slice(0..model_range.start);
        updated.append(run.clone());
        updated.append(content.slice(model_range.end..content.len()));
        Some(RevealEdit::Source {
            content: updated,
            reveal: Self::new(model_range.start, run, source),
        })
    }
}

/// The run of syntax formatted text touching `cursor`: every mark with
/// syntax containing it, grown over the marks overlapping or touching them.
fn region(content: &Inline, cursor: usize) -> Option<Range<usize>> {
    let marks: Vec<Range<usize>> = content
        .marks()
        .iter()
        .filter(|mark| has_syntax(&mark.kind))
        .map(|mark| mark.range.clone())
        .collect();
    let mut range: Option<Range<usize>> = None;
    for mark in &marks {
        if mark.start <= cursor && cursor <= mark.end {
            range = Some(range.map_or_else(
                || mark.clone(),
                |range| range.start.min(mark.start)..range.end.max(mark.end),
            ));
        }
    }
    let mut range = range?;
    loop {
        let grown = marks.iter().fold(range.clone(), |range, mark| {
            if mark.start <= range.end && range.start <= mark.end {
                range.start.min(mark.start)..range.end.max(mark.end)
            } else {
                range
            }
        });
        if grown == range {
            return Some(range);
        }
        range = grown;
    }
}

/// Match the chars of `plain` inside `source` in order. Everything left over
/// is syntax. Link destinations and HTML tags are skipped as a whole so
/// their letters never stand for content.
fn align(source: &str, plain: &str) -> (Vec<usize>, Vec<Option<usize>>) {
    let plain: Vec<char> = plain.chars().collect();
    let chars: Vec<char> = source.chars().collect();
    let mut to_model = Vec::with_capacity(source.len() + 1);
    let mut content = Vec::with_capacity(source.len());
    let mut next = 0;
    let mut offset = 0;
    let mut skip_until: Option<char> = None;
    let mut previous_syntax: Option<char> = None;
    for (index, &ch) in chars.iter().enumerate() {
        let expected = plain.get(next).copied();
        let matched = if let Some(close) = skip_until {
            if ch == close {
                skip_until = None;
            }
            false
        } else if ch == '(' && previous_syntax == Some(']') && expected != Some('(') {
            skip_until = Some(')');
            false
        } else if ch == '<' && expected != Some('<') && starts_tag(&chars[index + 1..]) {
            skip_until = Some('>');
            false
        } else if ch == '\\'
            && chars
                .get(index + 1)
                .is_some_and(|next| Some(*next) == expected)
        {
            false
        } else {
            expected == Some(ch)
        };
        for _ in 0..ch.len_utf16() {
            to_model.push(offset);
            content.push(matched.then_some(offset));
        }
        if matched {
            offset += ch.len_utf16();
            next += 1;
            previous_syntax = None;
        } else {
            previous_syntax = Some(ch);
        }
    }
    to_model.push(offset);
    (to_model, content)
}

fn starts_tag(rest: &[char]) -> bool {
    let name = rest.strip_prefix(&['/']).unwrap_or(rest);
    name.first().is_some_and(char::is_ascii_alphabetic) && rest.contains(&'>')
}

/// New content for an edited source. Styling without syntax that `old` had
/// is kept on the text that survived the edit.
fn reparse(source: &str, old: &Inline, options: &ParseOptions) -> Inline {
    let core_start = source.trim_start();
    let lead = &source[..source.len() - core_start.len()];
    let core = core_start.trim_end();
    let trail = &core_start[core.len()..];
    let parsed = parse_inline(core, options)
        .or_else(|| parse_inline(&escape_block_start(core), options))
        .unwrap_or_else(|| Inline::from(core));
    let mut new = Inline::from(lead);
    new.append(parsed);
    new.append(Inline::from(trail));
    if !new.atoms().is_empty() {
        return new;
    }

    let mut kept = old.clone();
    if let Some(change) = diff(old.text(), new.text(), usize::MAX) {
        kept.replace(change.range, &change.inserted);
    }
    if kept.text() != new.text() {
        return new;
    }
    for family in SYNTAX_FAMILIES {
        kept.remove_family(0..kept.len(), family);
    }
    for mark in new.marks() {
        kept.add_mark(mark.range.clone(), mark.kind.clone());
    }
    kept
}

/// Escape what would make `source` parse as a heading, list or quote.
fn escape_block_start(source: &str) -> String {
    let marker = source
        .char_indices()
        .find(|(_, ch)| !ch.is_ascii_digit())
        .filter(|(_, ch)| ch.is_ascii_punctuation());
    match marker {
        Some((index, _)) => format!("{}\\{}", &source[..index], &source[index..]),
        None => source.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(markdown: &str) -> Inline {
        parse_inline(markdown, &ParseOptions::default()).expect("a paragraph")
    }

    fn edit(reveal: &Reveal, content: &Inline, shown: &str) -> (Inline, Reveal) {
        let hint = reveal
            .display_text(content)
            .chars()
            .zip(shown.chars())
            .take_while(|(a, b)| a == b)
            .count();
        match reveal.edit(content, shown, hint, &ParseOptions::default()) {
            Some(RevealEdit::Source { content, reveal }) => (content, reveal),
            Some(RevealEdit::Outside { model_text, reveal }) => {
                let mut content = content.clone();
                content.apply_text(&model_text, hint);
                (content, reveal)
            }
            None => (content.clone(), reveal.clone()),
        }
    }

    #[test]
    fn caret_after_bold_shows_its_source() {
        let content = parse("hello **world**");
        let reveal = Reveal::open(&content, 11).expect("touching the bold");
        assert_eq!(reveal.display_text(&content), "hello **world**");
        assert_eq!(reveal.to_display(11), 15);
        assert_eq!(reveal.to_display(6), 6);
        assert_eq!(reveal.to_display(8), 10);
        assert_eq!(reveal.to_model(15), 11);
        assert_eq!(reveal.to_model(7), 6);
    }

    #[test]
    fn plain_text_is_not_revealed() {
        let content = parse("hello **world** again");
        assert!(Reveal::open(&content, 2).is_none());
        assert!(Reveal::open(&content, 18).is_none());
    }

    #[test]
    fn visual_only_styling_is_not_revealed() {
        let content = parse("<u>under</u> text");
        assert!(Reveal::open(&content, 3).is_none());
    }

    #[test]
    fn typing_after_the_closing_delimiter_stays_plain() {
        let content = parse("**bold**");
        let reveal = Reveal::open(&content, 4).expect("revealed");
        let (content, _) = edit(&reveal, &content, "**bold**x");
        assert_eq!(content.text(), "boldx");
        assert!(content.mark_at(4, MarkFamily::Strong).is_none());
        assert!(content.mark_at(0, MarkFamily::Strong).is_some());
    }

    #[test]
    fn deleting_a_delimiter_turns_formatting_into_text() {
        let content = parse("**bold**");
        let reveal = Reveal::open(&content, 4).expect("revealed");
        let (content, reveal) = edit(&reveal, &content, "**bold*");
        assert_eq!(content.text(), "*bold");
        assert!(
            content
                .marks()
                .iter()
                .all(|mark| mark.kind != MarkKind::Strong)
        );
        assert!(content.mark_at(1, MarkFamily::Emphasis).is_some());
        assert_eq!(reveal.display_text(&content), "**bold*");

        let (content, _) = edit(&reveal, &content, "**bold");
        assert_eq!(content.text(), "**bold");
        assert!(content.marks().is_empty());
    }

    #[test]
    fn edits_before_the_source_move_it() {
        let content = parse("ab **c**");
        let reveal = Reveal::open(&content, 3).expect("revealed");
        let (content, reveal) = edit(&reveal, &content, "aXb **c**");
        assert_eq!(content.text(), "aXb c");
        assert!(reveal.is_current(&content));
        assert_eq!(reveal.display_text(&content), "aXb **c**");
    }

    #[test]
    fn colors_survive_editing_the_source() {
        let content = parse("**<span style=\"color: #ff0000\">red</span>**");
        let reveal = Reveal::open(&content, 1).expect("revealed");
        assert!(!reveal.display_text(&content).contains("span"));
        let shown = reveal.display_text(&content).replace("red", "reed");
        let (content, _) = edit(&reveal, &content, &shown);
        assert_eq!(content.text(), "reed");
        assert!(content.mark_at(1, MarkFamily::Color).is_some());
        assert!(content.mark_at(1, MarkFamily::Strong).is_some());
    }

    #[test]
    fn link_destination_is_syntax() {
        let content = parse("[docs](https://d.example) **d**");
        let reveal = Reveal::open(&content, 2).expect("revealed");
        let shown = reveal.display_inline(&content, Rgba::rgb(1, 2, 3));
        let url_at = shown.text().find("https").expect("url shown");
        assert!(shown.mark_at(url_at, MarkFamily::Color).is_some());
        assert!(shown.mark_at(1, MarkFamily::Link).is_some());
    }

    #[test]
    fn heading_like_source_stays_inline() {
        let content = parse("**a**");
        let reveal = Reveal::open(&content, 0).expect("revealed");
        let (content, _) = edit(&reveal, &content, "# **a**");
        assert_eq!(content.text(), "# a");
        assert!(content.mark_at(2, MarkFamily::Strong).is_some());
    }
}
