//! Highlighting verses: a double tap or a drag over verses opens the
//! selection menu (inks, note, reference, notebook).

use std::rc::Rc;
use std::sync::Arc;

use freya::prelude::*;
use notein_bible::BibleAnchor;
use notein_bible::state::{BibleBookmark, rgba_to_u32, u32_to_rgba};
use notein_bible::ui::reader::{HighlightPreview, HighlightTarget};
use notein_plugin::bookmark::SourceRef;
use notein_plugin::services::{AnnotationOutcome, AnnotationRequest};

use super::{KIND, Reader};

/// The verses a gesture targets while its menu is open, and the colour
/// being tried on them.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Gesture {
    pending: State<Option<HighlightTarget>>,
    preview: State<Option<Color>>,
}

impl std::fmt::Debug for Gesture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gesture")
            .field("pending", &*self.pending.peek())
            .finish_non_exhaustive()
    }
}

/// The verses `lo..=hi` of the current chapter, as the menu needs them.
struct Selection {
    lo: u16,
    hi: u16,
    book: String,
    chapter: u16,
    quote: String,
    existing: Option<BibleBookmark>,
    reference: SourceRef,
}

impl Selection {
    fn of(reader: &Reader, target: HighlightTarget) -> Self {
        let (lo, hi) = target.kind.span();
        let st = reader.state.peek();
        let quote = st
            .current_chapter_data()
            .map(|c| {
                c.verses
                    .iter()
                    .filter(|v| (lo..=hi).contains(&v.verse))
                    .map(|v| v.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        let existing = st
            .highlights_on(&st.current_book, st.current_chapter)
            .into_iter()
            .find(|h| h.start_verse <= hi && h.end_verse >= lo)
            .cloned();
        let anchor = BibleAnchor {
            bible_id: st.bible_id.clone(),
            book_id: st.current_book.clone(),
            start_chapter: st.current_chapter,
            start_verse: lo,
            end_chapter: st.current_chapter,
            end_verse: hi,
            start_char: None,
            end_char: None,
        };
        let reference = SourceRef::Plugin {
            plugin_id: KIND.to_owned(),
            item: reader.item.0,
            anchor: anchor.encode(),
            selection: (!quote.is_empty()).then(|| quote.clone()),
        };
        Self {
            lo,
            hi,
            book: st.current_book.clone(),
            chapter: st.current_chapter,
            quote,
            existing,
            reference,
        }
    }

    /// The stored ink, as RGB.
    fn rgb(&self) -> Option<[u8; 3]> {
        self.existing
            .as_ref()
            .and_then(|h| h.color)
            .map(u32_to_rgba)
            .map(|c| [c[0], c[1], c[2]])
    }

    fn title(&self) -> String {
        if self.lo == self.hi {
            format!("Verso {}", self.lo)
        } else {
            format!("Versos {}–{}", self.lo, self.hi)
        }
    }
}

fn packed(rgb: [u8; 3]) -> u32 {
    rgba_to_u32([rgb[0], rgb[1], rgb[2], 0xFF])
}

impl Gesture {
    pub(super) fn use_gesture() -> Self {
        Self {
            pending: use_state(|| None),
            preview: use_state(|| None),
        }
    }

    /// The colour being tried, over the verses it would cover.
    pub(super) fn preview(self) -> Option<HighlightPreview> {
        let target = (*self.pending.read())?;
        let color = (*self.preview.read())?;
        let (lo, hi) = target.kind.span();
        Some(HighlightPreview { lo, hi, color })
    }

    /// First and last verse the open menu works on.
    pub(super) fn selected(self) -> Option<(u16, u16)> {
        (*self.pending.read()).map(|target| target.kind.span())
    }

    pub(super) fn handler(self, reader: &Reader) -> EventHandler<HighlightTarget> {
        let reader = reader.clone();
        EventHandler::new(move |target| self.open(&reader, target))
    }

    fn end(self) {
        let (mut pending, mut preview) = (self.pending, self.preview);
        pending.set(None);
        preview.set(None);
    }

    /// The selection menu for `target`.
    fn open(self, reader: &Reader, target: HighlightTarget) {
        let mut pending = self.pending;
        pending.set(Some(target));
        let selection = Rc::new(Selection::of(reader, target));
        let active = selection
            .rgb()
            .and_then(|c| crate::selection::inks().iter().position(|ink| *ink == c));
        let label = crate::selection::label_of(&selection.reference);
        let on_color = {
            let (state, selection) = (reader.state, Rc::clone(&selection));
            Rc::new(move |rgb: Option<[u8; 3]>| {
                let note = selection
                    .existing
                    .as_ref()
                    .map(|h| h.note.clone())
                    .unwrap_or_default();
                let mut state = state;
                state.write().upsert_highlight(
                    &selection.book,
                    selection.chapter,
                    selection.lo,
                    selection.hi,
                    rgb.map(packed),
                    note,
                );
                self.end();
            })
        };
        let on_note = {
            let (reader, selection) = (reader.clone(), Rc::clone(&selection));
            Rc::new(move || self.annotate(&reader, &selection, target.anchor_x, target.anchor_y))
        };
        crate::selection::SelectionMenu {
            at: (target.anchor_x, target.anchor_y),
            active,
            on_color,
            on_note,
            reference: Some((selection.reference.clone(), label)),
            document: Some(reader.item),
            extra: Vec::new(),
            on_dismiss: Rc::new(move || {
                let mut pending = self.pending;
                pending.set(None);
            }),
        }
        .show();
    }

    /// The note popover for `selection`; its colour previews live on the
    /// verses.
    fn annotate(self, reader: &Reader, selection: &Selection, x: f32, y: f32) {
        let (done_tx, done_rx) = flume::bounded::<AnnotationOutcome>(1);
        let (preview_tx, preview_rx) = flume::unbounded::<Option<[u8; 3]>>();
        let mut preview = self.preview;
        spawn_in_window(async move {
            while let Ok(rgb) = preview_rx.recv_async().await {
                preview.set(rgb.map(|c| Color::from_rgb(c[0], c[1], c[2])));
            }
        });
        let mut state = reader.state;
        let (book, chapter, lo, hi) = (selection.book.clone(), selection.chapter, selection.lo, selection.hi);
        let existing_id = selection.existing.as_ref().map(|h| h.id);
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
        crate::annotation::AnnotationPopover::open(
            AnnotationRequest {
                doc: crate::plugins::content::doc_id(reader.item),
                kind: KIND.to_owned(),
                title: selection.title(),
                quote: (!selection.quote.is_empty()).then(|| selection.quote.clone()),
                color: selection.rgb(),
                body: selection
                    .existing
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
