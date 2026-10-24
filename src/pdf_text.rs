//! Selecting text in an open PDF: hit-testing a point to a character,
//! extending a range while the pointer drags, and the rectangles and
//! text of what is selected.
//!
//! Pages are laid out exactly as the PDF background paints them
//! ([`freya_pdf::write::page_world_rects`]).

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use freya_canvas_bg::Rect;
use freya_pdf::PdfDocument;

use crate::library::{ItemId, bodies};

/// A selected range, by page and character index into
/// [`PdfDocument::extract_page_text`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PdfSelection {
    pub start_page: u32,
    pub start_char: u32,
    pub end_page: u32,
    pub end_char: u32,
}

impl PdfSelection {
    /// The same range with its start before its end.
    #[must_use]
    pub const fn normalized(self) -> Self {
        let swap = self.start_page > self.end_page
            || (self.start_page == self.end_page && self.start_char > self.end_char);
        if swap {
            Self {
                start_page: self.end_page,
                start_char: self.end_char,
                end_page: self.start_page,
                end_char: self.start_char,
            }
        } else {
            self
        }
    }

    /// A caret, not a range.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start_page == self.end_page && self.start_char == self.end_char
    }
}

/// How a drag extends: character by character, or whole words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectMode {
    Point,
    Word,
}

#[derive(Debug, Clone, Copy)]
struct Page {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

#[derive(Debug, Clone, Copy)]
struct Drag {
    page: u32,
    char_idx: u32,
    mode: SelectMode,
}

#[derive(Debug)]
struct OpenPdf {
    doc: PdfDocument,
    pages: Vec<Page>,
    selection: Option<PdfSelection>,
    drag: Option<Drag>,
}

impl OpenPdf {
    fn open(item: ItemId) -> Option<Self> {
        let bytes = bodies::read_pdf(item).ok()?;
        let doc = PdfDocument::open_bytes(bytes).ok()?;
        let pages = freya_pdf::write::page_world_rects(doc.page_sizes(), freya_pdf::write::PAGE_GAP)
            .into_iter()
            .map(|(x, y, w, h)| Page {
                min_x: x,
                min_y: y,
                max_x: x + w,
                max_y: y + h,
            })
            .collect();
        Some(Self {
            doc,
            pages,
            selection: None,
            drag: None,
        })
    }

    /// `(page, local_x, local_y)` under world point `(x, y)`.
    fn hit_page(&self, x: f32, y: f32) -> Option<(u32, f32, f32)> {
        self.pages.iter().enumerate().find_map(|(index, page)| {
            (y >= page.min_y && y < page.max_y).then(|| {
                let local_x = x.clamp(page.min_x, page.max_x - 1.0) - page.min_x;
                (u32::try_from(index).unwrap_or(u32::MAX), local_x, y - page.min_y)
            })
        })
    }

    fn char_at(&self, x: f32, y: f32) -> Option<(u32, usize)> {
        let (page, lx, ly) = self.hit_page(x, y)?;
        let index = self.doc.hit_char_at(page as usize, lx, ly).ok()??;
        Some((page, index))
    }

    fn word_around(&self, page: u32, index: usize) -> (usize, usize) {
        self.doc
            .extract_page_text_indexed(page as usize)
            .map_or((index, index + 1), |(text, starts)| word_bounds(&text, &starts, index))
    }

    fn page_chars(&self, page: u32) -> usize {
        self.doc
            .extract_page_text(page as usize)
            .map_or(0, |t| t.chars().count())
    }

    /// Character range of `sel` on `page`.
    fn range_on(&self, page: u32, sel: PdfSelection) -> Option<(usize, usize)> {
        if page < sel.start_page || page > sel.end_page {
            return None;
        }
        let start = if page == sel.start_page { sel.start_char as usize } else { 0 };
        let end = if page == sel.end_page { sel.end_char as usize } else { self.page_chars(page) };
        Some((start, end))
    }
}

/// Expand `anchor` over `text` (char-indexed by `starts`) to the word
/// around it, half-open.
fn word_bounds(text: &str, starts: &[usize], anchor: usize) -> (usize, usize) {
    let last = starts.len().saturating_sub(1);
    if anchor >= last {
        return (last, last);
    }
    let char_at = |i: usize| starts.get(i).and_then(|s| text[*s..].chars().next());
    let is_word = |i: usize| char_at(i).is_some_and(|c| !c.is_whitespace());
    if !is_word(anchor) {
        return (anchor, anchor + 1);
    }
    let start = (0..anchor).rev().take_while(|i| is_word(*i)).last().unwrap_or(anchor);
    let end = (anchor + 1..last).find(|i| !is_word(*i)).unwrap_or(last);
    (start, end)
}

/// Text of `page` between char indices, `end = usize::MAX` to the end.
fn page_slice(doc: &PdfDocument, page: u32, start: usize, end: usize) -> Option<String> {
    let (text, starts) = doc.extract_page_text_indexed(page as usize).ok()?;
    let last = starts.len().saturating_sub(1);
    let end = end.min(last);
    let start = start.min(end);
    Some(text[*starts.get(start)?..*starts.get(end)?].to_owned())
}

/// Text selection over every open PDF, keyed by document.
#[derive(Debug, Default)]
pub struct PdfText {
    open: Mutex<HashMap<ItemId, OpenPdf>>,
}

impl PdfText {
    /// The process-wide engine.
    #[must_use]
    pub fn get() -> &'static Self {
        static ENGINE: OnceLock<PdfText> = OnceLock::new();
        ENGINE.get_or_init(Self::default)
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<ItemId, OpenPdf>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn with<R>(&self, item: ItemId, f: impl FnOnce(&mut OpenPdf) -> R) -> Option<R> {
        let mut open = self.lock();
        let pdf = match open.entry(item) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => entry.insert(OpenPdf::open(item)?),
        };
        Some(f(pdf))
    }

