//! Markdown drawn straight onto a Skia canvas, without the UI: for
//! pictures made off screen (covers, exported pages).
//!
//! It reads the same [`Document`] the editor shows and follows its theme,
//! but only approximates the editor's layout: blocks stack with the theme's
//! spacing, lists and quotes indent, atoms read as their plain text and
//! images are left out.

use std::ops::Range;

use freya::prelude::Color;
use freya_engine::prelude::{
    Canvas, Color as SkColor, FontCollection, FontStyle, Paint, Paragraph, ParagraphBuilder,
    ParagraphStyle, Rect, Slant, TextDecoration, TextStyle, Weight, Width,
};

use crate::document::{
    Atom, BlockKind, Document, Inline, ListKind, MarkKind, OBJECT_REPLACEMENT, byte_index,
};
use crate::theme::EditorTheme;

/// Indent of one list level or of a quote's text, at zoom 1.
const GUTTER: f32 = 24.0;
/// Width of a quote's bar, at zoom 1.
const BAR: f32 = 3.0;
/// Inner space of a code block, at zoom 1.
const CODE_PAD: f32 = 8.0;

/// Markdown laid out for a width, ready to paint as often as needed.
pub struct MarkdownPicture {
    items: Vec<Item>,
    height: f32,
}

impl std::fmt::Debug for MarkdownPicture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MarkdownPicture")
            .field("items", &self.items.len())
            .field("height", &self.height)
            .finish()
    }
}

/// One thing to draw, in picture coordinates.
enum Item {
    Text {
        paragraph: Paragraph,
        at: (f32, f32),
    },
    Fill {
        rect: Rect,
        color: Color,
    },
}

/// Text style of a run, before marks.
#[derive(Clone)]
struct Base {
    size: f32,
    color: Color,
    bold: bool,
    italic: bool,
    mono: bool,
}

/// Lays blocks out top to bottom.
struct Layout<'a> {
    theme: &'a EditorTheme,
    fonts: &'a FontCollection,
    width: f32,
    items: Vec<Item>,
    y: f32,
}

impl MarkdownPicture {
    /// Lay `markdown` out `width` wide in `theme`, with text from `fonts`.
    #[must_use]
    #[cfg_attr(feature = "sismo", sismo::function("markdown::picture_layout"))]
    pub fn layout(markdown: &str, theme: &EditorTheme, width: f32, fonts: &FontCollection) -> Self {
        let mut layout = Layout {
            theme,
            fonts,
            width: width.max(1.0),
            items: Vec::new(),
            y: 0.0,
        };
        layout.document(&Document::parse(markdown), 0.0);
        let height = (layout.y - theme.block_spacing).max(0.0);
        Self {
            items: layout.items,
            height,
        }
    }

    /// Height of the laid out text.
    #[must_use]
    pub const fn height(&self) -> f32 {
        self.height
    }

    /// Draw it with its top left corner at `origin`.
    #[cfg_attr(feature = "sismo", sismo::function("markdown::picture_paint"))]
    pub fn paint(&self, canvas: &Canvas, origin: (f32, f32)) {
        for item in &self.items {
            match item {
                Item::Text { paragraph, at } => {
                    paragraph.paint(canvas, (origin.0 + at.0, origin.1 + at.1));
                }
                Item::Fill { rect, color } => {
                    let mut paint = Paint::default();
                    paint.set_anti_alias(true);
                    paint.set_color(SkColor::from(*color));
                    canvas.draw_rect(rect.with_offset(origin), &paint);
                }
            }
        }
    }
}

