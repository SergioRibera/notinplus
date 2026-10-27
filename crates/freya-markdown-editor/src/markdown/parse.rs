use comrak::nodes::{AstNode, ListType, NodeList, NodeValue, TableAlignment};
use comrak::{Arena, Options, parse_document};

use super::html::Tag;
use super::{Extensions, ParseOptions};
use crate::document::{
    Abbreviation, Align, Atom, Block, BlockKind, CalloutKind, ColumnAlign, Document, HeadingLevel,
    Inline, Link, ListKind, MarkKind, Table, Transclusion,
};

pub(super) fn parse(markdown: &str, options: &ParseOptions) -> Document {
    let extensions = options.extensions;
    let (source, abbreviations) = if extensions.abbreviations {
        extract_abbreviations(markdown)
    } else {
        (markdown.to_owned(), Vec::new())
    };

    let arena = Arena::new();
    let root = parse_document(&arena, &source, &comrak_options(extensions));
    let mut lowering = Lowering {
        options,
        blocks: Vec::new(),
        align: Vec::new(),
        details: None,
    };
    for child in root.children() {
        lowering.block(child, 0);
    }

    let mut document = Document::from(lowering.blocks);
    document.abbreviations = abbreviations;
    document
}

fn comrak_options(extensions: Extensions) -> Options<'static> {
    let mut options = Options::default();
    let ext = &mut options.extension;
    ext.table = extensions.tables;
    ext.tasklist = extensions.task_lists;
    ext.strikethrough = extensions.strikethrough;
    ext.autolink = extensions.autolinks;
    ext.footnotes = extensions.footnotes;
    ext.alerts = extensions.alerts;
    ext.math_dollars = extensions.math;
    ext.math_code = extensions.math;
    ext.highlight = extensions.highlight;
    ext.superscript = extensions.superscript;
    ext.subscript = extensions.subscript;
    ext.spoiler = extensions.spoiler;
    ext.description_lists = extensions.description_lists;
    ext.header_attributes = extensions.heading_ids;
    if extensions.front_matter {
        ext.front_matter_delimiter = Some("---".to_owned());
    }
    #[cfg(feature = "emoji")]
    {
        ext.shortcodes = extensions.emoji_shortcodes;
    }
    options
}

/// Pull `*[term]: expansion` lines out of the source (outside code fences).
fn extract_abbreviations(markdown: &str) -> (String, Vec<Abbreviation>) {
    let mut source = String::with_capacity(markdown.len());
    let mut abbreviations = Vec::new();
    let mut fence: Option<&str> = None;
    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim();
        let fence_marker = ["```", "~~~"]
            .into_iter()
            .find(|marker| trimmed.starts_with(marker));
        match (fence, fence_marker) {
            (None, Some(marker)) => fence = Some(marker),
            (Some(open), Some(marker)) if open == marker => fence = None,
            _ => {}
        }
        if fence.is_none()
            && let Some(rest) = trimmed.strip_prefix("*[")
            && let Some((term, expansion)) = rest.split_once("]:")
            && !term.is_empty()
        {
            abbreviations.push(Abbreviation {
                term: term.to_owned(),
                expansion: expansion.trim().to_owned(),
            });
            continue;
        }
        source.push_str(line);
    }
    (source, abbreviations)
}

struct Lowering<'o> {
    options: &'o ParseOptions,
    blocks: Vec<Block>,
    align: Vec<Align>,
    /// Index of the `<details>` block still waiting for its `</details>`;
    /// the paragraphs met meanwhile become its content.
    details: Option<usize>,
}

