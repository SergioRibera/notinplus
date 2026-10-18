use std::collections::BTreeSet;

use super::Dialect;
use super::html::span_open;
use crate::document::{
    Align, Atom, Block, BlockKind, ColumnAlign, Document, Inline, ListKind, Mark, MarkFamily,
    MarkKind, OBJECT_REPLACEMENT, Table, byte_index,
};

pub(super) fn serialize(document: &Document, dialect: Dialect) -> String {
    let mut writer = Writer {
        document,
        dialect,
        list_columns: Vec::new(),
    };
    let mut out = String::new();
    let mut previous: Option<&BlockKind> = None;
    for (index, block) in document.blocks().iter().enumerate() {
        let Some(chunk) = writer.block(block, index) else {
            continue;
        };
        if let Some(previous) = previous {
            out.push_str(if tight(previous, &block.kind) {
                "\n"
            } else {
                "\n\n"
            });
        }
        out.push_str(&chunk);
        previous = Some(&block.kind);
    }
    if dialect == Dialect::Rich && !document.abbreviations.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        let lines: Vec<String> = document
            .abbreviations
            .iter()
            .map(|abbr| format!("*[{}]: {}", abbr.term, abbr.expansion))
            .collect();
        out.push_str(&lines.join("\n"));
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

const fn tight(previous: &BlockKind, current: &BlockKind) -> bool {
    matches!(
        (previous, current),
        (BlockKind::ListItem { .. }, BlockKind::ListItem { .. })
            | (
                BlockKind::Term(_) | BlockKind::Definition(_),
                BlockKind::Definition(_)
            )
    )
}

struct Writer<'d> {
    document: &'d Document,
    dialect: Dialect,
    /// Content column of the last list item seen at every depth.
    list_columns: Vec<usize>,
}

