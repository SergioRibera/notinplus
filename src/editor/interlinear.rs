//! An [`Interlinear`] laid out as a quote shows it: one column per word,
//! the word over how it reads and what it means, columns flowing in lines
//! like text. Right to left text puts its first word on the right.
//!
//! The columns are measured on the first frame, in one row at their own
//! widths, and packed into lines here: Freya wrapping would squeeze the
//! last column of a line, and cannot fill lines from the right.

use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use notin_plugin::bookmark::SourceRef;
use notin_plugin::kind::{GlossedWord, Interlinear};
use notin_plugin::services::Placement;

/// Space between columns and between lines, at zoom 1.
const COLUMN_GAP: f32 = 16.0;
const LINE_GAP: f32 = 12.0;
/// Size of the word against the body text.
const WORD_SCALE: f32 = 1.3;
/// Width of the row the columns are measured in: more than any line of
/// words, so each column takes its own width.
const MEASURING_WIDTH: f32 = 100_000.0;

#[derive(Clone, PartialEq)]
pub(crate) struct InterlinearView {
    pub(crate) interlinear: Interlinear,
    /// Theme of the quote, with its text colours.
    pub(crate) theme: EditorTheme,
}

impl Component for InterlinearView {
    #[sismo::function("editor::interlinear")]
    fn render(&self) -> impl IntoElement {
        let words = &self.interlinear.words;
        let rtl = self.interlinear.right_to_left;
        let mut widths = use_state(Vec::<f32>::new);
        let mut room = use_state(|| 0.0_f32);
        if widths.peek().len() != words.len() {
            widths.set(vec![0.0; words.len()]);
        }
        let gap = self.theme.px(COLUMN_GAP);
        let line_gap = self.theme.px(LINE_GAP);
        // Read, not peeked: the measures landing must lay the lines out.
        let room_now = *room.read();
        let measured: Option<Vec<f32>> = {
            let widths = widths.read();
            (room_now > 0.0 && widths.iter().all(|w| *w > 0.0)).then(|| widths.clone())
        };
        let column = |index: usize, word: &GlossedWord, width: Option<f32>| {
            WordColumn {
                word: word.clone(),
                theme: self.theme.clone(),
                font: self.interlinear.font_family.clone(),
                rtl,
            }
            .into_element_with_size(index, widths, width)
        };
        let packed = |sizes: Vec<f32>| {
            let lines = pack(&sizes, room_now, gap);
            rect()
                .width(Size::fill())
                .spacing(line_gap)
                .children(lines.into_iter().map(|line| {
                    let order: Vec<usize> = if rtl {
                        line.into_iter().rev().collect()
                    } else {
                        line
                    };
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .main_align(if rtl {
                            Alignment::End
                        } else {
                            Alignment::Start
                        })
                        .spacing(gap)
                        .children(
                            order
                                .into_iter()
                                .map(|i| column(i, &words[i], Some(sizes[i].min(room_now)))),
                        )
                        .into_element()
                }))
                .into_element()
        };
        // First frame: every column at its own width in one row, to measure
        // them; the next frame lays the lines out.
        let measuring = || {
            rect()
                .width(Size::fill())
                .overflow(Overflow::Clip)
                .child(
                    // Wide enough that no column is squeezed while measured.
                    rect()
                        .width(Size::px(MEASURING_WIDTH))
                        .horizontal()
                        .spacing(gap)
                        .children(
                            words
                                .iter()
                                .enumerate()
                                .map(|(i, word)| column(i, word, None)),
                        ),
                )
                .into_element()
        };
        let body = measured.map_or_else(measuring, packed);
        rect()
            .width(Size::fill())
            .on_sized(move |event: Event<SizedEventData>| {
                room.set_if_modified(event.area.width());
            })
            .child(body)
    }
}

/// Lines of word indices, in reading order, that fit `room` with `gap`
/// between columns. A word wider than the room gets a line of its own.
fn pack(widths: &[f32], room: f32, gap: f32) -> Vec<Vec<usize>> {
    let mut lines: Vec<Vec<usize>> = Vec::new();
    let mut used = 0.0;
    for (index, width) in widths.iter().enumerate() {
        match lines.last_mut() {
            Some(line) if used + gap + width <= room => {
                line.push(index);
                used += gap + width;
            }
            _ => {
                lines.push(vec![index]);
                used = *width;
            }
        }
    }
    lines
}

/// One word over its reading and meaning.
#[derive(Clone, PartialEq)]
struct WordColumn {
    word: GlossedWord,
    theme: EditorTheme,
    font: Option<String>,
    rtl: bool,
}

impl WordColumn {
    /// The column, `width` wide once measured; until then at its own width,
    /// reported into slot `index` of `widths`.
    fn into_element_with_size(
        self,
        index: usize,
        mut widths: State<Vec<f32>>,
        width: Option<f32>,
    ) -> Element {
        let link = self.word.link.clone();
        rect()
            .key(index)
            .map(width, |r, width| r.width(Size::px(width)))
            .maybe(width.is_none(), |r| {
                r.on_sized(move |event: Event<SizedEventData>| {
                    let width = event.area.width();
                    if widths
                        .peek()
                        .get(index)
                        .is_some_and(|w| (w - width).abs() > 0.5)
                    {
                        widths.write()[index] = width;
                    }
                })
            })
            .maybe(link.is_some(), |r| r.cursor(CursorIcon::Pointer))
            .map(link, |r, link| {
                r.on_press(move |event: Event<PressEventData>| {
                    event.stop_propagation();
                    crate::plugins::navigate::open(
                        &SourceRef::from_link(&link),
                        Placement::Preferred,
                    );
                })
            })
            .child(self)
            .into_element()
    }
}

impl Component for WordColumn {
    #[sismo::function("editor::interlinear_word")]
    fn render(&self) -> impl IntoElement {
        let theme = &self.theme;
        let colors = &theme.colors;
        let mut word = label()
            .font_size(theme.body_size * WORD_SCALE)
            .color(colors.text)
            .text(self.word.text.clone());
        if let Some(font) = &self.font {
            word = word.font_family(font.clone());
        }
        rect()
            .cross_align(if self.rtl {
                Alignment::End
            } else {
                Alignment::Start
            })
            .child(word)
            .maybe_child(self.word.reading.clone().map(|reading| {
                label()
                    .font_size(theme.body_size * 0.75)
                    .font_slant(FontSlant::Italic)
                    .color(colors.muted)
                    .text(reading)
                    .into_element()
            }))
            .maybe_child(self.word.gloss.clone().map(|gloss| {
                label()
                    .font_size(theme.body_size * 0.85)
                    .color(colors.muted)
                    .text(gloss)
                    .into_element()
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::pack;

    #[test]
    fn words_fill_lines_in_reading_order() {
        let lines = pack(&[40.0, 40.0, 40.0, 40.0], 100.0, 10.0);
        assert_eq!(lines, vec![vec![0, 1], vec![2, 3]]);
    }

    #[test]
    fn a_word_wider_than_the_room_takes_a_line() {
        let lines = pack(&[30.0, 200.0, 30.0], 100.0, 10.0);
        assert_eq!(lines, vec![vec![0], vec![1], vec![2]]);
    }
}