impl Lowering<'_> {
    fn push(&mut self, kind: BlockKind) {
        if let (Some(index), BlockKind::Paragraph(paragraph)) = (self.details, &kind)
            && let Some(content) = self.blocks[index].kind.content_mut()
        {
            if !content.is_empty() {
                content.push_str("\n", &[]);
            }
            content.append(paragraph.clone());
            return;
        }
        let align = self.align.last().copied().unwrap_or_default();
        self.blocks.push(Block::from(kind).with_align(align));
    }

    /// The quote `node` as a [`Transclusion`]: its first paragraph opens
    /// with `[!kind]` (a kind with a colon) and its last names the source
    /// with `— [label](url)`.
    fn transclusion<'a>(&self, node: &'a AstNode<'a>) -> Option<Transclusion> {
        let paragraphs = node
            .children()
            .map(|child| {
                matches!(child.data().value, NodeValue::Paragraph).then(|| self.inline(child))
            })
            .collect::<Option<Vec<_>>>()?;
        let [first, middle @ .., last] = paragraphs.as_slice() else {
            return None;
        };
        let open = first.text().strip_prefix("[!")?;
        let kind = &open[..open.find(']')?];
        if !kind.contains(':') || kind.contains(char::is_whitespace) {
            return None;
        }
        let (url, label) = source_link(last)?;
        let marker = "[!]".encode_utf16().count() + kind.encode_utf16().count();
        let lead = first.slice(marker..first.len());
        let fallback = std::iter::once(&lead)
            .chain(middle)
            .map(|paragraph| super::inline_source(paragraph).trim().to_owned())
            .filter(|paragraph| !paragraph.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        Some(Transclusion {
            kind: kind.to_owned(),
            url,
            label,
            fallback,
        })
    }

    fn inline<'a>(&self, node: &'a AstNode<'a>) -> Inline {
        let mut builder = InlineBuilder::new(self.options);
        for child in node.children() {
            builder.node(child);
        }
        builder.finish()
    }

    /// Paragraph children joined with hard breaks.
    fn paragraphs<'a>(&mut self, node: &'a AstNode<'a>, depth: u8) -> Inline {
        let mut content = Inline::default();
        for child in node.children() {
            if matches!(child.data().value, NodeValue::Paragraph) {
                if !content.is_empty() {
                    content.push_str("\n", &[]);
                }
                content.append(self.inline(child));
            } else {
                self.block(child, depth);
            }
        }
        content
    }

    #[allow(clippy::too_many_lines, reason = "one arm per comrak block node")]
    fn block<'a>(&mut self, node: &'a AstNode<'a>, depth: u8) {
        let value = node.data().value.clone();
        match value {
            NodeValue::Paragraph => self.paragraph(node),
            NodeValue::Heading(heading) => {
                let level = HeadingLevel::try_from(heading.level).unwrap_or(HeadingLevel::H6);
                let anchor = node
                    .data()
                    .attrs
                    .as_ref()
                    .and_then(|attrs| attrs.id.clone());
                let content = self.inline(node);
                self.push(BlockKind::Heading {
                    level,
                    content,
                    anchor,
                });
            }
            NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
                if let Some(transclusion) = self.transclusion(node) {
                    self.push(BlockKind::Transclusion(transclusion));
                    return;
                }
                for child in node.children() {
                    if matches!(child.data().value, NodeValue::Paragraph) {
                        let content = self.inline(child);
                        self.push(BlockKind::Quote(content));
                    } else {
                        self.block(child, depth);
                    }
                }
            }
            NodeValue::Alert(alert) => {
                let index = self.blocks.len();
                self.push(BlockKind::Callout {
                    kind: callout_kind(alert.alert_type),
                    title: alert.title.clone(),
                    content: Inline::default(),
                });
                let content = self.paragraphs(node, depth);
                if let Some(slot) = self.blocks[index].kind.content_mut() {
                    *slot = content;
                }
            }
            NodeValue::List(list) => {
                for item in node.children() {
                    self.list_item(item, &list, depth);
                }
            }
            NodeValue::CodeBlock(code) => {
                let language = code
                    .info
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                let literal = code.literal.strip_suffix('\n').unwrap_or(&code.literal);
                let block = if language == "math" {
                    BlockKind::Math(Inline::from(literal))
                } else {
                    BlockKind::Code {
                        language,
                        code: Inline::from(literal),
                    }
                };
                self.push(block);
            }
            NodeValue::ThematicBreak => self.push(BlockKind::Divider),
            NodeValue::Table(table) => {
                let columns = table.alignments.iter().map(|a| column_align(*a)).collect();
                let rows = node
                    .children()
                    .map(|row| row.children().map(|cell| self.inline(cell)).collect())
                    .collect();
                self.push(BlockKind::Table(Table::from_rows(columns, rows)));
            }
            NodeValue::FootnoteDefinition(definition) => {
                let index = self.blocks.len();
                self.push(BlockKind::Footnote {
                    label: definition.name,
                    content: Inline::default(),
                });
                let content = self.paragraphs(node, depth);
                if let Some(slot) = self.blocks[index].kind.content_mut() {
                    *slot = content;
                }
            }
            NodeValue::DescriptionList => {
                for item in node.children() {
                    for part in item.children() {
                        let is_term = matches!(part.data().value, NodeValue::DescriptionTerm);
                        let index = self.blocks.len();
                        self.push(if is_term {
                            BlockKind::Term(Inline::default())
                        } else {
                            BlockKind::Definition(Inline::default())
                        });
                        let content = self.paragraphs(part, depth);
                        if let Some(slot) = self.blocks[index].kind.content_mut() {
                            *slot = content;
                        }
                    }
                }
            }
            NodeValue::FrontMatter(raw) => {
                self.push(BlockKind::FrontMatter(Inline::from(front_matter_body(
                    &raw,
                ))));
            }
            NodeValue::HtmlBlock(html) => self.html_block(&html.literal),
            NodeValue::Document
            | NodeValue::Item(_)
            | NodeValue::TaskItem(_)
            | NodeValue::DescriptionItem(_)
            | NodeValue::DescriptionTerm
            | NodeValue::DescriptionDetails
            | NodeValue::BlockDirective(_) => {
                for child in node.children() {
                    self.block(child, depth);
                }
            }
            _ => {}
        }
    }

    fn paragraph<'a>(&mut self, node: &'a AstNode<'a>) {
        let mut children = node.children();
        if let (Some(only), None) = (children.next(), children.next())
            && let NodeValue::Math(math) = &only.data().value
            && math.display_math
        {
            let tex = math.literal.trim_matches('\n').to_owned();
            self.push(BlockKind::Math(Inline::from(tex)));
            return;
        }
        let content = self.inline(node);
        let marker = content.text().trim().to_ascii_lowercase();
        if self.options.extensions.table_of_contents
            && content.marks().is_empty()
            && matches!(marker.as_str(), "[toc]" | "[[toc]]" | "[[_toc_]]")
        {
            self.push(BlockKind::TableOfContents);
            return;
        }
        self.push(BlockKind::Paragraph(content));
    }

    fn list_item<'a>(&mut self, item: &'a AstNode<'a>, list: &NodeList, depth: u8) {
        let kind = match &item.data().value {
            NodeValue::TaskItem(task) => ListKind::Task {
                checked: task.symbol.is_some_and(|symbol| symbol != ' '),
            },
            _ if list.list_type == ListType::Ordered => ListKind::Ordered {
                start: u32::try_from(list.start).unwrap_or(1),
            },
            _ => ListKind::Bullet,
        };
        let index = self.blocks.len();
        self.push(BlockKind::ListItem {
            kind,
            depth,
            content: Inline::default(),
        });
        let mut first = true;
        for child in item.children() {
            let value = child.data().value.clone();
            match value {
                NodeValue::Paragraph => {
                    let content = self.inline(child);
                    if let Some(slot) = self.blocks[index].kind.content_mut() {
                        if !first {
                            slot.push_str("\n", &[]);
                        }
                        slot.append(content);
                    }
                }
                NodeValue::List(nested) => {
                    for nested_item in child.children() {
                        self.list_item(nested_item, &nested, depth.saturating_add(1));
                    }
                }
                _ => self.block(child, depth),
            }
            first = false;
        }
    }

    fn html_block(&mut self, literal: &str) {
        let trimmed = literal.trim();
        let lower = trimmed.to_ascii_lowercase();
        if let Some(note) = trimmed
            .strip_prefix("<!--")
            .and_then(|rest| rest.strip_suffix("-->"))
            .filter(|note| !note.contains("-->"))
        {
            self.push(BlockKind::Comment(Inline::from(note.trim())));
            return;
        }
        if lower == "</details>" && self.details.is_some() {
            self.details = None;
            return;
        }
        if let Some(details) = Details::parse(trimmed) {
            let closed = details.closed;
            self.push(BlockKind::Details {
                summary: details.summary,
                open: details.open,
                content: Inline::from(details.body.as_str()),
            });
            if !closed {
                self.details = Some(self.blocks.len() - 1);
            }
            return;
        }
        if lower == "</div>" && !self.align.is_empty() {
            self.align.pop();
            return;
        }
        if let Some(align) = lower
            .strip_prefix("<div align=")
            .and_then(|rest| rest.strip_suffix('>'))
            .and_then(|value| Align::from_css(value.trim_matches(['"', '\''])))
        {
            self.align.push(align);
            return;
        }
        self.push(BlockKind::Html(Inline::from(trimmed)));
    }
}

