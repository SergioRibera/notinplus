//! Search in the open translation: words in the verses, or a
//! reference ("Juan 3:16") that goes straight there.

use freya::prelude::*;
use notein_bible::db::{BookRow, SearchHit};
use notein_bible::ui::search::{SearchHitView, hit_to_view};
use notein_ui_kit::theme::palette;
use notein_ui_kit::{EmptyState, ModalController, SearchField, Sheet, Text, TextRole, tokens};

use super::Reader;

/// Most verse matches listed.
const LIMIT: usize = 50;

pub(super) fn open(reader: &Reader) {
    Sheet::titled("Buscar en la Biblia")
        .child(SearchPane {
            reader: reader.clone(),
        })
        .open();
}

#[derive(Clone, Debug, PartialEq)]
struct SearchPane {
    reader: Reader,
}

impl Component for SearchPane {
    fn render(&self) -> impl IntoElement {
        let query = use_state(String::new);
        let typed = query.read().trim().to_owned();
        let hits = if typed.is_empty() {
            Vec::new()
        } else {
            find(&self.reader, &typed)
        };
        let results: Element = if !typed.is_empty() && hits.is_empty() {
            EmptyState::new(freya::icons::lucide::search_x(), "Sin resultados")
                .body("Probá con otras palabras o con una cita como Juan 3:16.")
                .into_element()
        } else {
            rect()
                .width(Size::fill())
                .spacing(2.0)
                .children(hits.into_iter().map(|hit| hit_row(&self.reader, hit)))
                .into_element()
        };
        rect()
            .width(Size::fill())
            .spacing(12.0)
            .child(SearchField::new(query, "Palabras o una cita (Juan 3:16)").auto_focus(true))
            .child(results)
    }
}

/// One result; pressing it opens the verse and closes the search.
fn hit_row(reader: &Reader, hit: SearchHitView) -> Element {
    let p = palette();
    let reader = reader.clone();
    let source = hit.source.clone();
    let mut snippet = paragraph()
        .width(Size::fill())
        .font_size(tokens::text::BODY)
        .color(p.t2)
        .max_lines(3);
    for run in hit.snippet_runs {
        snippet = snippet.span(if run.bold {
            Span::new(run.text).color(p.t1).font_weight(FontWeight::BOLD)
        } else {
            Span::new(run.text)
        });
    }
    rect()
        .width(Size::fill())
        .padding((10.0, 12.0))
        .spacing(4.0)
        .corner_radius(tokens::radius::MD)
        .on_press(move |_: Event<PressEventData>| {
            reader.go(&source.book_id, source.chapter, source.verse);
            ModalController::get().close();
        })
        .child(Text::new(hit.reference).role(TextRole::Strong))
        .child(snippet)
        .into_element()
}

/// Matches for `query`: a reference it names first, then verses with
/// its words.
fn find(reader: &Reader, query: &str) -> Vec<SearchHitView> {
    let db = &reader.db;
    let bible_id = reader.bible_id();
    let books = db.list_books(&bible_id).unwrap_or_default();
    let name_of = |book_id: &str| {
        books
            .iter()
            .find(|b| b.book_id == book_id)
            .map(|b| b.name.clone())
    };
    let mut views: Vec<SearchHitView> = db
        .search(Some(&bible_id), query, LIMIT)
        .unwrap_or_default()
        .into_iter()
        .map(|hit| {
            let name = name_of(&hit.book_id);
            hit_to_view(hit, name.as_deref())
        })
        .collect();
    if let Some(place) = reference(reader, &books, query) {
        views.insert(0, place);
    }
    views
}

/// `query` read as a reference to a verse that exists in the open
/// translation.
fn reference(reader: &Reader, books: &[BookRow], query: &str) -> Option<SearchHitView> {
    let db = &reader.db;
    let bible_id = reader.bible_id();
    let parsed = notein_bible::reference::parse(query)?;
    let book_id =
        notein_bible::resolver::resolve_book_token(db, &bible_id, &parsed.book_token)?;
    let chapters = db.chapter_count(&bible_id, &book_id).unwrap_or(0);
    if !(1..=chapters).contains(&parsed.chapter) {
        return None;
    }
    let verse = parsed.start_verse.unwrap_or(1);
    db.verse_text(&bible_id, &book_id, parsed.chapter, verse)
        .ok()
        .flatten()?;
    let name = books
        .iter()
        .find(|b| b.book_id == book_id)
        .map_or_else(|| book_id.clone(), |b| b.name.clone());
    let place = if parsed.start_verse.is_some() {
        format!("{name} {}:{verse}", parsed.chapter)
    } else {
        format!("{name} {}", parsed.chapter)
    };
    Some(SearchHitView {
        reference: format!("Ir a {place}"),
        snippet_runs: Vec::new(),
        source: SearchHit {
            bible_id,
            book_id,
            chapter: parsed.chapter,
            verse: if parsed.start_verse.is_some() { verse } else { 0 },
            snippet: String::new(),
            score: 0.0,
        },
    })
}
