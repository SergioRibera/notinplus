use std::ops::Range;

use super::color::Rgba;

/// Placeholder char that stands in for every [`Atom`] inside [`Inline::text`].
///
/// Skia uses the same char for inline placeholders, so a paragraph built from
/// an [`Inline`] keeps the exact same index space as the text it renders.
pub const OBJECT_REPLACEMENT: char = '\u{FFFC}';

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Link {
    pub url: String,
    pub title: Option<String>,
}

impl Link {
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            title: None,
        }
    }
}

/// Formatting applied over a range of an [`Inline`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum MarkKind {
    Strong,
    Emphasis,
    Underline,
    Strikethrough,
    Code,
    Highlight,
    Superscript,
    Subscript,
    Spoiler,
    Link(Link),
    Color(Rgba),
    Background(Rgba),
    FontFamily(String),
    /// Font size in logical pixels.
    FontSize(u16),
    /// `<abbr title="...">`: the text abbreviates the title.
    Abbreviation(String),
}

/// Value-less identity of a [`MarkKind`]. Two marks of the same family never
/// overlap: applying a new color replaces the previous one on that range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MarkFamily {
    Strong,
    Emphasis,
    Underline,
    Strikethrough,
    Code,
    Highlight,
    Superscript,
    Subscript,
    Spoiler,
    Link,
    Color,
    Background,
    FontFamily,
    FontSize,
    Abbreviation,
}

impl MarkKind {
    #[must_use]
    pub const fn family(&self) -> MarkFamily {
        match self {
            Self::Strong => MarkFamily::Strong,
            Self::Emphasis => MarkFamily::Emphasis,
            Self::Underline => MarkFamily::Underline,
            Self::Strikethrough => MarkFamily::Strikethrough,
            Self::Code => MarkFamily::Code,
            Self::Highlight => MarkFamily::Highlight,
            Self::Superscript => MarkFamily::Superscript,
            Self::Subscript => MarkFamily::Subscript,
            Self::Spoiler => MarkFamily::Spoiler,
            Self::Link(_) => MarkFamily::Link,
            Self::Color(_) => MarkFamily::Color,
            Self::Background(_) => MarkFamily::Background,
            Self::FontFamily(_) => MarkFamily::FontFamily,
            Self::FontSize(_) => MarkFamily::FontSize,
            Self::Abbreviation(_) => MarkFamily::Abbreviation,
        }
    }

    /// Whether text typed right after the end of the mark joins it.
    pub(crate) const fn is_inclusive(&self) -> bool {
        !matches!(self, Self::Code | Self::Link(_) | Self::Abbreviation(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Mark {
    /// UTF-16 code unit range inside [`Inline::text`].
    pub range: Range<usize>,
    pub kind: MarkKind,
}

/// Non textual inline content. Each atom occupies one [`OBJECT_REPLACEMENT`]
/// char so it is deleted, selected and moved over as a single unit.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Atom {
    Image {
        url: String,
        alt: String,
        title: Option<String>,
    },
    /// A link claimed by a decorator and rendered as a custom widget.
    Embed {
        url: String,
        label: String,
        title: Option<String>,
    },
    FootnoteRef {
        label: String,
    },
    Math {
        tex: String,
    },
    Html {
        raw: String,
    },
}

impl Atom {
    /// Text used when the atom has to be flattened to plain text.
    #[must_use]
    pub fn plain_text(&self) -> &str {
        match self {
            Self::Image { alt, .. } => alt,
            Self::Embed { label, .. } | Self::FootnoteRef { label } => label,
            Self::Math { tex } => tex,
            Self::Html { raw } => raw,
        }
    }
}

/// A text change expressed against the previous text: `range` was replaced
/// by `inserted`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChange {
    pub range: Range<usize>,
    pub inserted: String,
}

/// Rich inline content: visible text plus formatting marks and atoms.
///
/// Every index is expressed in UTF-16 code units, the same unit
/// `freya-edit` uses for cursors and selections.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Inline {
    text: String,
    marks: Vec<Mark>,
    atoms: Vec<Atom>,
}

impl From<&str> for Inline {
    fn from(text: &str) -> Self {
        Self::from(text.to_owned())
    }
}

impl From<String> for Inline {
    fn from(mut text: String) -> Self {
        text.retain(|ch| ch != OBJECT_REPLACEMENT);
        Self {
            text,
            ..Self::default()
        }
    }
}

const AUTOFORMAT_RULES: [(&str, MarkKind); 10] = [
    ("**", MarkKind::Strong),
    ("__", MarkKind::Strong),
    ("~~", MarkKind::Strikethrough),
    ("==", MarkKind::Highlight),
    ("||", MarkKind::Spoiler),
    ("*", MarkKind::Emphasis),
    ("_", MarkKind::Emphasis),
    ("`", MarkKind::Code),
    ("^", MarkKind::Superscript),
    ("~", MarkKind::Subscript),
];

impl Inline {
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }

