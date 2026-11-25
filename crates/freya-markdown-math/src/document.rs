//! A markdown note as a PDF document, typeset by Typst.
//!
//! The note is parsed with [`freya_markdown_editor`]'s document model and
//! written out as Typst markup ([`markdown_typst`]), which Typst lays out
//! in pages and exports as a real PDF: selectable text, embedded fonts,
//! formulas typeset like on screen. Fonts are embedded in the crate, so
//! it works offline and the same on every platform.
//!
//! Text runs travel as Typst string literals (`#"…"`), never as markup,
//! so nothing a note contains is read as Typst syntax.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::LazyLock;

use freya_markdown_editor::document::{
    Align, Atom, BlockKind, CalloutKind, ColumnAlign, Document, HeadingLevel, Inline, ListKind,
    MarkKind, OBJECT_REPLACEMENT,
};
use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, Source};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_layout::PagedDocument;

use crate::typeset::{FONTS, SCOPE};

/// Text fonts of documents, besides the math ones formulas share.
const TEXT_FONTS: [&[u8]; 4] = [
    include_bytes!("../assets/fonts/LibertinusSerif-Bold.otf"),
    include_bytes!("../assets/fonts/LibertinusSerif-Italic.otf"),
    include_bytes!("../assets/fonts/LibertinusSerif-BoldItalic.otf"),
    include_bytes!("../assets/fonts/DejaVuSansMono.ttf"),
];

/// Page size of the document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Paper {
    #[default]
    A4,
    Letter,
}

impl Paper {
    const fn typst(self) -> &'static str {
        match self {
            Self::A4 => "a4",
            Self::Letter => "us-letter",
        }
    }
}

/// How the document is laid out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PdfSetup {
    pub paper: Paper,
    /// Written into the PDF's metadata.
    pub title: Option<String>,
    /// Language of the text (`es`, `en`), for hyphenation and quotes.
    pub lang: Option<String>,
}

/// Why a note could not become a PDF.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentError {
    /// Typst refused the document; the text is its first error.
    Typeset(String),
    /// The laid out document could not be written as PDF.
    Export(String),
}

impl std::fmt::Display for DocumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Typeset(message) | Self::Export(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for DocumentError {}

/// `markdown` as a PDF. `resource` gives the bytes of an image the note
/// links to (`assets/1a2b.png`); an image it cannot give shows its
/// description instead.
///
/// # Errors
/// [`DocumentError`] when Typst cannot lay the note out or write it.
#[cfg_attr(feature = "sismo", sismo::function("math::markdown_pdf"))]
pub fn markdown_pdf(
    markdown: &str,
    setup: &PdfSetup,
    resource: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Result<Vec<u8>, DocumentError> {
    let (source, images) = markdown_typst(markdown, setup, resource);
    let world = DocumentWorld {
        main: Source::detached(source),
        images: images.into_iter().map(Bytes::new).collect(),
    };
    let document = typst::compile::<PagedDocument>(&world).output;
    typst::comemo::evict(30);
    let document = document.map_err(|errors| {
        DocumentError::Typeset(
            errors
                .first()
                .map_or_else(String::new, |error| error.message.to_string()),
        )
    })?;
    typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default()).map_err(|errors| {
        DocumentError::Export(
            errors
                .first()
                .map_or_else(String::new, |error| error.message.to_string()),
        )
    })
}

/// `markdown` as Typst source, and the images it embeds: image `n` is the
/// file `/img/n`.
#[must_use]
pub fn markdown_typst(
    markdown: &str,
    setup: &PdfSetup,
    resource: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> (String, Vec<Vec<u8>>) {
    let mut writer = Writer {
        resource,
        images: Vec::new(),
        notes: HashMap::new(),
    };
    let mut out = String::from(PREAMBLE);
    if let Some(title) = &setup.title {
        let _ = writeln!(out, "#set document(title: {})", literal(title));
    }
    if let Some(lang) = setup.lang.as_deref().filter(|lang| is_lang(lang)) {
        let _ = writeln!(out, "#set text(lang: \"{lang}\")");
    }
    let _ = writeln!(out, "#set page(paper: \"{}\")\n", setup.paper.typst());
    out.push_str(&writer.document(&Document::parse(markdown)));
    (out, writer.images)
}

/// A two or three letter language code: the only thing written raw.
fn is_lang(lang: &str) -> bool {
    (2..=3).contains(&lang.len()) && lang.chars().all(|c| c.is_ascii_lowercase())
}

const PREAMBLE: &str = r##"#import "/mitex/mod.typ": mitex-scope
#set page(margin: (x: 2.2cm, y: 2.4cm), numbering: "1")
#set text(font: "Libertinus Serif", size: 11pt)
#set par(leading: 0.7em, spacing: 1.1em)
#set heading(numbering: none)
#show heading: set block(above: 1.4em, below: 0.8em)
#show raw: set text(font: "DejaVu Sans Mono", size: 0.88em)
#show math.equation: set text(font: "New Computer Modern Math")
#show link: set text(fill: rgb("#1d4ed8"))
#let fit-image(path) = layout(size => {
  let img = image(path)
  if measure(img).width > size.width { image(path, width: 100%) } else { img }
})
#let callout(color, title, body) = block(
  width: 100%, inset: (x: 10pt, y: 8pt), radius: 3pt,
  fill: color.lighten(90%), stroke: (left: 2.5pt + color),
  { if title != none { text(fill: color, weight: "bold", title); parbreak() }; body },
)
#let task(done) = box(baseline: 12%, rect(
  width: 0.75em, height: 0.75em, radius: 1.5pt, stroke: 0.6pt + luma(90),
  fill: if done { luma(90) } else { none },
))
#set terms(separator: h(0.45em), hanging-indent: 1.3em)
#let code-block(body) = block(width: 100%, inset: 8pt, radius: 3pt, fill: luma(245), body)
"##;