impl Writer<'_> {
    #[allow(clippy::too_many_lines, reason = "one arm per block kind")]
    fn block(&mut self, block: &Block, index: usize) -> Option<String> {
        if !matches!(block.kind, BlockKind::ListItem { .. }) {
            self.list_columns.clear();
        }
        let rich = self.dialect == Dialect::Rich;
        let chunk = match &block.kind {
            BlockKind::Paragraph(content) => {
                if content.is_empty() {
                    return None;
                }
                self.inline(content, InlineContext::Block)
            }
            BlockKind::Heading {
                level,
                content,
                anchor,
            } => {
                let mut line = format!(
                    "{} {}",
                    "#".repeat(level.get().into()),
                    self.inline(content, InlineContext::SingleLine)
                );
                if rich && let Some(anchor) = anchor {
                    line = format!("{line} {{#{anchor}}}");
                }
                line
            }
            BlockKind::Quote(content) => {
                prefix_lines(&self.inline(content, InlineContext::Block), "> ", "> ")
            }
            BlockKind::Callout {
                kind,
                title,
                content,
            } => {
                let mut header = format!("> [!{}]", kind.marker());
                if let Some(title) = title {
                    header.push(' ');
                    header.push_str(title);
                }
                if content.is_empty() {
                    header
                } else {
                    let body = self.inline(content, InlineContext::Block);
                    format!("{header}\n{}", prefix_lines(&body, "> ", "> "))
                }
            }
            BlockKind::ListItem {
                kind,
                depth,
                content,
            } => self.list_item(block, *kind, *depth, content),
            BlockKind::Code { language, code } => fenced(language, code.text()),
            BlockKind::Math(tex) => format!("$$\n{}\n$$", tex.text()),
            BlockKind::Divider => "---".to_owned(),
            BlockKind::Table(table) => self.table(table),
            BlockKind::Footnote { label, content } => {
                let body = self.inline(content, InlineContext::Block);
                prefix_lines(&body, &format!("[^{label}]: "), "    ")
            }
            BlockKind::Term(content) => {
                let term = self.inline(content, InlineContext::SingleLine);
                if rich { term } else { format!("**{term}**") }
            }
            BlockKind::Definition(content) => {
                let body = self.inline(content, InlineContext::Block);
                if rich {
                    prefix_lines(&body, ": ", "  ")
                } else {
                    body
                }
            }
            BlockKind::FrontMatter(raw) => {
                if index == 0 {
                    format!("---\n{}\n---", raw.text())
                } else {
                    fenced("yaml", raw.text())
                }
            }
            BlockKind::TableOfContents => {
                if rich {
                    "[TOC]".to_owned()
                } else {
                    self.table_of_contents()?
                }
            }
            BlockKind::Html(raw) => raw.text().to_owned(),
        };
        let wrap = rich
            && block.align != Align::Left
            && matches!(
                block.kind,
                BlockKind::Paragraph(_) | BlockKind::Heading { .. }
            );
        Some(if wrap {
            format!(
                "<div align=\"{}\">\n\n{chunk}\n\n</div>",
                block.align.as_css()
            )
        } else {
            chunk
        })
    }

    fn list_item(&mut self, block: &Block, kind: ListKind, depth: u8, content: &Inline) -> String {
        let depth = usize::from(depth).min(self.list_columns.len());
        let base = depth
            .checked_sub(1)
            .and_then(|parent| self.list_columns.get(parent))
            .copied()
            .unwrap_or(0);
        let marker = match kind {
            ListKind::Bullet => "- ".to_owned(),
            ListKind::Ordered { .. } => {
                format!("{}. ", self.document.ordinal(block.id).unwrap_or(1))
            }
            ListKind::Task { checked } => {
                if checked {
                    "- [x] ".to_owned()
                } else {
                    "- [ ] ".to_owned()
                }
            }
        };
        let nesting_width = match kind {
            ListKind::Task { .. } => 2,
            _ => marker.len(),
        };
        self.list_columns.truncate(depth);
        self.list_columns.push(base + nesting_width);

        let indent = " ".repeat(base);
        let body = self.inline(content, InlineContext::Block);
        let first = format!("{indent}{marker}");
        let rest = " ".repeat(base + marker.len());
        if body.is_empty() {
            first.trim_end().to_owned()
        } else {
            prefix_lines(&body, &first, &rest)
        }
    }

    fn table(&self, table: &Table) -> String {
        let cells: Vec<Vec<String>> = table
            .rows()
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| self.inline(cell, InlineContext::TableCell))
                    .collect()
            })
            .collect();
        let widths: Vec<usize> = (0..table.columns().len())
            .map(|column| {
                cells
                    .iter()
                    .map(|row| row[column].chars().count())
                    .max()
                    .unwrap_or(0)
                    .max(3)
            })
            .collect();
        let line = |row: &[String]| {
            let padded: Vec<String> = row
                .iter()
                .zip(&widths)
                .map(|(cell, width)| {
                    let pad = width - cell.chars().count();
                    format!("{cell}{}", " ".repeat(pad))
                })
                .collect();
            format!("| {} |", padded.join(" | "))
        };
        let separator: Vec<String> = table
            .columns()
            .iter()
            .zip(&widths)
            .map(|(align, width)| {
                let dashes = |n: usize| "-".repeat(n);
                match align {
                    ColumnAlign::None => dashes(*width),
                    ColumnAlign::Left => format!(":{}", dashes(width - 1)),
                    ColumnAlign::Center => format!(":{}:", dashes(width - 2)),
                    ColumnAlign::Right => format!("{}:", dashes(width - 1)),
                }
            })
            .collect();
        let mut lines = vec![line(&cells[0]), format!("| {} |", separator.join(" | "))];
        lines.extend(cells[1..].iter().map(|row| line(row)));
        lines.join("\n")
    }

    fn table_of_contents(&self) -> Option<String> {
        let headings: Vec<_> = self.document.headings().collect();
        let min_level = headings.iter().map(|(_, level, _)| level.get()).min()?;
        let block_anchor = |id| {
            self.document.block(id).and_then(|block| match &block.kind {
                BlockKind::Heading { anchor, .. } => anchor.clone(),
                _ => None,
            })
        };
        let lines: Vec<String> = headings
            .iter()
            .map(|(id, level, content)| {
                let text = content.plain_text();
                let anchor = block_anchor(*id).unwrap_or_else(|| slug(&text));
                format!(
                    "{}- [{}](#{anchor})",
                    "  ".repeat(usize::from(level.get() - min_level)),
                    escape_text(&text, false, InlineContext::SingleLine)
                )
            })
            .collect();
        Some(lines.join("\n"))
    }

    fn inline(&self, inline: &Inline, context: InlineContext) -> String {
        InlineWriter::new(inline, self.dialect, context).write()
    }
}