    /// Start a selection at world point `(x, y)`; `false` when it is not
    /// on text.
    pub fn begin(&self, item: ItemId, x: f32, y: f32, mode: SelectMode) -> bool {
        self.with(item, |pdf| {
            let Some((page, index)) = pdf.char_at(x, y) else {
                return false;
            };
            let (start, end) = match mode {
                SelectMode::Point => (index, index),
                SelectMode::Word => pdf.word_around(page, index),
            };
            let (start, end) = (u32::try_from(start).unwrap_or(u32::MAX), u32::try_from(end).unwrap_or(u32::MAX));
            pdf.drag = Some(Drag { page, char_idx: start, mode });
            pdf.selection = Some(PdfSelection {
                start_page: page,
                start_char: start,
                end_page: page,
                end_char: end,
            });
            true
        })
        .unwrap_or(false)
    }

    /// Extend the selection being dragged to world point `(x, y)`.
    pub fn extend(&self, item: ItemId, x: f32, y: f32) {
        self.with(item, |pdf| {
            let Some(drag) = pdf.drag else {
                return;
            };
            let Some((page, index)) = pdf.char_at(x, y) else {
                return;
            };
            let to_u32 = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
            pdf.selection = Some(match drag.mode {
                SelectMode::Point => PdfSelection {
                    start_page: drag.page,
                    start_char: drag.char_idx,
                    end_page: page,
                    end_char: to_u32(index),
                },
                SelectMode::Word => {
                    let (anchor_start, anchor_end) = pdf.word_around(drag.page, drag.char_idx as usize);
                    let (live_start, live_end) = pdf.word_around(page, index);
                    if (page, to_u32(index)) >= (drag.page, drag.char_idx) {
                        PdfSelection {
                            start_page: drag.page,
                            start_char: to_u32(anchor_start),
                            end_page: page,
                            end_char: to_u32(live_end),
                        }
                    } else {
                        PdfSelection {
                            start_page: page,
                            start_char: to_u32(live_start),
                            end_page: drag.page,
                            end_char: to_u32(anchor_end),
                        }
                    }
                }
            });
        });
    }

    /// The drag ended; the range stays selected.
    pub fn end(&self, item: ItemId) {
        self.with(item, |pdf| pdf.drag = None);
    }

    pub fn clear(&self, item: ItemId) {
        self.with(item, |pdf| {
            pdf.drag = None;
            pdf.selection = None;
        });
    }

    /// Close the document (its face went away).
    pub fn forget(&self, item: ItemId) {
        self.lock().remove(&item);
    }

    /// What is selected in `item`, when anything is.
    #[must_use]
    pub fn selection(&self, item: ItemId) -> Option<PdfSelection> {
        self.lock()
            .get(&item)
            .and_then(|pdf| pdf.selection)
            .map(PdfSelection::normalized)
            .filter(|s| !s.is_empty())
    }

    /// World rectangles covering the selection.
    #[must_use]
    pub fn selection_rects(&self, item: ItemId) -> Vec<Rect> {
        let open = self.lock();
        let Some(pdf) = open.get(&item) else {
            return Vec::new();
        };
        let Some(sel) = pdf.selection.map(PdfSelection::normalized).filter(|s| !s.is_empty()) else {
            return Vec::new();
        };
        let rects = (sel.start_page..=sel.end_page)
            .filter_map(|page| {
                let (start, end) = pdf.range_on(page, sel)?;
                let origin = pdf.pages.get(page as usize)?;
                let rects = pdf.doc.char_rects(page as usize, start, end).ok()?;
                Some(rects.into_iter().map(move |r| Rect {
                    min_x: r.min_x + origin.min_x,
                    min_y: r.min_y + origin.min_y,
                    max_x: r.max_x + origin.min_x,
                    max_y: r.max_y + origin.min_y,
                }))
            })
            .flatten()
            .collect();
        drop(open);
        rects
    }

    /// The selected text; pages joined with a line break.
    #[must_use]
    pub fn selected_text(&self, item: ItemId) -> Option<String> {
        let open = self.lock();
        let pdf = open.get(&item)?;
        let sel = pdf.selection.map(PdfSelection::normalized).filter(|s| !s.is_empty())?;
        let parts: Vec<String> = (sel.start_page..=sel.end_page)
            .filter_map(|page| {
                let (start, end) = pdf.range_on(page, sel)?;
                page_slice(&pdf.doc, page, start, end)
            })
            .collect();
        drop(open);
        Some(parts.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::{PdfSelection, word_bounds};

    #[test]
    fn normalized_orders_the_ends() {
        let sel = PdfSelection {
            start_page: 2,
            start_char: 5,
            end_page: 1,
            end_char: 9,
        }
        .normalized();
        assert_eq!((sel.start_page, sel.start_char, sel.end_page, sel.end_char), (1, 9, 2, 5));
    }

    #[test]
    fn word_bounds_expand_to_spaces() {
        let text = "hola mundo bello";
        let starts: Vec<usize> = text.char_indices().map(|(i, _)| i).chain(std::iter::once(text.len())).collect();
        assert_eq!(word_bounds(text, &starts, 7), (5, 10));
        assert_eq!(word_bounds(text, &starts, 4), (4, 5));
    }
}
