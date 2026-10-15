//! Freya chapter reader.
//!
//! Pure builders — the host embeds [`chapter_view`] inside a reactive
//! scope and feeds a loaded [`ChapterPlan`]. The reader itself does not
//! own state; chapter navigation and selection live on
//! [`crate::state::BibleViewerState`].

use freya::prelude::*;

use super::chapter::{Block, ChapterPlan, ParagraphSpan};
use super::theme;
use super::typography;

/// Full chapter view — scrollable column of section headers +
/// paragraph groups.
///
/// `font_size` and `line_height` override the typography defaults so
/// the shell can honour the user's plugin config without re-building
/// this crate.
pub fn chapter_view(
    plan: &ChapterPlan,
    font_size: f32,
    line_height: f32,
) -> impl IntoElement {
    let drop_cap_size = font_size * 4.5;

    rect()
        .width(Size::fill())
        .height(Size::fill())
        .background(parse_rgb(theme::BG))
        .padding((24., 28.))
        .child(
            ScrollView::new()
                .child(
                    rect()
                        .width(Size::fill())
                        .padding((16., 0.))
                        .spacing(14.)
                        .child(spacer(28.))
                        .child(drop_cap_block(plan, font_size, line_height, drop_cap_size))
                        .child(trailing_blocks(plan, font_size, line_height)),
                ),
        )
}

/// First visual block of the chapter — large chapter number sitting
/// to the left of the opening paragraph. The chapter number is placed
/// absolutely so the paragraph wraps full-width underneath after the
/// first two lines; close enough to the YouVersion-style drop cap
/// without needing a true float.
fn drop_cap_block(
    plan: &ChapterPlan,
    font_size: f32,
    line_height: f32,
    drop_cap_size: f32,
) -> impl IntoElement {
    let first_paragraph = plan
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Paragraph(spans) => Some(spans.clone()),
            Block::Header(_) => None,
        })
        .unwrap_or_default();

    rect()
        .width(Size::fill())
        .padding((0., 0., 10., 0.))
        .child(
            rect()
                .position(Position::new_absolute().left(0.).top(0.))
                .width(Size::px(drop_cap_size * 0.75))
                .child(
                    label()
                        .font_family(typography::FAMILY)
                        .font_size(drop_cap_size)
                        .color(parse_rgb(theme::DROP_CAP))
                        .text(format!("{}", plan.chapter)),
                ),
        )
        .child(rect().padding((0., 0., 0., drop_cap_size * 0.65)).child(
            verse_paragraph(&first_paragraph, font_size, line_height),
        ))
}

/// Every block after the first paragraph — includes section headers
/// and later paragraph groups. The first paragraph is handled by
/// [`drop_cap_block`] above so we skip it here to avoid duplicating
/// text.
fn trailing_blocks(
    plan: &ChapterPlan,
    font_size: f32,
    line_height: f32,
) -> impl IntoElement {
    let mut root = rect().width(Size::fill()).spacing(14.);
    let mut skipped_first = false;
    for block in &plan.blocks {
        match block {
            Block::Paragraph(spans) => {
                if !skipped_first {
                    skipped_first = true;
                    continue;
                }
                root = root.child(verse_paragraph(spans, font_size, line_height));
            }
            Block::Header(text) => {
                root = root.child(section_header(text));
            }
        }
    }
    root
}

/// A single paragraph with inline superscript verse numbers.
fn verse_paragraph(
    spans: &[ParagraphSpan],
    font_size: f32,
    line_height: f32,
) -> impl IntoElement {
    let mut p = paragraph()
        .width(Size::fill())
        .line_height(line_height)
        .color(parse_rgb(theme::TEXT_PRIMARY))
        .font_family(typography::FAMILY)
        .font_size(font_size);

    for span in spans {
        p = p.span(
            Span::new(format!(" {} ", span.verse))
                .font_size(font_size * 0.6)
                .color(parse_rgb(theme::VERSE_NUM)),
        );
        p = p.span(
            Span::new(format!("{} ", span.text))
                .font_size(font_size)
                .color(parse_rgb(theme::TEXT_PRIMARY)),
        );
    }
    p
}

fn section_header(text: &str) -> impl IntoElement {
    rect()
        .width(Size::fill())
        .padding((12., 0., 4., 0.))
        .child(
            label()
                .font_family(typography::FAMILY)
                .font_size(typography::SECTION_HEADER_PX)
                .color(parse_rgb(theme::HEADER))
                .text(text.to_string()),
        )
}

fn spacer(height: f32) -> impl IntoElement {
    rect().width(Size::fill()).height(Size::px(height))
}

/// Parse an `"rgb(r, g, b)"` string into a Freya `Color`. The theme
/// module keeps the strings for parity with the inline markup used in
/// other Freya components in this project.
fn parse_rgb(s: &str) -> Color {
    let trimmed = s.trim_start_matches("rgb(").trim_end_matches(')');
    let parts: Vec<u8> = trimmed
        .split(',')
        .filter_map(|t| t.trim().parse::<u8>().ok())
        .collect();
    if parts.len() == 3 {
        Color::from_rgb(parts[0], parts[1], parts[2])
    } else {
        Color::WHITE
    }
}
