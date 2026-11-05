//! Rough heights of blocks that are not laid out yet.

use crate::document::{Block, BlockKind};
use crate::theme::EditorTheme;

/// Width assumed before the editor has been measured.
const UNMEASURED_WIDTH: f32 = 600.0;

/// Average glyph width as a fraction of the font size.
const GLYPH_WIDTH: f32 = 0.5;

/// Height `block` most likely takes at `width`, from its kind and how
/// much text it holds. Rough on purpose: a strut of about the right size
/// keeps the scrollbar steady while blocks mount, and nothing depends on
/// it being exact.
#[allow(
    clippy::cast_precision_loss,
    reason = "line and row counts are far below f32 precision"
)]
pub(crate) fn height_hint(
    block: &Block,
    theme: &EditorTheme,
    width: f32,
    toc_entries: usize,
) -> f32 {
    let width = if width > 0.0 { width } else { UNMEASURED_WIDTH };
    let line = theme.body_size * theme.line_height;
    let spacing = theme.block_spacing;
    let lines = |text: &str, font_size: f32| {
        let per_line = (width / (font_size * GLYPH_WIDTH)).max(8.0);
        let wrapped = (text.len() as f32 / per_line).ceil().max(1.0);
        wrapped + text.matches('\n').count() as f32
    };
    let body = |text: &str| lines(text, theme.body_size) * line + spacing;
    match &block.kind {
        BlockKind::Paragraph(content)
        | BlockKind::Quote(content)
        | BlockKind::Term(content)
        | BlockKind::Definition(content)
        | BlockKind::FrontMatter(content)
        | BlockKind::Html(content)
        | BlockKind::Comment(content)
        | BlockKind::Footnote { content, .. }
        | BlockKind::ListItem { content, .. } => body(content.text()),
        BlockKind::Heading { level, content, .. } => {
            let size = theme.heading_size(*level);
            lines(content.text(), size) * size * theme.line_height + 2.0 * spacing
        }
        BlockKind::Callout { content, .. } => body(content.text()) + line + theme.px(24.0),
        BlockKind::Code { code, .. } => {
            code.text().lines().count().max(1) as f32 * line + theme.px(24.0)
        }
        BlockKind::Math(_) => 3.0 * line,
        BlockKind::Divider => theme.px(21.0),
        BlockKind::Table(table) => (table.rows().len() + 1) as f32 * (line + theme.px(16.0)),
        BlockKind::TableOfContents => (toc_entries + 1) as f32 * line + theme.px(16.0),
        BlockKind::Details { content, open, .. } => {
            line + spacing + if *open { body(content.text()) } else { 0.0 }
        }
        BlockKind::Transclusion(transclusion) => body(&transclusion.fallback) + 2.0 * line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;

    fn hints(markdown: &str) -> Vec<f32> {
        let document = Document::parse(markdown);
        let theme = EditorTheme::default();
        document
            .blocks()
            .iter()
            .map(|block| height_hint(block, &theme, 600.0, 0))
            .collect()
    }

    #[test]
    fn longer_text_takes_more_lines() {
        let short = hints("word")[0];
        let long = hints(&"word ".repeat(200))[0];
        assert!(long > short * 3.0, "{long} vs {short}");
    }

    #[test]
    fn code_grows_with_its_lines() {
        let one = hints("```\na\n```")[0];
        let ten = hints(&format!("```\n{}```", "a\n".repeat(10)))[0];
        assert!(ten > one * 3.0, "{ten} vs {one}");
    }

    #[test]
    fn every_kind_has_a_height() {
        let all = "# H\n\ntext\n\n> q\n\n- item\n\n```\ncode\n```\n\n---\n\n| a |\n|---|\n| b |\n";
        assert!(hints(all).iter().all(|hint| *hint > 0.0));
    }
}