impl Layout<'_> {
    const fn px(&self, length: f32) -> f32 {
        self.theme.px(length)
    }

    const fn body(&self) -> Base {
        Base {
            size: self.theme.body_size,
            color: self.theme.colors.text,
            bold: false,
            italic: false,
            mono: false,
        }
    }

    #[allow(clippy::too_many_lines, reason = "one arm per block kind")]
    fn document(&mut self, document: &Document, indent: f32) {
        let mut ordinal = 0_u32;
        for block in document.blocks() {
            if !matches!(
                block.kind,
                BlockKind::ListItem {
                    kind: ListKind::Ordered { .. },
                    ..
                }
            ) {
                ordinal = 0;
            }
            match &block.kind {
                BlockKind::Paragraph(content) => {
                    if !content.is_empty() {
                        self.text(content, &self.body(), indent, "");
                    }
                }
                BlockKind::Heading { level, content, .. } => {
                    let index = usize::from(level.get().saturating_sub(1)).min(5);
                    let base = Base {
                        size: self.theme.heading_sizes[index],
                        bold: true,
                        ..self.body()
                    };
                    self.text(content, &base, indent, "");
                }
                BlockKind::Quote(content) => {
                    let base = Base {
                        color: self.theme.colors.muted,
                        ..self.body()
                    };
                    self.barred(self.theme.colors.quote_bar, indent, |layout, inner| {
                        layout.text(content, &base, inner, "");
                    });
                }
                BlockKind::Callout {
                    kind,
                    title,
                    content,
                } => {
                    let color = self.theme.colors.callout(*kind);
                    self.barred(color, indent, |layout, inner| {
                        if let Some(title) = title {
                            let base = Base {
                                bold: true,
                                color,
                                ..layout.body()
                            };
                            layout.text(&Inline::from(title.as_str()), &base, inner, "");
                        }
                        layout.text(content, &layout.body(), inner, "");
                    });
                }
                BlockKind::ListItem {
                    kind,
                    depth,
                    content,
                } => {
                    let marker = match kind {
                        ListKind::Bullet => "•  ".to_owned(),
                        ListKind::Ordered { start } => {
                            ordinal = if ordinal == 0 { *start } else { ordinal + 1 };
                            format!("{ordinal}.  ")
                        }
                        ListKind::Task { checked: true } => "☑  ".to_owned(),
                        ListKind::Task { checked: false } => "☐  ".to_owned(),
                    };
                    let inner = self.px(GUTTER).mul_add(f32::from(*depth), indent);
                    self.text(content, &self.body(), inner, &marker);
                }
                BlockKind::Code { code, .. } | BlockKind::Math(code) => {
                    let base = Base {
                        mono: true,
                        color: self.theme.colors.code_text,
                        ..self.body()
                    };
                    self.boxed(code, &base, indent);
                }
                BlockKind::Divider => {
                    let thickness = self.px(1.0);
                    let y = self.y + self.px(6.0);
                    self.items.push(Item::Fill {
                        rect: Rect::from_xywh(indent, y, self.width - indent, thickness),
                        color: self.theme.colors.border,
                    });
                    self.y = y + thickness + self.px(6.0) + self.theme.block_spacing;
                }
                BlockKind::Table(table) => {
                    for (index, row) in table.rows().iter().enumerate() {
                        let mut line = Inline::default();
                        for (column, cell) in row.iter().enumerate() {
                            if column > 0 {
                                line.append(Inline::from("  │  "));
                            }
                            line.append(cell.clone());
                        }
                        let base = Base {
                            bold: index == 0,
                            ..self.body()
                        };
                        self.text(&line, &base, indent, "");
                    }
                }
                BlockKind::Footnote { label, content } => {
                    let base = Base {
                        size: self.theme.body_size * 0.85,
                        color: self.theme.colors.muted,
                        ..self.body()
                    };
                    self.text(content, &base, indent, &format!("[{label}]  "));
                }
                BlockKind::Term(content) => {
                    let base = Base {
                        bold: true,
                        ..self.body()
                    };
                    self.text(content, &base, indent, "");
                }
                BlockKind::Definition(content) => {
                    self.text(content, &self.body(), indent + self.px(GUTTER), "");
                }
                BlockKind::Details {
                    summary,
                    open,
                    content,
                } => {
                    let base = Base {
                        bold: true,
                        ..self.body()
                    };
                    let mark = if *open { "▾  " } else { "▸  " };
                    self.text(&Inline::from(summary.as_str()), &base, indent, mark);
                    if *open {
                        self.text(content, &self.body(), indent + self.px(GUTTER), "");
                    }
                }
                BlockKind::Html(content) => {
                    self.text(content, &self.body(), indent, "");
                }
                BlockKind::Transclusion(transclusion) => {
                    let color = transclusion
                        .color
                        .map_or(self.theme.colors.text, Color::from);
                    let quoted = Document::parse(&transclusion.fallback);
                    self.barred(self.theme.colors.quote_bar, indent, |layout, inner| {
                        let mut theme = layout.theme.clone();
                        theme.colors.text = color;
                        let mut nested = Layout {
                            theme: &theme,
                            fonts: layout.fonts,
                            width: layout.width,
                            items: Vec::new(),
                            y: layout.y,
                        };
                        nested.document(&quoted, inner);
                        layout.items.append(&mut nested.items);
                        layout.y = nested.y;
                        let base = Base {
                            size: layout.theme.body_size * 0.8,
                            color: layout.theme.colors.muted,
                            ..layout.body()
                        };
                        let source = format!("— {}", transclusion.label);
                        layout.text(&Inline::from(source.as_str()), &base, inner, "");
                    });
                }
                BlockKind::FrontMatter(_) | BlockKind::Comment(_) | BlockKind::TableOfContents => {}
            }
        }
    }

    /// Blocks drawn by `inner` behind a vertical bar of `color`.
    fn barred(&mut self, color: Color, indent: f32, inner: impl FnOnce(&mut Self, f32)) {
        let top = self.y;
        let text = self.px(GUTTER).mul_add(0.5, indent + self.px(BAR));
        inner(self, text);
        let bottom = (self.y - self.theme.block_spacing).max(top);
        self.items.push(Item::Fill {
            rect: Rect::from_xywh(indent, top, self.px(BAR), bottom - top),
            color,
        });
    }

    /// `content` on a code background.
    fn boxed(&mut self, content: &Inline, base: &Base, indent: f32) {
        let pad = self.px(CODE_PAD);
        let top = self.y;
        let at = self.items.len();
        self.y += pad;
        self.text(content, base, indent + pad, "");
        let bottom = self.y - self.theme.block_spacing + pad;
        self.items.insert(
            at,
            Item::Fill {
                rect: Rect::from_xywh(indent, top, self.width - indent, bottom - top),
                color: self.theme.colors.code_background,
            },
        );
        self.y = bottom + self.theme.block_spacing;
    }

    /// One paragraph of `content` after `marker`, from `indent` to the
    /// right edge.
    fn text(&mut self, content: &Inline, base: &Base, indent: f32, marker: &str) {
        let mut style = ParagraphStyle::new();
        style.set_text_style(&self.style(base, &[]));
        let mut builder = ParagraphBuilder::new(&style, self.fonts.clone());
        if !marker.is_empty() {
            builder.push_style(&self.style(base, &[]));
            builder.add_text(marker);
            builder.pop();
        }
        for (range, marks) in runs(content) {
            let text = run_text(content, range);
            if text.is_empty() {
                continue;
            }
            builder.push_style(&self.style(base, &marks));
            builder.add_text(&text);
            builder.pop();
        }
        let mut paragraph = builder.build();
        paragraph.layout((self.width - indent).max(1.0));
        let height = paragraph.height();
        self.items.push(Item::Text {
            paragraph,
            at: (indent, self.y),
        });
        self.y += height + self.theme.block_spacing;
    }

    /// Skia style of `base` with `marks` on top.
    fn style(&self, base: &Base, marks: &[&MarkKind]) -> TextStyle {
        let colors = &self.theme.colors;
        let mut size = base.size;
        let mut color = base.color;
        let (mut bold, mut italic, mut mono) = (base.bold, base.italic, base.mono);
        let mut decoration = TextDecoration::NO_DECORATION;
        let mut background = None;
        let mut family = None;
        let mut shift = 0.0;
        for mark in marks {
            match mark {
                MarkKind::Strong => bold = true,
                MarkKind::Emphasis => italic = true,
                MarkKind::Underline | MarkKind::Abbreviation(_) => {
                    decoration |= TextDecoration::UNDERLINE;
                }
                MarkKind::Strikethrough => decoration |= TextDecoration::LINE_THROUGH,
                MarkKind::Code => {
                    mono = true;
                    color = colors.code_text;
                    background = Some(colors.code_background);
                }
                MarkKind::Highlight => background = Some(colors.highlight),
                MarkKind::Spoiler => {
                    color = colors.spoiler;
                    background = Some(colors.spoiler);
                }
                MarkKind::Superscript => {
                    size *= 0.7;
                    shift = -base.size * 0.35;
                }
                MarkKind::Subscript => {
                    size *= 0.7;
                    shift = base.size * 0.2;
                }
                MarkKind::Link(_) => {
                    color = colors.accent;
                    decoration |= TextDecoration::UNDERLINE;
                }
                MarkKind::Color(rgba) => color = Color::from(*rgba),
                MarkKind::Background(rgba) => background = Some(Color::from(*rgba)),
                MarkKind::FontFamily(name) => family = Some(name.as_str()),
                MarkKind::FontSize(points) => size = self.px(f32::from(*points)),
            }
        }
        let mut style = TextStyle::new();
        style.set_color(SkColor::from(color));
        style.set_font_size(size);
        style.set_height(self.theme.line_height);
        style.set_height_override(true);
        let fonts: Vec<&str> = if mono {
            vec![self.theme.monospace.as_ref()]
        } else {
            family
                .into_iter()
                .chain(self.theme.fonts.iter().map(AsRef::as_ref))
                .collect()
        };
        style.set_font_families(&fonts);
        style.set_font_style(FontStyle::new(
            if bold { Weight::BOLD } else { Weight::NORMAL },
            Width::NORMAL,
            if italic {
                Slant::Italic
            } else {
                Slant::Upright
            },
        ));
        if decoration != TextDecoration::NO_DECORATION {
            style.set_decoration_type(decoration);
            style.set_decoration_color(SkColor::from(color));
        }
        if let Some(background) = background {
            let mut paint = Paint::default();
            paint.set_color(SkColor::from(background));
            style.set_background_paint(&paint);
        }
        if shift != 0.0 {
            style.set_baseline_shift(shift);
        }
        style
    }
}