/// Markdown source of `inline` as written inside a paragraph.
pub(super) fn inline_source(inline: &Inline) -> String {
    InlineWriter::new(inline, Dialect::Rich, InlineContext::Block).write()
}

fn fenced(language: &str, code: &str) -> String {
    let longest_run = code
        .lines()
        .map(|line| {
            line.trim_start()
                .chars()
                .take_while(|ch| *ch == '`')
                .count()
        })
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest_run.max(2) + 1);
    format!("{fence}{language}\n{code}\n{fence}")
}

fn prefix_lines(body: &str, first: &str, rest: &str) -> String {
    body.split('\n')
        .enumerate()
        .map(|(index, line)| {
            let prefix = if index == 0 { first } else { rest };
            if line.is_empty() {
                prefix.trim_end().to_owned()
            } else {
                format!("{prefix}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// GitHub style heading slug.
#[must_use]
pub fn slug(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .filter_map(|ch| match ch {
            ' ' => Some('-'),
            '-' | '_' => Some(ch),
            ch if ch.is_alphanumeric() => Some(ch),
            _ => None,
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InlineContext {
    Block,
    SingleLine,
    TableCell,
}

/// Group of marks opened together and closed together.
struct Open {
    kinds: Vec<MarkKind>,
    /// Byte offset in the output where a code span's content starts.
    code_start: Option<usize>,
    closing: String,
}

struct InlineWriter<'i> {
    inline: &'i Inline,
    dialect: Dialect,
    context: InlineContext,
    marks: Vec<Mark>,
    out: String,
    stack: Vec<Open>,
    line_start: bool,
}

impl<'i> InlineWriter<'i> {
    fn new(inline: &'i Inline, dialect: Dialect, context: InlineContext) -> Self {
        let marks = effective_marks(inline, dialect);
        Self {
            inline,
            dialect,
            context,
            marks,
            out: String::new(),
            stack: Vec::new(),
            line_start: true,
        }
    }

    fn write(mut self) -> String {
        let text = self.inline.text();
        let len = self.inline.len();
        let mut bounds: BTreeSet<usize> = BTreeSet::from([0, len]);
        for mark in &self.marks {
            bounds.insert(mark.range.start);
            bounds.insert(mark.range.end);
        }
        let mut atoms = self.inline.atoms().iter();
        let mut units = 0;
        for ch in text.chars() {
            if ch == OBJECT_REPLACEMENT {
                bounds.insert(units);
                bounds.insert(units + 1);
            }
            units += ch.len_utf16();
        }

        let bounds: Vec<usize> = bounds.into_iter().collect();
        for window in bounds.windows(2) {
            let (start, end) = (window[0], window[1]);
            self.transition(start, end);
            let segment = &text[byte_index(text, start)..byte_index(text, end)];
            if segment == OBJECT_REPLACEMENT.to_string() {
                if let Some(atom) = atoms.next() {
                    self.atom(atom);
                }
            } else {
                self.segment(segment);
            }
        }
        while !self.stack.is_empty() {
            self.close_top();
        }
        self.out
    }

    fn active(&self, start: usize, end: usize) -> Vec<&Mark> {
        self.marks
            .iter()
            .filter(|mark| mark.range.start <= start && end <= mark.range.end)
            .collect()
    }

    fn transition(&mut self, start: usize, end: usize) {
        let active: Vec<Mark> = self.active(start, end).into_iter().cloned().collect();
        let still_open = |open: &Open| {
            open.kinds
                .iter()
                .all(|kind| active.iter().any(|mark| mark.kind == *kind))
        };
        if let Some(lowest) = self.stack.iter().position(|open| !still_open(open)) {
            while self.stack.len() > lowest {
                self.close_top();
            }
        }

        let mut pending: Vec<&Mark> = active
            .iter()
            .filter(|mark| {
                !self
                    .stack
                    .iter()
                    .any(|open| open.kinds.contains(&mark.kind))
            })
            .collect();
        pending.sort_by(|a, b| {
            b.range
                .end
                .cmp(&a.range.end)
                .then(priority(&a.kind).cmp(&priority(&b.kind)))
        });

        let mut index = 0;
        while index < pending.len() {
            let mark = pending[index];
            if is_style(&mark.kind) {
                let group: Vec<&Mark> = pending[index..]
                    .iter()
                    .take_while(|other| is_style(&other.kind) && other.range.end == mark.range.end)
                    .copied()
                    .collect();
                index += group.len();
                let kinds: Vec<&MarkKind> = group.iter().map(|mark| &mark.kind).collect();
                self.out.push_str(&span_open(&kinds));
                self.stack.push(Open {
                    kinds: kinds.into_iter().cloned().collect(),
                    code_start: None,
                    closing: "</span>".to_owned(),
                });
            } else {
                index += 1;
                self.open(mark);
            }
        }
    }

    fn open(&mut self, mark: &Mark) {
        let (opening, closing) = self.delimiters(mark);
        self.out.push_str(&opening);
        if !opening.is_empty() {
            self.line_start = false;
        }
        let code_start = (mark.kind == MarkKind::Code).then_some(self.out.len());
        self.stack.push(Open {
            kinds: vec![mark.kind.clone()],
            code_start,
            closing,
        });
    }

    fn close_top(&mut self) {
        let Some(open) = self.stack.pop() else {
            return;
        };
        if let Some(code_start) = open.code_start {
            let content = self.out.split_off(code_start);
            self.out.push_str(&code_span(&content));
        } else {
            self.out.push_str(&open.closing);
        }
    }

    fn delimiters(&self, mark: &Mark) -> (String, String) {
        let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
        let flanking = self.flanking(mark);
        match &mark.kind {
            MarkKind::Strong if flanking => pair("**", "**"),
            MarkKind::Strong => pair("<strong>", "</strong>"),
            MarkKind::Emphasis if flanking => pair("*", "*"),
            MarkKind::Emphasis => pair("<em>", "</em>"),
            MarkKind::Strikethrough if flanking => pair("~~", "~~"),
            MarkKind::Strikethrough => pair("<del>", "</del>"),
            MarkKind::Highlight if flanking => pair("==", "=="),
            MarkKind::Highlight => pair("<mark>", "</mark>"),
            MarkKind::Spoiler => pair("||", "||"),
            MarkKind::Underline => match self.dialect {
                Dialect::Rich => pair("<u>", "</u>"),
                Dialect::Portable => pair("<ins>", "</ins>"),
            },
            MarkKind::Superscript => pair("<sup>", "</sup>"),
            MarkKind::Subscript => pair("<sub>", "</sub>"),
            MarkKind::Link(link) => {
                let destination = link_destination(&link.url);
                let title = link
                    .title
                    .as_ref()
                    .map(|title| format!(" \"{}\"", title.replace('"', "\\\"")))
                    .unwrap_or_default();
                ("[".to_owned(), format!("]({destination}{title})"))
            }
            // Code content is buffered and wrapped on close, styles are
            // grouped into a single span by the caller.
            MarkKind::Code
            | MarkKind::Color(_)
            | MarkKind::Background(_)
            | MarkKind::FontFamily(_)
            | MarkKind::FontSize(_) => pair("", ""),
        }
    }

    /// Whether `*`-like delimiters around `mark` satisfy the `CommonMark`
    /// flanking rules given the surrounding characters.
    fn flanking(&self, mark: &Mark) -> bool {
        let text = self.inline.text();
        let at = |pos: usize| text[byte_index(text, pos)..].chars().next();
        let before = mark.range.start.checked_sub(1).and_then(at);
        let first = at(mark.range.start);
        let last = mark.range.end.checked_sub(1).and_then(at);
        let after = at(mark.range.end);
        let loose =
            |ch: Option<char>| ch.is_none_or(|c| c.is_whitespace() || c.is_ascii_punctuation());
        let opens = first.is_some_and(|c| !c.is_whitespace())
            && (!first.is_some_and(|c| c.is_ascii_punctuation()) || loose(before));
        let closes = last.is_some_and(|c| !c.is_whitespace())
            && (!last.is_some_and(|c| c.is_ascii_punctuation()) || loose(after));
        opens && closes
    }

    fn segment(&mut self, segment: &str) {
        let in_code = self
            .stack
            .last()
            .is_some_and(|open| open.code_start.is_some());
        if in_code {
            self.out.push_str(&segment.replace('\n', " "));
            return;
        }
        let escaped = escape_text(segment, self.line_start, self.context);
        if !escaped.is_empty() {
            self.line_start = escaped.ends_with('\n');
        }
        self.out.push_str(&escaped);
    }

    fn atom(&mut self, atom: &Atom) {
        self.line_start = false;
        let rendered = match atom {
            Atom::Image { url, alt, title } => {
                let title = title
                    .as_ref()
                    .map(|title| format!(" \"{}\"", title.replace('"', "\\\"")))
                    .unwrap_or_default();
                format!(
                    "![{}]({}{title})",
                    escape_text(alt, false, InlineContext::SingleLine),
                    link_destination(url)
                )
            }
            Atom::Embed { url, label, title } => {
                let title = title
                    .as_ref()
                    .map(|title| format!(" \"{}\"", title.replace('"', "\\\"")))
                    .unwrap_or_default();
                format!(
                    "[{}]({}{title})",
                    escape_text(label, false, InlineContext::SingleLine),
                    link_destination(url)
                )
            }
            Atom::FootnoteRef { label } => format!("[^{label}]"),
            Atom::Math { tex } => {
                if tex.contains('$') || tex.starts_with(' ') || tex.ends_with(' ') {
                    format!("$`{tex}`$")
                } else {
                    format!("${tex}$")
                }
            }
            Atom::Html { raw } => raw.clone(),
        };
        self.out.push_str(&rendered);
    }
}

const fn is_style(kind: &MarkKind) -> bool {
    matches!(
        kind.family(),
        MarkFamily::Color | MarkFamily::Background | MarkFamily::FontFamily | MarkFamily::FontSize
    )
}

const fn priority(kind: &MarkKind) -> u8 {
    match kind {
        MarkKind::Link(_) => 0,
        MarkKind::Color(_)
        | MarkKind::Background(_)
        | MarkKind::FontFamily(_)
        | MarkKind::FontSize(_) => 1,
        MarkKind::Strong => 2,
        MarkKind::Emphasis => 3,
        MarkKind::Underline => 4,
        MarkKind::Strikethrough => 5,
        MarkKind::Highlight => 6,
        MarkKind::Spoiler => 7,
        MarkKind::Superscript => 8,
        MarkKind::Subscript => 9,
        MarkKind::Code => 10,
    }
}

/// Marks that the dialect can express, with whitespace moved outside of
/// delimiter runs and code split around atoms.
fn effective_marks(inline: &Inline, dialect: Dialect) -> Vec<Mark> {
    let text = inline.text();
    let at = |pos: usize| text[byte_index(text, pos)..].chars().next();
    let atom_positions: Vec<usize> = {
        let mut units = 0;
        text.chars()
            .filter_map(|ch| {
                let pos = units;
                units += ch.len_utf16();
                (ch == OBJECT_REPLACEMENT).then_some(pos)
            })
            .collect()
    };
    inline
        .marks()
        .iter()
        .filter(|mark| match dialect {
            Dialect::Rich => true,
            Dialect::Portable => !matches!(
                mark.kind.family(),
                MarkFamily::Highlight
                    | MarkFamily::Spoiler
                    | MarkFamily::Color
                    | MarkFamily::Background
                    | MarkFamily::FontFamily
                    | MarkFamily::FontSize
            ),
        })
        .flat_map(|mark| {
            let mut range = mark.range.clone();
            let trims = matches!(
                mark.kind.family(),
                MarkFamily::Strong
                    | MarkFamily::Emphasis
                    | MarkFamily::Strikethrough
                    | MarkFamily::Highlight
                    | MarkFamily::Spoiler
            );
            if trims {
                while range.start < range.end && at(range.start).is_some_and(char::is_whitespace) {
                    range.start += 1;
                }
                while range.start < range.end && at(range.end - 1).is_some_and(char::is_whitespace)
                {
                    range.end -= 1;
                }
            }
            if mark.kind != MarkKind::Code {
                return vec![Mark {
                    range,
                    kind: mark.kind.clone(),
                }];
            }
            let mut pieces = Vec::new();
            let mut start = range.start;
            for atom in atom_positions
                .iter()
                .filter(|pos| range.contains(pos))
                .copied()
            {
                pieces.push(start..atom);
                start = atom + 1;
            }
            pieces.push(start..range.end);
            pieces
                .into_iter()
                .map(|range| Mark {
                    range,
                    kind: MarkKind::Code,
                })
                .collect()
        })
        .filter(|mark| !mark.range.is_empty())
        .collect()
}

fn code_span(content: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for ch in content.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat(longest + 1);
    let pad = content.starts_with('`')
        || content.ends_with('`')
        || (content.starts_with(' ') && content.ends_with(' ') && !content.trim().is_empty());
    if pad {
        format!("{fence} {content} {fence}")
    } else {
        format!("{fence}{content}{fence}")
    }
}

fn link_destination(url: &str) -> String {
    let needs_brackets = url.is_empty()
        || url
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '(' | ')' | '<' | '>'));
    if needs_brackets {
        comrak::escape_commonmark_link_destination(url)
    } else {
        url.to_owned()
    }
}

fn escape_text(text: &str, mut line_start: bool, context: InlineContext) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let chars: Vec<char> = text.chars().collect();
    for (index, &ch) in chars.iter().enumerate() {
        let next = chars.get(index + 1).copied();
        if ch == '\n' {
            match context {
                InlineContext::Block => {
                    if out.ends_with('\\') && !out.ends_with("\\\\") {
                        out.push('\\');
                    }
                    out.push_str("\\\n");
                    line_start = true;
                }
                InlineContext::SingleLine => out.push(' '),
                InlineContext::TableCell => out.push_str("<br>"),
            }
            continue;
        }
        if line_start {
            if ch == ' ' || ch == '\t' {
                continue;
            }
            line_start = false;
            let rest: String = chars[index..].iter().collect();
            if needs_line_start_escape(&rest) {
                out.push('\\');
                out.push(ch);
                continue;
            }
            if let Some(digits) = ordered_marker_digits(&rest) {
                out.extend(&chars[index..index + digits]);
                out.push('\\');
                continue;
            }
        }
        let escape = match ch {
            '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '~' | '^' | '$' => true,
            '|' => context == InlineContext::TableCell || next == Some('|'),
            '=' => next == Some('='),
            '&' => looks_like_entity(&chars[index + 1..]),
            '!' => next == Some('['),
            _ => false,
        };
        if escape {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn needs_line_start_escape(rest: &str) -> bool {
    let mut chars = rest.chars();
    let first = chars.next();
    let second = chars.next();
    match first {
        Some('#' | '>') => true,
        Some('-' | '+' | '*' | '=') => {
            second.is_none_or(|ch| ch == ' ' || ch == first.unwrap_or(' '))
        }
        _ => false,
    }
}

/// Length of a leading `123.` / `123)` marker that would start a list.
fn ordered_marker_digits(rest: &str) -> Option<usize> {
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    let after = rest[digits..].chars().next();
    (digits > 0 && digits <= 9 && matches!(after, Some('.' | ')'))).then_some(digits)
}

fn looks_like_entity(rest: &[char]) -> bool {
    let body: String = rest
        .iter()
        .take(32)
        .take_while(|ch| ch.is_alphanumeric() || **ch == '#')
        .collect();
    !body.is_empty() && rest.get(body.chars().count()) == Some(&';')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::{Link, Rgba, utf16_len};

    fn md(blocks: Vec<BlockKind>, dialect: Dialect) -> String {
        Document::from(blocks.into_iter().map(Block::from).collect::<Vec<_>>()).to_markdown(dialect)
    }

    fn styled(text: &str, marks: &[(std::ops::Range<usize>, MarkKind)]) -> Inline {
        let mut inline = Inline::from(text);
        for (range, kind) in marks {
            inline.add_mark(range.clone(), kind.clone());
        }
        inline
    }

    #[test]
    fn nested_marks_close_in_order() {
        let inline = styled(
            "bold and both",
            &[(0..13, MarkKind::Strong), (9..13, MarkKind::Emphasis)],
        );
        assert_eq!(
            md(vec![BlockKind::Paragraph(inline)], Dialect::Rich),
            "**bold and *both***\n"
        );
    }

    #[test]
    fn whitespace_moves_outside_delimiters() {
        let inline = styled("a bold b", &[(1..7, MarkKind::Strong)]);
        assert_eq!(
            md(vec![BlockKind::Paragraph(inline)], Dialect::Rich),
            "a **bold** b\n"
        );
    }

    #[test]
    fn styles_become_spans_only_in_rich() {
        let inline = styled(
            "red",
            &[
                (0..3, MarkKind::Color(Rgba::rgb(255, 0, 0))),
                (0..3, MarkKind::FontSize(20)),
            ],
        );
        let blocks = vec![BlockKind::Paragraph(inline)];
        assert_eq!(
            md(blocks.clone(), Dialect::Rich),
            "<span style=\"color: #ff0000; font-size: 20px\">red</span>\n"
        );
        assert_eq!(md(blocks, Dialect::Portable), "red\n");
    }

    #[test]
    fn code_spans_pick_safe_fences() {
        let inline = styled("a`b", &[(0..3, MarkKind::Code)]);
        assert_eq!(
            md(vec![BlockKind::Paragraph(inline)], Dialect::Rich),
            "``a`b``\n"
        );
    }

    #[test]
    fn links_wrap_their_text() {
        let inline = styled(
            "see docs",
            &[(4..8, MarkKind::Link(Link::new("https://x.dev/a b")))],
        );
        assert_eq!(
            md(vec![BlockKind::Paragraph(inline)], Dialect::Rich),
            "see [docs](<https://x.dev/a b>)\n"
        );
    }

    #[test]
    fn special_chars_are_escaped() {
        let inline = Inline::from("# not *a* heading");
        assert_eq!(
            md(vec![BlockKind::Paragraph(inline)], Dialect::Rich),
            "\\# not \\*a\\* heading\n"
        );
        let inline = Inline::from("1. not a list");
        assert_eq!(
            md(vec![BlockKind::Paragraph(inline)], Dialect::Rich),
            "1\\. not a list\n"
        );
    }

    #[test]
    fn hard_breaks_use_backslash() {
        let inline = Inline::from("one\ntwo");
        assert_eq!(
            md(vec![BlockKind::Paragraph(inline)], Dialect::Rich),
            "one\\\ntwo\n"
        );
    }

    #[test]
    fn punctuation_edges_fall_back_to_html() {
        let inline = styled("(a)b", &[(0..3, MarkKind::Strong)]);
        assert_eq!(
            md(vec![BlockKind::Paragraph(inline)], Dialect::Rich),
            "<strong>(a)</strong>b\n"
        );
    }

    #[test]
    fn slug_matches_github() {
        assert_eq!(slug("Hello, World! 2"), "hello-world-2");
    }

    #[test]
    fn unicode_ranges_are_respected() {
        let inline = styled("ñandú 😀 ok", &[(9..11, MarkKind::Strong)]);
        assert_eq!(utf16_len(inline.text()), 11);
        assert_eq!(
            md(vec![BlockKind::Paragraph(inline)], Dialect::Rich),
            "ñandú 😀 **ok**\n"
        );
    }
}
