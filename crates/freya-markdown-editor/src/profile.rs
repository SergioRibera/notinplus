//! Profiler helpers that name work after the block kind it is for. Without
//! the `sismo` feature they compile to nothing.

#[cfg(feature = "sismo")]
use crate::document::{Atom, Document};
use crate::document::BlockKind;

/// Runs `build` inside a span named after `kind`.
#[cfg(feature = "sismo")]
pub(crate) fn kind_span<T>(kind: &BlockKind, build: impl FnOnce() -> T) -> T {
    macro_rules! span {
        ($name:literal) => {{
            sismo::scope!($name);
            build()
        }};
    }
    match kind {
        BlockKind::Paragraph(_) => span!("markdown::block::paragraph"),
        BlockKind::Heading { .. } => span!("markdown::block::heading"),
        BlockKind::Quote(_) => span!("markdown::block::quote"),
        BlockKind::Callout { .. } => span!("markdown::block::callout"),
        BlockKind::ListItem { .. } => span!("markdown::block::list_item"),
        BlockKind::Code { .. } => span!("markdown::block::code"),
        BlockKind::Math(_) => span!("markdown::block::math"),
        BlockKind::Divider => span!("markdown::block::divider"),
        BlockKind::Table(_) => span!("markdown::block::table"),
        BlockKind::Footnote { .. } => span!("markdown::block::footnote"),
        BlockKind::Term(_) => span!("markdown::block::term"),
        BlockKind::Definition(_) => span!("markdown::block::definition"),
        BlockKind::FrontMatter(_) => span!("markdown::block::front_matter"),
        BlockKind::TableOfContents => span!("markdown::block::toc"),
        BlockKind::Html(_) => span!("markdown::block::html"),
        BlockKind::Comment(_) => span!("markdown::block::comment"),
        BlockKind::Details { .. } => span!("markdown::block::details"),
        BlockKind::Transclusion(_) => span!("markdown::block::transclusion"),
    }
}

#[cfg(not(feature = "sismo"))]
pub(crate) fn kind_span<T>(_kind: &BlockKind, build: impl FnOnce() -> T) -> T {
    build()
}

/// Records how many blocks of each kind, formulas, images and characters
/// `document` holds.
#[cfg(feature = "sismo")]
pub(crate) fn count_document(document: &Document) {
    sismo::scope!("markdown::count_document");
    let mut kinds = [0usize; 17];
    let (mut formulas, mut images, mut embeds, mut chars) = (0usize, 0usize, 0usize, 0usize);
    let mut atoms = |content: &crate::document::Inline| {
        chars += content.len();
        for atom in content.atoms() {
            match atom {
                Atom::Math { .. } => formulas += 1,
                Atom::Image { .. } => images += 1,
                Atom::Embed { .. } => embeds += 1,
                Atom::FootnoteRef { .. } | Atom::Html { .. } => {}
            }
        }
    };
    for block in document.blocks() {
        let slot = match &block.kind {
            BlockKind::Paragraph(_) => 0,
            BlockKind::Heading { .. } => 1,
            BlockKind::Quote(_) => 2,
            BlockKind::Callout { .. } => 3,
            BlockKind::ListItem { .. } => 4,
            BlockKind::Code { .. } => 5,
            BlockKind::Math(_) => 6,
            BlockKind::Divider => 7,
            BlockKind::Table(_) => 8,
            BlockKind::Footnote { .. } => 9,
            BlockKind::Term(_) | BlockKind::Definition(_) => 10,
            BlockKind::FrontMatter(_) => 11,
            BlockKind::TableOfContents => 12,
            BlockKind::Html(_) => 13,
            BlockKind::Comment(_) => 14,
            BlockKind::Details { .. } => 15,
            BlockKind::Transclusion(_) => 16,
        };
        kinds[slot] += 1;
        if let BlockKind::Table(table) = &block.kind {
            table.rows().iter().flatten().for_each(&mut atoms);
        } else if let Some(content) = block.kind.content() {
            atoms(content);
        }
    }
    sismo::counter!("markdown.blocks.paragraph", kinds[0]);
    sismo::counter!("markdown.blocks.heading", kinds[1]);
    sismo::counter!("markdown.blocks.quote", kinds[2]);
    sismo::counter!("markdown.blocks.callout", kinds[3]);
    sismo::counter!("markdown.blocks.list_item", kinds[4]);
    sismo::counter!("markdown.blocks.code", kinds[5]);
    sismo::counter!("markdown.blocks.math", kinds[6]);
    sismo::counter!("markdown.blocks.divider", kinds[7]);
    sismo::counter!("markdown.blocks.table", kinds[8]);
    sismo::counter!("markdown.blocks.footnote", kinds[9]);
    sismo::counter!("markdown.blocks.definition", kinds[10]);
    sismo::counter!("markdown.blocks.front_matter", kinds[11]);
    sismo::counter!("markdown.blocks.toc", kinds[12]);
    sismo::counter!("markdown.blocks.html", kinds[13]);
    sismo::counter!("markdown.blocks.comment", kinds[14]);
    sismo::counter!("markdown.blocks.details", kinds[15]);
    sismo::counter!("markdown.blocks.transclusion", kinds[16]);
    sismo::counter!("markdown.formulas", formulas + kinds[6]);
    sismo::counter!("markdown.images", images);
    sismo::counter!("markdown.embeds", embeds);
    sismo::counter!("markdown.chars", chars);
}