/// UTF-16 ranges of `content` with the marks that cover each, in order.
fn runs(content: &Inline) -> Vec<(Range<usize>, Vec<&MarkKind>)> {
    let len = content.len();
    let mut cuts = vec![0, len];
    for mark in content.marks() {
        cuts.push(mark.range.start.min(len));
        cuts.push(mark.range.end.min(len));
    }
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .map(|pair| {
            let range = pair[0]..pair[1];
            let marks = content
                .marks()
                .iter()
                .filter(|mark| mark.range.start <= range.start && range.end <= mark.range.end)
                .map(|mark| &mark.kind)
                .collect();
            (range, marks)
        })
        .collect()
}

/// Text of `range` in `content`, atoms read as their plain text.
fn run_text(content: &Inline, range: Range<usize>) -> String {
    let text = content.text();
    let start = byte_index(text, range.start);
    let end = byte_index(text, range.end);
    let atoms_before = text[..start].matches(OBJECT_REPLACEMENT).count();
    let mut atoms = content.atoms().iter().skip(atoms_before);
    let mut out = String::with_capacity(end - start);
    for ch in text[start..end].chars() {
        if ch == OBJECT_REPLACEMENT {
            match atoms.next() {
                Some(Atom::Image { .. } | Atom::Html { .. }) | None => {}
                Some(atom) => out.push_str(atom.plain_text()),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_split_where_marks_start_and_end() {
        let document = Document::parse("a **b** c");
        let BlockKind::Paragraph(content) = &document.blocks()[0].kind else {
            panic!("{:?}", document.blocks());
        };
        let runs: Vec<_> = runs(content)
            .into_iter()
            .map(|(range, marks)| (run_text(content, range), marks.len()))
            .collect();
        assert_eq!(
            runs,
            [
                ("a ".to_owned(), 0),
                ("b".to_owned(), 1),
                (" c".to_owned(), 0)
            ]
        );
    }
}