/// Writes one document; images and footnotes are shared by everything it
/// holds (a quoted document included).
struct Writer<'a> {
    resource: &'a dyn Fn(&str) -> Option<Vec<u8>>,
    images: Vec<Vec<u8>>,
    /// Footnote bodies by label, as Typst content.
    notes: HashMap<String, String>,
}

impl Writer<'_> {
    #[allow(clippy::too_many_lines, reason = "one arm per block kind")]
    fn document(&mut self, document: &Document) -> String {
        // Footnotes are written where they are cited.
        for block in document.blocks() {
            if let BlockKind::Footnote { label, content } = &block.kind {
                let body = self.inline(content);
                self.notes.insert(label.clone(), body);
            }
        }
        let mut out = String::new();
        let mut numbers: Vec<u32> = Vec::new();
        let mut in_list = false;
        for block in document.blocks() {
            if let BlockKind::ListItem {
                kind,
                depth,
                content,
            } = &block.kind
            {
                let depth = usize::from(*depth);
                numbers.truncate(depth + 1);
                numbers.resize(depth + 1, 0);
                let marker = match kind {
                    ListKind::Bullet => "-".to_owned(),
                    // A term list: the box stands where a bullet would.
                    ListKind::Task { checked } => format!("/ #task({checked}):"),
                    ListKind::Ordered { start } => {
                        let next = if numbers[depth] == 0 {
                            *start
                        } else {
                            numbers[depth] + 1
                        };
                        numbers[depth] = next;
                        format!("{next}.")
                    }
                };
                let _ = writeln!(
                    out,
                    "{}{marker} {}",
                    "  ".repeat(depth),
                    self.inline(content)
                );
                in_list = true;
                continue;
            }
            if in_list {
                out.push('\n');
                in_list = false;
                numbers.clear();
            }
            let body = match &block.kind {
                BlockKind::Paragraph(content) => self.inline(content),
                BlockKind::Heading { level, content, .. } => format!(
                    "#heading(level: {})[{}]",
                    heading_level(*level),
                    self.inline(content)
                ),
                BlockKind::Quote(content) => {
                    format!("#quote(block: true)[{}]", self.inline(content))
                }
                BlockKind::Callout {
                    kind,
                    title,
                    content,
                } => format!(
                    "#callout(rgb(\"{}\"), {}, [{}])",
                    callout_color(*kind),
                    title.as_deref().map_or_else(|| "none".to_owned(), literal),
                    self.inline(content)
                ),
                BlockKind::Code { language, code } => format!(
                    "#code-block(raw(block: true, lang: {}, {}))",
                    if language.is_empty() {
                        "none".to_owned()
                    } else {
                        literal(language)
                    },
                    literal(code.text())
                ),
                BlockKind::Math(tex) => math(&tex.plain_text(), true),
                BlockKind::Divider => "#line(length: 100%, stroke: 0.5pt + luma(170))".to_owned(),
                BlockKind::Table(table) => self.table(table),
                BlockKind::Term(content) => format!("#strong[{}]", self.inline(content)),
                BlockKind::Definition(content) => {
                    format!("#pad(left: 1.5em)[{}]", self.inline(content))
                }
                BlockKind::Details {
                    summary, content, ..
                } => format!("#strong({})\n\n{}", literal(summary), self.inline(content)),
                BlockKind::TableOfContents => "#outline()".to_owned(),
                BlockKind::Transclusion(quoted) => {
                    let inner = self.document(&Document::parse(&quoted.fallback));
                    format!(
                        "#quote(block: true, attribution: {})[{inner}]",
                        literal(&quoted.label)
                    )
                }
                BlockKind::ListItem { .. }
                | BlockKind::Footnote { .. }
                | BlockKind::FrontMatter(_)
                | BlockKind::Comment(_)
                | BlockKind::Html(_) => continue,
            };
            let body = match block.align {
                Align::Center => format!("#align(center)[{body}]"),
                Align::Right => format!("#align(right)[{body}]"),
                _ => body,
            };
            out.push_str(&body);
            out.push_str("\n\n");
        }
        out
    }

    fn table(&mut self, table: &freya_markdown_editor::document::Table) -> String {
        let columns = table.columns();
        let align = columns
            .iter()
            .map(|align| match align {
                ColumnAlign::Center => "center",
                ColumnAlign::Right => "right",
                ColumnAlign::None | ColumnAlign::Left => "left",
            })
            .collect::<Vec<_>>()
            .join(", ");
        let mut out = format!(
            "#table(columns: {}, align: ({align},), stroke: 0.5pt + luma(170), inset: 6pt,\n",
            columns.len().max(1)
        );
        for (index, row) in table.rows().iter().enumerate() {
            let cells: Vec<String> = row
                .iter()
                .map(|cell| format!("[{}]", self.inline(cell)))
                .collect();
            if index == 0 {
                let _ = writeln!(out, "  table.header({}),", cells.join(", "));
            } else {
                let _ = writeln!(out, "  {},", cells.join(", "));
            }
        }
        out.push(')');
        out
    }

    /// `inline` as Typst markup: each run a string literal wrapped in the
    /// functions of its marks.
    fn inline(&mut self, inline: &Inline) -> String {
        let chars: Vec<char> = inline.text().chars().collect();
        let mut cuts = vec![0, chars.len()];
        for mark in inline.marks() {
            cuts.push(mark.range.start.min(chars.len()));
            cuts.push(mark.range.end.min(chars.len()));
        }
        for (at, ch) in chars.iter().enumerate() {
            if matches!(*ch, OBJECT_REPLACEMENT | '\n') {
                cuts.extend([at, at + 1]);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();

        let mut atoms = inline.atoms().iter();
        let mut out = String::new();
        for window in cuts.windows(2) {
            let (start, end) = (window[0], window[1]);
            if start >= end {
                continue;
            }
            let marks: Vec<&MarkKind> = inline
                .marks()
                .iter()
                .filter(|mark| mark.range.start <= start && end <= mark.range.end)
                .map(|mark| &mark.kind)
                .collect();
            let piece: String = chars[start..end].iter().collect();
            let code = marks.iter().any(|kind| matches!(kind, MarkKind::Code));
            let expr = match piece.as_str() {
                "\n" => {
                    out.push_str("#linebreak()");
                    continue;
                }
                "\u{FFFC}" => match atoms.next() {
                    Some(atom) => self.atom(atom),
                    None => continue,
                },
                text if code => format!("raw({})", literal(text)),
                text => literal(text),
            };
            out.push('#');
            out.push_str(&wrap(expr, &marks));
        }
        out
    }

    fn atom(&mut self, atom: &Atom) -> String {
        match atom {
            Atom::Image { url, alt, .. } => match (self.resource)(url) {
                Some(bytes) => {
                    self.images.push(bytes);
                    format!("fit-image(\"/img/{}\")", self.images.len() - 1)
                }
                None => format!("emph({})", literal(alt)),
            },
            Atom::Embed { url, label, .. } => {
                if url.starts_with("http://") || url.starts_with("https://") {
                    format!("link({}, {})", literal(url), literal(label))
                } else {
                    literal(label)
                }
            }
            Atom::FootnoteRef { label } => self.notes.get(label).map_or_else(
                || literal(&format!("[{label}]")),
                |body| format!("footnote[{body}]"),
            ),
            Atom::Math { tex } => math(tex, false)
                .strip_prefix('#')
                .map(str::to_owned)
                .unwrap_or_default(),
            Atom::Html { .. } => "[]".to_owned(),
        }
    }
}

/// `expr` inside the functions its marks call, innermost first.
fn wrap(mut expr: String, marks: &[&MarkKind]) -> String {
    for kind in marks {
        expr = match kind {
            MarkKind::Superscript => format!("super({expr})"),
            MarkKind::Subscript => format!("sub({expr})"),
            _ => continue,
        };
    }
    for kind in marks {
        expr = match kind {
            MarkKind::Strong => format!("strong({expr})"),
            MarkKind::Emphasis => format!("emph({expr})"),
            MarkKind::Underline => format!("underline({expr})"),
            MarkKind::Strikethrough => format!("strike({expr})"),
            MarkKind::Highlight => format!("highlight({expr})"),
            MarkKind::Background(color) => {
                format!(
                    "highlight(fill: {}, {expr})",
                    rgb(color.r, color.g, color.b)
                )
            }
            MarkKind::Color(color) => {
                format!("text(fill: {}, {expr})", rgb(color.r, color.g, color.b))
            }
            MarkKind::FontSize(size) => format!("text(size: {size}pt, {expr})"),
            _ => continue,
        };
    }
    if let Some(MarkKind::Link(link)) = marks.iter().find(|kind| matches!(kind, MarkKind::Link(_)))
    {
        expr = format!("link({}, {expr})", literal(&link.url));
    }
    expr
}

/// A formula: LaTeX converted by `MiTeX`, or its source when it does not
/// convert.
fn math(tex: &str, block: bool) -> String {
    mitex::convert_math(tex.trim(), None).map_or_else(
        |_| format!("#raw(block: {block}, {})", literal(tex)),
        |math| {
            format!(
                "#math.equation(block: {block}, eval(\"$\" + {} + \"$\", scope: mitex-scope))",
                literal(&math)
            )
        },
    )
}

/// `text` as a Typst string literal.
fn literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' | OBJECT_REPLACEMENT => {}
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn rgb(r: u8, g: u8, b: u8) -> String {
    format!("rgb(\"#{r:02x}{g:02x}{b:02x}\")")
}

const fn heading_level(level: HeadingLevel) -> u8 {
    level as u8
}

const fn callout_color(kind: CalloutKind) -> &'static str {
    match kind {
        CalloutKind::Note => "#2563eb",
        CalloutKind::Tip => "#16a34a",
        CalloutKind::Important => "#7c3aed",
        CalloutKind::Warning => "#d97706",
        CalloutKind::Caution => "#dc2626",
    }
}

