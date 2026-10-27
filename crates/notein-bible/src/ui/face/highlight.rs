//! Highlighting verses: a double tap, a long press or a drag over verses
//! opens a selection, which then grows and shrinks verse by verse with
//! taps. Its bar (inks, note, reference, notebook, share) stays docked
//! until it is done or closed.

use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;

use freya::prelude::*;
use notein_sdk::bookmark::SourceRef;
use notein_sdk::services::{AnnotationOutcome, AnnotationRequest, Share};
use notein_sdk::ui::{SelectionExtra, SelectionMenu};
use notein_ui_kit::theme::palette;

use super::Reader;
use crate::BibleAnchor;
use crate::kind::KIND;
use crate::state::{BibleBookmark, rgba_to_u32, u32_to_rgba};
use crate::ui::reader::{HighlightPreview, HighlightTarget, Verses};

/// The verses a selection holds, the chapter they are in and where it
/// started, plus the colour being tried on them.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Gesture {
    verses: State<Verses>,
    /// Book and chapter of the verses.
    place: State<Option<(String, u16)>>,
    /// Window point of the press that opened the selection.
    at: State<(f32, f32)>,
    preview: State<Option<Color>>,
}

impl std::fmt::Debug for Gesture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gesture")
            .field("verses", &*self.verses.peek())
            .finish_non_exhaustive()
    }
}

/// Runs of consecutive verses, as `(first, last)`.
fn runs(verses: &BTreeSet<u16>) -> Vec<(u16, u16)> {
    let mut runs: Vec<(u16, u16)> = Vec::new();
    for &verse in verses {
        match runs.last_mut() {
            Some((_, last)) if last.checked_add(1) == Some(verse) => *last = verse,
            _ => runs.push((verse, verse)),
        }
    }
    runs
}

/// The selected verses of the current chapter, as the menu needs them.
struct Selection {
    runs: Vec<(u16, u16)>,
    count: usize,
    book: String,
    chapter: u16,
    quote: String,
    /// Highlights already touching each run, in run order.
    existing: Vec<Option<BibleBookmark>>,
    anchors: Vec<BibleAnchor>,
}

