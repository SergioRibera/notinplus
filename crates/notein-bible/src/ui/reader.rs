//! Freya chapter reader.
//!
//! Pure builders — the host embeds [`chapter_view`] inside a reactive
//! scope and feeds a loaded [`ChapterPlan`]. The reader itself does not
//! own state; chapter navigation and selection live on
//! [`crate::state::BibleViewerState`].

use std::collections::HashMap;

use freya::prelude::*;

use crate::state::{BibleBookmark, u32_to_rgba};

use super::chapter::{Block, ChapterPlan, ParagraphSpan};
use super::theme;
use super::typography;

/// Styling info derived from the active highlights, keyed by verse
/// number inside a single chapter. Precomputed so the paragraph builder
/// does one hash lookup per span instead of a linear scan.
#[derive(Clone, Debug, Default)]
pub struct VerseStyle {
    /// Non-default text colour when the verse is highlighted. The
    /// absence of a stored colour on the bookmark falls back to the
    /// theme highlight yellow.
    pub tint: Option<Color>,
    /// True when the verse carries a non-empty note. Drives the inline
    /// note icon rendered right after the verse text.
    pub has_note: bool,
}

/// Build the per-verse style map from a highlights slice. The caller
/// passes `state.highlights_on(book, chapter)` for the chapter being
/// rendered.
#[must_use]
pub fn style_map(highlights: &[&BibleBookmark]) -> HashMap<u16, VerseStyle> {
    let mut out: HashMap<u16, VerseStyle> = HashMap::new();
    for h in highlights {
        let tint = h.color.map(|c| rgba_to_freya(u32_to_rgba(c))).or_else(|| {
            Some(parse_rgb(theme::HIGHLIGHT_YELLOW))
        });
        let has_note = !h.note.is_empty();
        for v in h.start_verse..=h.end_verse {
            let entry = out.entry(v).or_default();
            // Later highlights win on tint — matches a layered-notes
            // mental model (the newest note shows its colour).
            entry.tint = tint;
            entry.has_note |= has_note;
        }
    }
    out
}

fn rgba_to_freya(rgba: [u8; 4]) -> Color {
    Color::from_rgb(rgba[0], rgba[1], rgba[2])
}

/// Full chapter view — scrollable column of section headers +
/// paragraph groups.
///
/// `font_size` and `line_height` override the typography defaults so
/// the shell can honour the user's plugin config without re-building
/// this crate.
pub fn chapter_view(
    plan: &ChapterPlan,
    highlights: &[&BibleBookmark],
    font_size: f32,
    line_height: f32,
) -> impl IntoElement {
    let drop_cap_size = font_size * 4.5;
    let styles = style_map(highlights);

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
                        .child(drop_cap_block(plan, &styles, font_size, line_height, drop_cap_size))
                        .child(trailing_blocks(plan, &styles, font_size, line_height)),
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
    styles: &HashMap<u16, VerseStyle>,
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
            verse_paragraph(&first_paragraph, styles, font_size, line_height),
        ))
}

/// Every block after the first paragraph — includes section headers
/// and later paragraph groups. The first paragraph is handled by
/// [`drop_cap_block`] above so we skip it here to avoid duplicating
/// text.
fn trailing_blocks(
    plan: &ChapterPlan,
    styles: &HashMap<u16, VerseStyle>,
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
                root = root.child(verse_paragraph(spans, styles, font_size, line_height));
            }
            Block::Header(text) => {
                root = root.child(section_header(text));
            }
        }
    }
    root
}

/// A single paragraph with inline superscript verse numbers.
/// Highlighted verses get their text repainted in the stored tint and
/// gain an inline note icon when the bookmark has a non-empty body.
fn verse_paragraph(
    spans: &[ParagraphSpan],
    styles: &HashMap<u16, VerseStyle>,
    font_size: f32,
    line_height: f32,
) -> impl IntoElement {
    let mut p = paragraph()
        .width(Size::fill())
        .line_height(line_height)
        .color(parse_rgb(theme::TEXT_PRIMARY))
        .font_family(typography::FAMILY)
        .font_size(font_size);

    let default_text_color = parse_rgb(theme::TEXT_PRIMARY);

    for span in spans {
        let style = styles.get(&span.verse);
        let text_color = style
            .and_then(|s| s.tint)
            .unwrap_or(default_text_color);

        p = p.span(
            Span::new(format!(" {} ", span.verse))
                .font_size(font_size * 0.6)
                .color(parse_rgb(theme::VERSE_NUM)),
        );
        p = p.span(
            Span::new(format!("{} ", span.text))
                .font_size(font_size)
                .color(text_color),
        );
        if style.map(|s| s.has_note).unwrap_or(false) {
            p = p.child(note_icon(font_size));
        }
    }
    p
}

fn note_icon(font_size: f32) -> impl IntoElement {
    let size = font_size * 0.75;
    rect()
        .width(Size::px(size + 6.0))
        .height(Size::px(size + 2.0))
        .center()
        .child(
            SvgViewer::new(freya::icons::lucide::file_text())
                .color(parse_rgb(theme::VERSE_NUM))
                .width(Size::px(size))
                .height(Size::px(size)),
        )
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