/// Library and fonts of documents, built on the first export.
struct Base {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
}

static BASE: LazyLock<Base> = LazyLock::new(|| {
    let fonts: Vec<Font> = FONTS
        .into_iter()
        .chain(TEXT_FONTS)
        .flat_map(|data| Font::iter(Bytes::new(data)))
        .collect();
    Base {
        library: LazyHash::new(Library::builder().build()),
        book: LazyHash::new(FontBook::from_fonts(&fonts)),
        fonts,
    }
});

/// One note as a Typst document: its source, `MiTeX`'s and its images.
struct DocumentWorld {
    main: Source,
    images: Vec<Bytes>,
}

impl DocumentWorld {
    fn path(id: FileId) -> String {
        id.get().vpath().get_without_slash().to_owned()
    }
}

impl World for DocumentWorld {
    fn library(&self) -> &LazyHash<Library> {
        &BASE.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &BASE.book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            return Ok(self.main.clone());
        }
        let path = Self::path(id);
        SCOPE
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, text)| Source::new(id, (*text).to_owned()))
            .ok_or_else(|| FileError::NotFound(path.into()))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        let path = Self::path(id);
        path.strip_prefix("img/")
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| self.images.get(index).cloned())
            .ok_or_else(|| FileError::NotFound(path.into()))
    }

    fn font(&self, index: usize) -> Option<Font> {
        BASE.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "# Título #1\n\nTexto con **negrita**, *itálica*, `código` y [enlace](https://example.com).\nUn \"#comando\" y $x^2$ en línea.\n\n- uno\n- dos\n  - tres\n\n1. primero\n2. segundo\n\n- [x] hecho\n- [ ] pendiente\n\n> [!WARNING] Ojo\n> Cuidado aquí.\n\n```rust\nfn main() {}\n```\n\n$$\n\\frac{a}{b}\n$$\n\n| A | B |\n|---|--:|\n| 1 | 2 |\n\nNota al pie[^1].\n\n[^1]: El texto de la nota.\n\n![una imagen](assets/a.png)\n";

    #[test]
    fn note_text_never_reads_as_typst() {
        let (source, _) = markdown_typst(NOTE, &PdfSetup::default(), &|_| None);
        assert!(source.contains("Un \\\"#comando\\\" y \""), "{source}");
        assert!(source.contains("#heading(level: 1)[#\"Título #1\"]"));
        assert!(source.contains("#strong(\"negrita\")"));
        assert!(source.contains("#raw(\"código\")"));
        assert!(source.contains("/ #task(true): "));
        assert!(source.contains("footnote["));
        assert!(
            source.contains("#emph(\"una imagen\")"),
            "an image it cannot read"
        );
    }

    #[test]
    fn a_note_becomes_a_pdf() {
        // A 1x1 PNG.
        let png: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99,
            0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let setup = PdfSetup {
            title: Some("Prueba".into()),
            lang: Some("es".into()),
            ..PdfSetup::default()
        };
        let pdf = markdown_pdf(NOTE, &setup, &|url| {
            (url == "assets/a.png").then(|| png.to_vec())
        })
        .unwrap_or_else(|err| panic!("{err}"));
        assert!(pdf.starts_with(b"%PDF-"));
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("note.pdf"), &pdf);
    }

    /// Every `.md` under `NOTES_DIR` must typeset:
    /// `NOTES_DIR=docs cargo test -p freya-markdown-math --features pdf -- --ignored every_note`.
    #[test]
    #[ignore = "reads a directory of notes"]
    fn every_note_typesets() {
        let dir = std::env::var("NOTES_DIR").expect("NOTES_DIR");
        let mut failed = Vec::new();
        let mut count = 0;
        for entry in std::fs::read_dir(dir).expect("dir").flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "md") {
                continue;
            }
            count += 1;
            let markdown = std::fs::read_to_string(&path).expect("read");
            if let Err(err) = markdown_pdf(&markdown, &PdfSetup::default(), &|_| None) {
                failed.push(format!("{}: {err}", path.display()));
            }
        }
        assert!(
            failed.is_empty(),
            "{count} notes, failed:\n{}",
            failed.join("\n")
        );
    }
}
