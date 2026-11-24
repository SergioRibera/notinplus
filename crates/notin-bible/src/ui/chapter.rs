//! Chapter view.
//!
//! Freya rsx tree landing here owns:
//! - absolute-positioned drop-cap chapter number
//! - flowing `paragraph` with per-verse spans + superscript verse numbers
//! - headings between verse groups chunked by `paragraph_break`
//! - poetry lines (`line_start`, `line_breaks`) as line breaks inside a
//!   paragraph, so a stanza stays one paragraph

use crate::db::ChapterRow;

/// Logical sections of the chapter view, assembled from a loaded
/// `ChapterRow`.
///
/// The rsx assembly consumes this on the UI thread; keeping the plan a plain
/// data value lets the rendering layer stay thin + testable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChapterPlan {
    pub chapter: u16,
    pub book_name: String,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    Header(Heading),
    Paragraph(Vec<ParagraphSpan>),
}

/// A heading between paragraphs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    pub text: String,
    /// 1 a major division (`LIBRO I`), 2 a section, 3 a psalm title or a
    /// passage range (`(Lc. 6.20-23)`).
    pub level: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParagraphSpan {
    pub verse: u16,
    /// The verse as shown: the spaces where its poetry lines break are
    /// line breaks, so it measures the same as the stored text.
    pub text: String,
    /// The next verse of the paragraph starts on a line of its own.
    pub ends_line: bool,
}

#[must_use]
#[cfg_attr(feature = "sismo", sismo::function("bible::plan_chapter"))]
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
            let header = &data.headers[header_cursor];
            blocks.push(Block::Header(Heading {
                text: header.text.clone(),
                level: header.level,
            }));
            header_cursor += 1;
        }

        if v.paragraph_break && !current.is_empty() {
            blocks.push(Block::Paragraph(std::mem::take(&mut current)));
        }
        if let Some(previous) = current.last_mut() {
            previous.ends_line = v.line_start;
        }
        current.push(ParagraphSpan {
            verse: v.verse,
            text: shown_text(&v.text, &v.line_breaks),
            ends_line: false,
        });
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

/// `text` with the spaces at `breaks` turned into line breaks (a stanza
/// inside a verse is one too: a paragraph cannot split a verse).
fn shown_text(text: &str, breaks: &[crate::import::layout::LineBreak]) -> String {
    if breaks.is_empty() {
        return text.to_owned();
    }
    let mut shown = text.to_owned();
    for line in breaks {
        // Only a space that is still there: a stale layout never cuts a
        // word or a character.
        if shown.as_bytes().get(line.at) == Some(&b' ') {
            shown.replace_range(line.at..=line.at, "\n");
        }
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{HeaderRow, VerseRow};
    use crate::import::layout::{Break, LineBreak, lay_out};

    fn verse(n: u16, pieces: &[&str], paragraph: bool, line: bool) -> VerseRow {
        let laid = lay_out(pieces.iter().copied());
        VerseRow {
            verse: n,
            text: laid.text,
            paragraph_break: paragraph,
            line_start: line,
            line_breaks: laid.breaks,
        }
    }

    #[test]
    fn poetry_stays_one_paragraph_per_stanza() {
        let data = ChapterRow {
            bible_id: "spa_rv1960".into(),
            book_id: "psa".into(),
            book_name: "Salmos".into(),
            chapter: 23,
            verses: vec![
                verse(1, &["Jehová es mi pastor; nada me faltará."], true, false),
                verse(
                    2,
                    &[
                        "En lugares de delicados pastos me hará descansar;\nJunto a aguas de reposo me pastoreará.",
                    ],
                    false,
                    true,
                ),
                verse(
                    3,
                    &["Confortará mi alma;\nMe guiará por sendas"],
                    false,
                    true,
                ),
                verse(4, &["Aunque ande en valle"], true, false),
            ],
            headers: vec![
                HeaderRow {
                    before_verse: 1,
                    text: "Jehová es mi pastor".into(),
                    level: 2,
                },
                HeaderRow {
                    before_verse: 1,
                    text: "Salmo de David.".into(),
                    level: 3,
                },
            ],
        };
        let plan = plan_chapter(&data);
        let levels: Vec<_> = plan
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Header(h) => Some(h.level),
                Block::Paragraph(_) => None,
            })
            .collect();
        assert_eq!(levels, [2, 3]);
        let Block::Paragraph(stanza) = &plan.blocks[2] else {
            panic!("a stanza");
        };
        assert_eq!(stanza.len(), 3, "verses 1 to 3, then the stanza break");
        assert!(stanza[0].ends_line && stanza[1].ends_line && !stanza[2].ends_line);
        assert_eq!(
            stanza[1].text,
            "En lugares de delicados pastos me hará descansar;\nJunto a aguas de reposo me pastoreará."
        );
        assert_eq!(
            stanza[1].text.len(),
            data.verses[1].text.len(),
            "same length as stored"
        );
        assert!(matches!(&plan.blocks[3], Block::Paragraph(next) if next[0].verse == 4));
    }

    #[test]
    fn a_stale_layout_never_cuts_a_word() {
        let breaks = [LineBreak {
            at: 2,
            kind: Break::Line,
        }];
        assert_eq!(shown_text("abcdef", &breaks), "abcdef");
        assert_eq!(shown_text("ab cdef", &breaks), "ab\ncdef");
    }
}