    #[must_use]
    pub fn atoms(&self) -> &[Atom] {
        &self.atoms
    }

    /// Length in UTF-16 code units.
    #[must_use]
    pub fn len(&self) -> usize {
        utf16_len(&self.text)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Text with every atom flattened to its [`Atom::plain_text`].
    #[must_use]
    pub fn plain_text(&self) -> String {
        let mut atoms = self.atoms.iter();
        self.text
            .chars()
            .fold(String::with_capacity(self.text.len()), |mut out, ch| {
                if ch == OBJECT_REPLACEMENT {
                    if let Some(atom) = atoms.next() {
                        out.push_str(atom.plain_text());
                    }
                } else {
                    out.push(ch);
                }
                out
            })
    }

    /// Atom whose placeholder sits at `pos`, if any.
    #[must_use]
    pub fn atom_at(&self, pos: usize) -> Option<&Atom> {
        let byte = byte_index(&self.text, pos);
        if !self.text[byte..].starts_with(OBJECT_REPLACEMENT) {
            return None;
        }
        self.atoms.get(self.atoms_before(byte))
    }

    /// Mark of `family` covering `pos`, if any.
    #[must_use]
    pub fn mark_at(&self, pos: usize, family: MarkFamily) -> Option<&Mark> {
        self.marks.iter().find(|mark| {
            mark.kind.family() == family && mark.range.start <= pos && pos < mark.range.end
        })
    }

    /// Marks that apply to text typed at a collapsed cursor at `pos`.
    #[must_use]
    pub fn kinds_at(&self, pos: usize) -> Vec<&MarkKind> {
        self.marks
            .iter()
            .filter(|mark| {
                let Range { start, end } = mark.range;
                if pos == 0 {
                    start == 0 && end > 0 && mark.kind.is_inclusive()
                } else if mark.kind.is_inclusive() {
                    start < pos && pos <= end
                } else {
                    start < pos && pos < end
                }
            })
            .map(|mark| &mark.kind)
            .collect()
    }

    /// Whether `range` is fully covered by marks of `family`. An empty range
    /// asks about the marks that typing at that position would inherit.
    #[must_use]
    pub fn covers(&self, range: Range<usize>, family: MarkFamily) -> bool {
        if range.is_empty() {
            return self
                .kinds_at(range.start)
                .iter()
                .any(|kind| kind.family() == family);
        }
        let mut covered: Vec<Range<usize>> = self
            .marks
            .iter()
            .filter(|mark| mark.kind.family() == family)
            .map(|mark| mark.range.start.max(range.start)..mark.range.end.min(range.end))
            .filter(|clipped| !clipped.is_empty())
            .collect();
        covered.sort_by_key(|clipped| clipped.start);
        let mut reached = range.start;
        for clipped in covered {
            if clipped.start > reached {
                return false;
            }
            reached = reached.max(clipped.end);
        }
        reached >= range.end
    }

    /// First mark of `family` overlapping `range` (or containing the cursor
    /// for an empty range).
    #[must_use]
    pub fn find_mark(&self, range: Range<usize>, family: MarkFamily) -> Option<&MarkKind> {
        if range.is_empty() {
            return self
                .kinds_at(range.start)
                .into_iter()
                .find(|kind| kind.family() == family)
                .or_else(|| self.mark_at(range.start, family).map(|mark| &mark.kind));
        }
        self.marks
            .iter()
            .find(|mark| {
                mark.kind.family() == family
                    && mark.range.start < range.end
                    && range.start < mark.range.end
            })
            .map(|mark| &mark.kind)
    }

    pub(crate) fn push_str(&mut self, text: &str, marks: &[MarkKind]) {
        let start = self.len();
        self.text
            .extend(text.chars().filter(|ch| *ch != OBJECT_REPLACEMENT));
        self.push_marks(start, marks);
    }

    pub(crate) fn push_atom(&mut self, atom: Atom, marks: &[MarkKind]) {
        let start = self.len();
        self.text.push(OBJECT_REPLACEMENT);
        self.atoms.push(atom);
        self.push_marks(start, marks);
    }

    fn push_marks(&mut self, start: usize, marks: &[MarkKind]) {
        let end = self.len();
        if start == end {
            return;
        }
        for kind in marks {
            match self
                .marks
                .iter_mut()
                .find(|mark| mark.range.end == start && mark.kind == *kind)
            {
                Some(mark) => mark.range.end = end,
                None => self.marks.push(Mark {
                    range: start..end,
                    kind: kind.clone(),
                }),
            }
        }
    }

    /// Replace `range` with plain `text`, keeping marks and atoms consistent.
    /// Returns the range the inserted text now occupies.
    pub fn replace(&mut self, range: Range<usize>, text: &str) -> Range<usize> {
        let len = self.len();
        let start = range.start.min(len);
        let end = range.end.clamp(start, len);
        let inserted: String = text
            .chars()
            .filter(|ch| *ch != OBJECT_REPLACEMENT)
            .collect();
        let inserted_len = utf16_len(&inserted);
        let removed = end - start;

        let start_byte = byte_index(&self.text, start);
        let end_byte = byte_index(&self.text, end);
        let first_atom = self.atoms_before(start_byte);
        let removed_atoms = self.text[start_byte..end_byte]
            .chars()
            .filter(|ch| *ch == OBJECT_REPLACEMENT)
            .count();
        self.atoms.drain(first_atom..first_atom + removed_atoms);
        self.text.replace_range(start_byte..end_byte, &inserted);

        for mark in &mut self.marks {
            let Range { start: s, end: e } = mark.range;
            let contained_removal = removed > 0 && s <= start && e >= end;
            let shrink = |x: usize| {
                if x <= start {
                    x
                } else if x < end {
                    start
                } else {
                    x - removed
                }
            };
            let (s, e) = (shrink(s), shrink(e));
            let inherits = contained_removal || mark.kind.is_inclusive();
            let (s, e) = if s < start && start < e {
                (s, e + inserted_len)
            } else if e == start && s < start {
                (s, if inherits { e + inserted_len } else { e })
            } else if s == start && (contained_removal || (start == 0 && inherits && e > s)) {
                (s, e + inserted_len)
            } else if s >= start {
                (s + inserted_len, e + inserted_len)
            } else {
                (s, e)
            };
            mark.range = s..e;
        }
        self.normalize();
        start..start + inserted_len
    }

    /// Apply the difference between the current text and `new_text`.
    /// `edit_start_hint` bounds where the change may begin (the selection
    /// start before the edit) so repeated characters resolve correctly.
    pub fn apply_text(&mut self, new_text: &str, edit_start_hint: usize) -> Option<TextChange> {
        let change = diff(&self.text, new_text, edit_start_hint)?;
        self.replace(change.range.clone(), &change.inserted);
        Some(change)
    }

    /// The `index`th atom, to change it in place.
    pub fn atom_mut(&mut self, index: usize) -> Option<&mut Atom> {
        self.atoms.get_mut(index)
    }

    /// UTF-16 position of the placeholder of the `index`th atom.
    #[must_use]
    pub fn atom_position(&self, index: usize) -> Option<usize> {
        let mut units = 0;
        let mut seen = 0;
        for ch in self.text.chars() {
            if ch == OBJECT_REPLACEMENT {
                if seen == index {
                    return Some(units);
                }
                seen += 1;
            }
            units += ch.len_utf16();
        }
        None
    }

    pub fn insert_atom(&mut self, at: usize, atom: Atom) {
        let at = at.min(self.len());
        let byte = byte_index(&self.text, at);
        let index = self.atoms_before(byte);
        self.text.insert(byte, OBJECT_REPLACEMENT);
        self.atoms.insert(index, atom);
        for mark in &mut self.marks {
            if mark.range.start >= at {
                mark.range = mark.range.start + 1..mark.range.end + 1;
            } else if mark.range.end > at {
                mark.range.end += 1;
            }
        }
    }

    pub fn insert_inline(&mut self, at: usize, other: Self) {
        let tail = self.split_off(at);
        self.append(other);
        self.append(tail);
    }

    #[must_use]
    pub fn slice(&self, range: Range<usize>) -> Self {
        let len = self.len();
        let start = range.start.min(len);
        let end = range.end.clamp(start, len);
        let start_byte = byte_index(&self.text, start);
        let end_byte = byte_index(&self.text, end);
        let first_atom = self.atoms_before(start_byte);
        let text = self.text[start_byte..end_byte].to_owned();
        let atom_count = text.chars().filter(|ch| *ch == OBJECT_REPLACEMENT).count();
        let marks = self
            .marks
            .iter()
            .filter_map(|mark| {
                let s = mark.range.start.max(start);
                let e = mark.range.end.min(end);
                (s < e).then(|| Mark {
                    range: s - start..e - start,
                    kind: mark.kind.clone(),
                })
            })
            .collect();
        Self {
            text,
            marks,
            atoms: self.atoms[first_atom..first_atom + atom_count].to_vec(),
        }
    }

    /// Split at `at`, keeping the head and returning the tail.
    #[must_use = "the tail is removed from `self`"]
    pub fn split_off(&mut self, at: usize) -> Self {
        let len = self.len();
        let tail = self.slice(at..len);
        *self = self.slice(0..at);
        tail
    }

    pub fn append(&mut self, other: Self) {
        let offset = self.len();
        self.text.push_str(&other.text);
        self.atoms.extend(other.atoms);
        self.marks.extend(other.marks.into_iter().map(|mark| Mark {
            range: mark.range.start + offset..mark.range.end + offset,
            kind: mark.kind,
        }));
        self.normalize();
    }

    /// Apply `kind` to `range`, replacing any mark of the same family there.
    pub fn add_mark(&mut self, range: Range<usize>, kind: MarkKind) {
        let range = range.start..range.end.min(self.len());
        if range.is_empty() {
            return;
        }
        self.remove_family(range.clone(), kind.family());
        self.marks.push(Mark { range, kind });
        self.normalize();
    }

    pub fn remove_family(&mut self, range: Range<usize>, family: MarkFamily) {
        let mut kept = Vec::with_capacity(self.marks.len());
        for mark in self.marks.drain(..) {
            let overlaps = mark.range.start < range.end && range.start < mark.range.end;
            if mark.kind.family() != family || !overlaps {
                kept.push(mark);
                continue;
            }
            if mark.range.start < range.start {
                kept.push(Mark {
                    range: mark.range.start..range.start,
                    kind: mark.kind.clone(),
                });
            }
            if range.end < mark.range.end {
                kept.push(Mark {
                    range: range.end..mark.range.end,
                    kind: mark.kind,
                });
            }
        }
        self.marks = kept;
        self.normalize();
    }

    /// Toggle `kind` over `range`. Returns whether the mark is now applied.
    pub fn toggle_mark(&mut self, range: Range<usize>, kind: MarkKind) -> bool {
        if self.covers(range.clone(), kind.family()) {
            self.remove_family(range, kind.family());
            false
        } else {
            self.add_mark(range, kind);
            true
        }
    }

    /// Turn a just typed `**text**` style sequence ending at `cursor` into
    /// the matching mark. Returns the new cursor and the applied family.
    pub fn autoformat(&mut self, cursor: usize) -> Option<(usize, MarkFamily)> {
        let cursor_byte = byte_index(&self.text, cursor);
        let before = &self.text[..cursor_byte];
        let (open_byte, close_byte, delimiter, kind) =
            AUTOFORMAT_RULES.iter().find_map(|(delimiter, kind)| {
                let close_byte = before.strip_suffix(delimiter)?.len();
                let search = &before[..close_byte];
                let open_byte = search.rfind(delimiter)?;
                let inner = &search[open_byte + delimiter.len()..];
                let preceding = search[..open_byte].chars().next_back();
                let delimiter_char = delimiter.chars().next()?;
                let valid = !inner.is_empty()
                    && !inner.starts_with(char::is_whitespace)
                    && !inner.ends_with(char::is_whitespace)
                    && !inner.contains('\n')
                    && !inner.starts_with(delimiter_char)
                    && !inner.ends_with(delimiter_char)
                    && preceding != Some(delimiter_char)
                    && !(delimiter_char == '_' && preceding.is_some_and(char::is_alphanumeric))
                    && !(*kind == MarkKind::Code && inner.contains('`'));
                valid.then(|| (open_byte, close_byte, delimiter.len(), kind.clone()))
            })?;

        let open = utf16_len(&self.text[..open_byte]);
        let close = utf16_len(&self.text[..close_byte]);
        self.replace(close..close + delimiter, "");
        self.replace(open..open + delimiter, "");
        let end = close - delimiter;
        let family = kind.family();
        self.add_mark(open..end, kind);
        Some((end, family))
    }

    fn atoms_before(&self, byte: usize) -> usize {
        self.text[..byte]
            .chars()
            .filter(|ch| *ch == OBJECT_REPLACEMENT)
            .count()
    }

    fn normalize(&mut self) {
        self.marks.retain(|mark| !mark.range.is_empty());
        self.marks.sort_by(|a, b| {
            a.kind
                .family()
                .cmp(&b.kind.family())
                .then(a.range.start.cmp(&b.range.start))
        });
        let mut merged: Vec<Mark> = Vec::with_capacity(self.marks.len());
        for mark in self.marks.drain(..) {
            let touches = |last: &Mark| last.range.end >= mark.range.start;
            match merged.last_mut() {
                Some(last) if last.kind == mark.kind && touches(last) => {
                    last.range.end = last.range.end.max(mark.range.end);
                }
                _ => merged.push(mark),
            }
        }
        merged.sort_by(|a, b| {
            a.range
                .start
                .cmp(&b.range.start)
                .then(a.kind.family().cmp(&b.kind.family()))
        });
        self.marks = merged;
    }
}

/// Number of UTF-16 code units in `text`.
#[must_use]
pub fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// Byte offset of the UTF-16 position `pos`, clamped to the text length and
/// rounded down to a char boundary.
#[must_use]
pub fn byte_index(text: &str, pos: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units + ch.len_utf16() > pos {
            return byte;
        }
        units += ch.len_utf16();
    }
    text.len()
}