/// A `<details>` element opened by one HTML block. Blank lines split the
/// element into several blocks, so its body and closing tag may come later.
struct Details {
    summary: String,
    open: bool,
    /// Text after the summary within this same block.
    body: String,
    /// Whether `</details>` was in this same block.
    closed: bool,
}

impl Details {
    fn parse(html: &str) -> Option<Self> {
        // ASCII lowercasing keeps byte offsets, so `lower` indexes `html`.
        let lower = html.to_ascii_lowercase();
        let tag_end = lower.find('>')?;
        let attributes = lower.get(..tag_end)?.strip_prefix("<details")?;
        if !(attributes.is_empty() || attributes.starts_with(char::is_whitespace)) {
            return None;
        }
        let open = attributes
            .split_whitespace()
            .any(|attribute| attribute == "open" || attribute.starts_with("open="));
        let mut rest = tag_end + 1;
        let mut summary = String::new();
        if let Some(start) = lower[rest..].find("<summary") {
            let start = rest + start;
            let inner = start + lower[start..].find('>')? + 1;
            let end = inner + lower[inner..].find("</summary>")?;
            summary = super::html::unescape(html[inner..end].trim());
            rest = end + "</summary>".len();
        }
        let (body, closed) = lower[rest..]
            .find("</details>")
            .map_or((&html[rest..], false), |end| {
                (&html[rest..rest + end], true)
            });
        Some(Self {
            summary,
            open,
            body: body.trim().to_owned(),
            closed,
        })
    }
}

