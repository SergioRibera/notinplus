//! Chapter view.
//!
//! Freya rsx tree landing here owns:
//! - absolute-positioned drop-cap chapter number
//! - flowing `paragraph` with per-verse spans + superscript verse numbers
//! - section headers between verse groups chunked by `paragraph_break`

use crate::db::ChapterRow;

/// Logical sections of the chapter view, assembled from a loaded
/// `ChapterRow`. The rsx assembly consumes this on the UI thread;
/// keeping the plan a plain data value lets the rendering layer stay
/// thin + testable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChapterPlan {
    pub chapter: u16,
    pub book_name: String,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Header(String),
    Paragraph(Vec<ParagraphSpan>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParagraphSpan {
    pub verse: u16,
    pub text: String,
}

#[must_use]
pub fn plan_chapter(data: &ChapterRow) -> ChapterPlan {
    let mut blocks: Vec<Block> = Vec::new();
    let mut header_cursor = 0usize;
    let mut current: Vec<ParagraphSpan> = Vec::new();

    for v in &data.verses {
        // Flush pending headers that belong before this verse.
        while header_cursor < data.headers.len()
            && data.headers[header_cursor].before_verse <= v.verse
        {
            if !current.is_empty() {
                blocks.push(Block::Paragraph(std::mem::take(&mut current)));
            }
            blocks.push(Block::Header(data.headers[header_cursor].text.clone()));
            header_cursor += 1;
        }

        if v.paragraph_break && !current.is_empty() {
            blocks.push(Block::Paragraph(std::mem::take(&mut current)));
        }
        current.push(ParagraphSpan { verse: v.verse, text: v.text.clone() });
    }

    if !current.is_empty() {
        blocks.push(Block::Paragraph(current));
    }

    ChapterPlan {
        chapter: data.chapter,
        book_name: data.book_name.clone(),
        blocks,
    }
}