/// Smallest single replacement turning `old` into `new`. The change never
/// starts after `start_hint` (UTF-16).
#[must_use]
pub fn diff(old: &str, new: &str, start_hint: usize) -> Option<TextChange> {
    if old == new {
        return None;
    }
    let mut prefix_bytes = 0;
    let mut prefix_units = 0;
    for (a, b) in old.chars().zip(new.chars()) {
        if a != b || prefix_units + a.len_utf16() > start_hint {
            break;
        }
        prefix_bytes += a.len_utf8();
        prefix_units += a.len_utf16();
    }
    let old_rest = &old[prefix_bytes..];
    let new_rest = &new[prefix_bytes..];
    let suffix_bytes: usize = old_rest
        .chars()
        .rev()
        .zip(new_rest.chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>()
        .min(old_rest.len())
        .min(new_rest.len());
    let removed = &old_rest[..old_rest.len() - suffix_bytes];
    let inserted = &new_rest[..new_rest.len() - suffix_bytes];
    Some(TextChange {
        range: prefix_units..prefix_units + utf16_len(removed),
        inserted: inserted.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold(range: Range<usize>) -> Mark {
        Mark {
            range,
            kind: MarkKind::Strong,
        }
    }

    fn sample() -> Inline {
        let mut inline = Inline::default();
        inline.push_str("hello ", &[]);
        inline.push_str("world", &[MarkKind::Strong]);
        inline
    }

    #[test]
    fn typing_after_bold_extends_it() {
        let mut inline = sample();
        inline.replace(11..11, "!");
        assert_eq!(inline.marks(), &[bold(6..12)]);
    }

    #[test]
    fn typing_before_bold_shifts_it() {
        let mut inline = sample();
        inline.replace(6..6, "x");
        assert_eq!(inline.marks(), &[bold(7..12)]);
    }

    #[test]
    fn typing_after_code_does_not_extend() {
        let mut inline = Inline::default();
        inline.push_str("code", &[MarkKind::Code]);
        inline.replace(4..4, "x");
        assert_eq!(inline.marks()[0].range, 0..4);
    }

    #[test]
    fn replacing_selection_inside_mark_keeps_mark() {
        let mut inline = sample();
        inline.replace(6..11, "there");
        assert_eq!(inline.text(), "hello there");
        assert_eq!(inline.marks(), &[bold(6..11)]);
    }

    #[test]
    fn deleting_marked_text_drops_mark() {
        let mut inline = sample();
        inline.replace(5..11, "");
        assert_eq!(inline.text(), "hello");
        assert!(inline.marks().is_empty());
    }

    #[test]
    fn atoms_follow_their_placeholder() {
        let mut inline = Inline::from("ab");
        inline.insert_atom(1, Atom::FootnoteRef { label: "1".into() });
        assert_eq!(inline.text(), "a\u{FFFC}b");
        assert!(inline.atom_at(1).is_some());
        inline.replace(1..2, "");
        assert!(inline.atoms().is_empty());
    }

    #[test]
    fn split_and_append_round_trip() {
        let original = sample();
        let mut head = original.clone();
        let tail = head.split_off(8);
        assert_eq!(head.text(), "hello wo");
        assert_eq!(tail.text(), "rld");
        assert_eq!(tail.marks(), &[bold(0..3)]);
        head.append(tail);
        assert_eq!(head, original);
    }

    #[test]
    fn toggling_partially_covered_range_applies() {
        let mut inline = sample();
        assert!(inline.toggle_mark(0..11, MarkKind::Strong));
        assert_eq!(inline.marks(), &[bold(0..11)]);
        assert!(!inline.toggle_mark(2..4, MarkKind::Strong));
        assert_eq!(inline.marks(), &[bold(0..2), bold(4..11)]);
    }

    #[test]
    fn same_family_marks_replace_each_other() {
        let mut inline = Inline::from("colors");
        inline.add_mark(0..6, MarkKind::Color(Rgba::rgb(255, 0, 0)));
        inline.add_mark(2..4, MarkKind::Color(Rgba::rgb(0, 0, 255)));
        let families: Vec<_> = inline.marks().iter().map(|m| m.range.clone()).collect();
        assert_eq!(families, vec![0..2, 2..4, 4..6]);
    }

    #[test]
    fn autoformat_strong() {
        let mut inline = Inline::from("say **hi**");
        let result = inline.autoformat(10);
        assert_eq!(result, Some((6, MarkFamily::Strong)));
        assert_eq!(inline.text(), "say hi");
        assert_eq!(inline.marks(), &[bold(4..6)]);
    }

    #[test]
    fn autoformat_ignores_spaced_content() {
        let mut inline = Inline::from("a * b *");
        assert_eq!(inline.autoformat(7), None);
    }

    #[test]
    fn diff_respects_start_hint_for_repeats() {
        let change = diff("aa", "aaa", 1).expect("change");
        assert_eq!(change.range, 1..1);
        assert_eq!(change.inserted, "a");
    }

    #[test]
    fn diff_handles_multibyte() {
        let change = diff("añb", "ab", 3).expect("change");
        assert_eq!(change.range, 1..2);
        assert_eq!(change.inserted, "");
    }

    #[test]
    fn utf16_indices_for_astral_chars() {
        let text = "a😀b";
        assert_eq!(utf16_len(text), 4);
        assert_eq!(byte_index(text, 3), 5);
    }
}