/// URL and label of the link on a `— [label](url)` line, whether the link
/// stayed a link or was claimed as an embed.
fn source_link(line: &Inline) -> Option<(String, String)> {
    if !line.text().trim_start().starts_with('—') {
        return None;
    }
    line.marks()
        .iter()
        .find_map(|mark| match &mark.kind {
            MarkKind::Link(target) => Some((
                target.url.clone(),
                line.slice(mark.range.clone()).plain_text(),
            )),
            _ => None,
        })
        .or_else(|| {
            line.atoms().iter().find_map(|atom| match atom {
                Atom::Embed { url, label, .. } => Some((url.clone(), label.clone())),
                _ => None,
            })
        })
}

const fn callout_kind(alert: comrak::nodes::AlertType) -> CalloutKind {
    use comrak::nodes::AlertType;
    match alert {
        AlertType::Note => CalloutKind::Note,
        AlertType::Tip => CalloutKind::Tip,
        AlertType::Important => CalloutKind::Important,
        AlertType::Warning => CalloutKind::Warning,
        AlertType::Caution => CalloutKind::Caution,
    }
}

const fn column_align(align: TableAlignment) -> ColumnAlign {
    match align {
        TableAlignment::None => ColumnAlign::None,
        TableAlignment::Left => ColumnAlign::Left,
        TableAlignment::Center => ColumnAlign::Center,
        TableAlignment::Right => ColumnAlign::Right,
    }
}

fn front_matter_body(raw: &str) -> String {
    let lines: Vec<&str> = raw.trim().lines().collect();
    match lines.as_slice() {
        [first, body @ .., last] if first.trim() == "---" && last.trim() == "---" => {
            body.join("\n")
        }
        _ => raw.trim().to_owned(),
    }
}

struct InlineBuilder<'o> {
    options: &'o ParseOptions,
    inline: Inline,
    active: Vec<MarkKind>,
    html: Vec<(String, Vec<MarkKind>)>,
}

impl<'o> InlineBuilder<'o> {
    fn new(options: &'o ParseOptions) -> Self {
        Self {
            options,
            inline: Inline::default(),
            active: Vec::new(),
            html: Vec::new(),
        }
    }