impl Selection {
    fn of(reader: &Reader, verses: &BTreeSet<u16>) -> Self {
        let runs = runs(verses);
        let st = reader.state.peek();
        let quote = st
            .current_chapter_data()
            .map(|c| {
                c.verses
                    .iter()
                    .filter(|v| verses.contains(&v.verse))
                    .map(|v| v.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        let marks = st.highlights_on(&st.current_book, st.current_chapter);
        let existing = runs
            .iter()
            .map(|&(lo, hi)| {
                marks
                    .iter()
                    .find(|h| h.start_verse <= hi && h.end_verse >= lo)
                    .map(|h| (*h).clone())
            })
            .collect();
        let anchors = runs
            .iter()
            .map(|&(lo, hi)| BibleAnchor {
                bible_id: st.bible_id.clone(),
                book_id: st.current_book.clone(),
                start_chapter: st.current_chapter,
                start_verse: lo,
                end_chapter: st.current_chapter,
                end_verse: hi,
                start_char: None,
                end_char: None,
            })
            .collect();
        Self {
            runs,
            count: verses.len(),
            book: st.current_book.clone(),
            chapter: st.current_chapter,
            quote,
            existing,
            anchors,
        }
    }

    /// The one run of consecutive verses, when that is all there is.
    fn single(&self) -> Option<(u16, u16)> {
        match self.runs.as_slice() {
            [only] => Some(*only),
            _ => None,
        }
    }

    /// A reference to the selection, when it is one run.
    fn reference(&self, reader: &Reader) -> Option<SourceRef> {
        let [anchor] = self.anchors.as_slice() else {
            return None;
        };
        Some(SourceRef::Plugin {
            plugin_id: KIND.to_owned(),
            item: reader.doc.0,
            anchor: anchor.encode(),
            selection: (!self.quote.is_empty()).then(|| self.quote.clone()),
        })
    }

    /// The ink every run already wears, as RGB, when they all wear the
    /// same.
    fn rgb(&self) -> Option<[u8; 3]> {
        let mut inks = self.existing.iter().map(|h| {
            h.as_ref()
                .and_then(|h| h.color)
                .map(u32_to_rgba)
                .map(|c| [c[0], c[1], c[2]])
        });
        let first = inks.next()??;
        inks.all(|ink| ink == Some(first)).then_some(first)
    }

    fn caption(&self) -> String {
        if self.count == 1 {
            "1 versículo".to_owned()
        } else {
            format!("{} versículos", self.count)
        }
    }

    fn title(&self) -> String {
        match self.single() {
            Some((lo, hi)) if lo == hi => format!("Verso {lo}"),
            Some((lo, hi)) => format!("Versos {lo}–{hi}"),
            None => self.caption(),
        }
    }
}

fn packed(rgb: [u8; 3]) -> u32 {
    rgba_to_u32([rgb[0], rgb[1], rgb[2], 0xFF])
}

impl Gesture {
    pub(super) fn use_gesture() -> Self {
        Self {
            verses: use_state(Verses::default),
            place: use_state(|| None),
            at: use_state(|| (0.0, 0.0)),
            preview: use_state(|| None),
        }
    }

    /// The colour being tried, over the verses it would cover.
    pub(super) fn preview(self) -> Option<HighlightPreview> {
        let color = (*self.preview.read())?;
        let verses = self.verses.read().clone();
        (!verses.is_empty()).then_some(HighlightPreview { verses, color })
    }

    /// The verses the selection holds, when it is on the chapter `reader`
    /// shows.
    pub(super) fn selected(self, reader: &Reader) -> Verses {
        let st = reader.state.read();
        let here = self.place.read().as_ref().is_some_and(|(book, chapter)| {
            *book == st.current_book && *chapter == st.current_chapter
        });
        if here {
            self.verses.read().clone()
        } else {
            Verses::default()
        }
    }

    /// Let go of the selection, bar and underline, once the reader moves
    /// to another chapter.
    pub(super) fn use_end_with_chapter(self, reader: &Reader) {
        let reader = reader.clone();
        use_side_effect(move || {
            let here = {
                let st = reader.state.read();
                (st.current_book.clone(), st.current_chapter)
            };
            let left = self
                .place
                .peek()
                .as_ref()
                .is_some_and(|place| *place != here);
            if left {
                self.end();
                reader.ui.close_selection();
            }
        });
    }

    /// Long presses, double taps and drags: open the selection or add to it.
    pub(super) fn handler(self, reader: &Reader) -> EventHandler<HighlightTarget> {
        let reader = reader.clone();
        EventHandler::new(move |target| self.open(&reader, target))
    }

    /// Taps while the selection is open: a verse joins or leaves it.
    pub(super) fn tap_handler(self, reader: &Reader) -> EventHandler<u16> {
        let reader = reader.clone();
        EventHandler::new(move |verse| self.toggle(&reader, verse))
    }

    fn end(self) {
        let (mut verses, mut place, mut preview) = (self.verses, self.place, self.preview);
        verses.set(Verses::default());
        place.set(None);
        preview.set(None);
    }

    /// The verses `target` names join the selection, which starts over
    /// when it was on another chapter.
    fn open(self, reader: &Reader, target: HighlightTarget) {
        let here = {
            let st = reader.state.peek();
            (st.current_book.clone(), st.current_chapter)
        };
        let mut verses: BTreeSet<u16> = if self.place.peek().as_ref() == Some(&here) {
            (**self.verses.peek()).clone()
        } else {
            BTreeSet::new()
        };
        let (lo, hi) = target.kind.span();
        verses.extend(lo..=hi);
        let (mut place, mut at) = (self.place, self.at);
        place.set(Some(here));
        at.set((target.anchor_x, target.anchor_y));
        self.show(reader, verses);
    }

    /// `verse` leaves the selection when it held it, else joins it. The
    /// bar goes away with the last verse.
    fn toggle(self, reader: &Reader, verse: u16) {
        let mut verses = (**self.verses.peek()).clone();
        if !verses.remove(&verse) {
            verses.insert(verse);
        }
        if verses.is_empty() {
            self.end();
            reader.ui.close_selection();
            return;
        }
        self.show(reader, verses);
    }

    /// Hold `verses` and show the bar for them.
    fn show(self, reader: &Reader, verses: BTreeSet<u16>) {
        let selection = Rc::new(Selection::of(reader, &verses));
        let mut held = self.verses;
        held.set(Rc::new(verses));
        let active = selection.rgb().and_then(|c| {
            palette()
                .highlighters
                .iter()
                .position(|ink| [ink.r(), ink.g(), ink.b()] == c)
        });
        let on_color = {
            let (state, selection) = (reader.state, Rc::clone(&selection));
            Rc::new(move |rgb: Option<[u8; 3]>| {
                let mut state = state;
                for (&(lo, hi), existing) in selection.runs.iter().zip(&selection.existing) {
                    let note = existing
                        .as_ref()
                        .map(|h| h.note.clone())
                        .unwrap_or_default();
                    state.write().upsert_highlight(
                        &selection.book,
                        selection.chapter,
                        lo,
                        hi,
                        rgb.map(packed),
                        note,
                    );
                }
                self.end();
            })
        };
        let on_note = selection.single().map(|_| {
            let (reader, selection) = (reader.clone(), Rc::clone(&selection));
            Rc::new(move || {
                let (x, y) = *self.at.peek();
                self.annotate(&reader, &selection, x, y);
            }) as Rc<dyn Fn()>
        });
        let reference = selection.reference(reader);
        reader.ui.selection(SelectionMenu {
            at: *self.at.peek(),
            active,
            on_color,
            on_note,
            document: reference.as_ref().map(|_| reader.doc),
            reference,
            extra: self.share(reader, &selection).into_iter().collect(),
            caption: Some(selection.caption()),
            sticky: true,
            on_dismiss: Rc::new(move || self.end()),
        });
    }

    /// The bar button that sends the selected verses, with their reference,
    /// to another app; `None` where the system cannot share.
    fn share(self, reader: &Reader, selection: &Selection) -> Option<SelectionExtra> {
        if !reader.host.can_share() {
            return None;
        }
        let (host, kind) = (reader.host.clone(), Arc::clone(&reader.kind));
        let anchors = selection.anchors.clone();
        let share: Rc<dyn Fn()> = Rc::new(move || {
            if let Some(text) = kind.shared_text(&anchors) {
                host.share(Share::Text {
                    text,
                    subject: kind.label_of_passages(&anchors),
                });
            }
            self.end();
        });
        Some(("Compartir", freya::icons::lucide::share_2(), share))
    }

    /// The note popover for a selection of one run; its colour previews
    /// live on the verses.
    fn annotate(self, reader: &Reader, selection: &Selection, x: f32, y: f32) {
        let Some((lo, hi)) = selection.single() else {
            return;
        };
        let existing = selection.existing.first().cloned().flatten();
        let (done_tx, done_rx) = flume::bounded::<AnnotationOutcome>(1);
        let (preview_tx, preview_rx) = flume::unbounded::<Option<[u8; 3]>>();
        let mut preview = self.preview;
        spawn_in_window(async move {
            while let Ok(rgb) = preview_rx.recv_async().await {
                preview.set(rgb.map(|c| Color::from_rgb(c[0], c[1], c[2])));
            }
        });
        let mut state = reader.state;
        let (book, chapter) = (selection.book.clone(), selection.chapter);
        let existing_id = existing.as_ref().map(|h| h.id);
        spawn_in_window(async move {
            match done_rx.recv_async().await {
                Ok(AnnotationOutcome::Saved { color, body }) => {
                    state
                        .write()
                        .upsert_highlight(&book, chapter, lo, hi, color.map(packed), body);
                }
                Ok(AnnotationOutcome::Deleted) => {
                    if let Some(id) = existing_id {
                        state.write().remove_highlight(id);
                    }
                }
                Ok(AnnotationOutcome::Cancelled) | Err(_) => {}
            }
            self.end();
        });
        reader.host.annotate(
            AnnotationRequest {
                doc: reader.doc,
                kind: KIND.to_owned(),
                title: selection.title(),
                quote: (!selection.quote.is_empty()).then(|| selection.quote.clone()),
                color: selection.rgb(),
                body: existing
                    .as_ref()
                    .map(|h| h.note.clone())
                    .unwrap_or_default(),
                target: Some((x, y, 1.0, 1.0)),
                existing: existing_id.is_some(),
            },
            Some(Arc::new(move |rgb| {
                // The popover closed already when this fails.
                let _ = preview_tx.send(rgb);
            })),
            Arc::new(move |outcome| {
                let _ = done_tx.send(outcome);
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verses_group_into_runs_of_consecutive_ones() {
        let verses: BTreeSet<u16> = [1, 3, 4, 5, 9].into_iter().collect();
        assert_eq!(runs(&verses), [(1, 1), (3, 5), (9, 9)]);
        assert!(runs(&BTreeSet::new()).is_empty());
    }
}