    fn finish(self) -> Inline {
        self.inline
    }

    fn marks(&self) -> Vec<MarkKind> {
        self.active
            .iter()
            .chain(self.html.iter().flat_map(|(_, marks)| marks))
            .cloned()
            .collect()
    }

    fn text(&mut self, text: &str) {
        let marks = self.marks();
        self.inline.push_str(text, &marks);
    }

    fn atom(&mut self, atom: Atom) {
        let marks = self.marks();
        self.inline.push_atom(atom, &marks);
    }

    fn with_mark<'a>(&mut self, mark: MarkKind, node: &'a AstNode<'a>) {
        self.active.push(mark);
        for child in node.children() {
            self.node(child);
        }
        self.active.pop();
    }

    fn node<'a>(&mut self, node: &'a AstNode<'a>) {
        let value = node.data().value.clone();
        match value {
            NodeValue::Text(text) => self.text(&text),
            NodeValue::SoftBreak => self.text(" "),
            NodeValue::LineBreak => self.text("\n"),
            NodeValue::Code(code) => {
                self.active.push(MarkKind::Code);
                self.text(&code.literal);
                self.active.pop();
            }
            NodeValue::Emph => self.with_mark(MarkKind::Emphasis, node),
            NodeValue::Strong => self.with_mark(MarkKind::Strong, node),
            NodeValue::Strikethrough => self.with_mark(MarkKind::Strikethrough, node),
            NodeValue::Highlight => self.with_mark(MarkKind::Highlight, node),
            NodeValue::Superscript => self.with_mark(MarkKind::Superscript, node),
            NodeValue::Subscript => self.with_mark(MarkKind::Subscript, node),
            NodeValue::Underline | NodeValue::Insert => {
                self.with_mark(MarkKind::Underline, node);
            }
            NodeValue::SpoileredText => self.with_mark(MarkKind::Spoiler, node),
            NodeValue::Link(link) => {
                let title = (!link.title.is_empty()).then(|| link.title.clone());
                if self
                    .options
                    .embed
                    .as_ref()
                    .is_some_and(|embed| embed(&link.url))
                {
                    self.atom(Atom::Embed {
                        url: link.url.clone(),
                        label: plain_text(node),
                        title,
                    });
                } else {
                    self.with_mark(
                        MarkKind::Link(Link {
                            url: link.url.clone(),
                            title,
                        }),
                        node,
                    );
                }
            }
            NodeValue::Image(link) => self.atom(Atom::Image {
                url: link.url.clone(),
                alt: plain_text(node),
                title: (!link.title.is_empty()).then(|| link.title.clone()),
            }),
            NodeValue::FootnoteReference(reference) => self.atom(Atom::FootnoteRef {
                label: reference.name.clone(),
            }),
            NodeValue::Math(math) => self.atom(Atom::Math { tex: math.literal }),
            NodeValue::HtmlInline(raw) => self.html(&raw),
            NodeValue::Raw(raw) => self.text(&raw),
            NodeValue::EscapedTag(tag) => self.text(tag),
            #[cfg(feature = "emoji")]
            NodeValue::ShortCode(code) => self.text(&code.emoji),
            _ => {
                for child in node.children() {
                    self.node(child);
                }
            }
        }
    }

    fn html(&mut self, raw: &str) {
        match Tag::classify(raw) {
            Tag::Open { name, marks } => self.html.push((name, marks)),
            Tag::Close { name } => {
                if let Some(index) = self.html.iter().rposition(|(open, _)| *open == name) {
                    self.html.remove(index);
                }
            }
            Tag::LineBreak => self.text("\n"),
            Tag::Unknown => self.atom(Atom::Html {
                raw: raw.to_owned(),
            }),
        }
    }
}

fn plain_text<'a>(node: &'a AstNode<'a>) -> String {
    node.descendants()
        .filter_map(|descendant| match &descendant.data().value {
            NodeValue::Text(text) => Some(text.to_string()),
            NodeValue::Code(code) => Some(code.literal.clone()),
            NodeValue::SoftBreak | NodeValue::LineBreak => Some(" ".to_owned()),
            _ => None,
        })
        .collect()
}
