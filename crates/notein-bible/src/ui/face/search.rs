//! Search in the open translation: words in the verses, or a
//! reference ("Juan 3:16") that goes straight there.

use crate::db::{BibleRow, BookRow, SearchHit};
use crate::ui::search::{SearchHitView, hit_to_view};
use freya::prelude::*;
use notein_i18n::tr;
use notein_ui_kit::TapExt;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    EmptyState, ModalController, SearchField, Segmented, Sheet, Text, TextRole, tokens,
};

use super::Reader;

/// Most verse matches listed.
const LIMIT: usize = 50;
#[cfg_attr(feature = "sismo", sismo::function("bible::search_open"))]
pub(super) fn open(reader: &Reader) {
    Sheet::titled(tr!("search-title"))
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
    #[cfg_attr(feature = "sismo", sismo::function("bible::search_pane"))]
    fn render(&self) -> impl IntoElement {
        let query = use_state(String::new);
        let mut everywhere = use_state(|| false);
        let bibles = self.reader.db.list_bibles().unwrap_or_default();
        let several = bibles.len() > 1;
        let scope = (several && *everywhere.read()).then_some(bibles.as_slice());
        let typed = query.read().trim().to_owned();
        let hits = if typed.is_empty() {
            Vec::new()
        } else {
            find(&self.reader, &typed, scope)
        };
        let found = hits.len();
        let results: Element = if !typed.is_empty() && hits.is_empty() {
            EmptyState::new(freya::icons::lucide::search_x(), tr!("search-empty-title"))
                .body(tr!("search-empty-body"))
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
            .child(SearchField::new(query, tr!("search-placeholder")).auto_focus(true))
            .maybe_child(several.then(|| {
                Segmented::new(
                    [tr!("search-scope-this"), tr!("search-scope-all")],
                    usize::from(*everywhere.read()),
                )
                .full_width()
                .height(32.0)
                .on_select(move |i: usize| everywhere.set(i == 1))
            }))
            .child(
                Text::new(if typed.is_empty() {
                    tr!("search-help")
                } else if found >= LIMIT {
                    tr!("search-closest", "count" => LIMIT)
                } else {
                    tr!("search-results", "count" => found)
                })
                .role(TextRole::Hint),
            )
            .child(results)
    }
}

/// One result; pressing it opens the verse and closes the search.
#[cfg_attr(feature = "sismo", sismo::function("bible::search_hit_row"))]
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
            Span::new(run.text)
                .color(p.t1)
                .font_weight(FontWeight::BOLD)
        } else {
            Span::new(run.text)
        });
    }
    rect()
        .width(Size::fill())
        .padding((10.0, 12.0))
        .spacing(4.0)
        .corner_radius(tokens::radius::MD)
        .on_tap(move |_: Event<PressEventData>| {
            if !source.bible_id.is_empty() && source.bible_id != reader.bible_id() {
                reader.switch_bible(&source.bible_id);
            }
            reader.go(&source.book_id, source.chapter, source.verse);
            ModalController::get().close();
        })
        .child(Text::new(hit.reference).role(TextRole::Strong))
        .child(snippet)
        .into_element()
}

/// Matches for `query`: a reference it names first, then verses with
/// its words, in the open translation or in every one of `everywhere`
/// (each hit then says which).
#[cfg_attr(feature = "sismo", sismo::function("bible::search_find"))]
fn find(reader: &Reader, query: &str, everywhere: Option<&[BibleRow]>) -> Vec<SearchHitView> {
    let db = &reader.db;
    let bible_id = reader.bible_id();
    let books = db.list_books(&bible_id).unwrap_or_default();
    let name_of = |book_id: &str| {
        books
            .iter()
            .find(|b| b.book_id == book_id)
            .map(|b| b.name.clone())
    };
    let scope = everywhere.is_none().then_some(bible_id.as_str());
    let mut views: Vec<SearchHitView> = db
        .search(scope, query, LIMIT)
        .unwrap_or_default()
        .into_iter()
        .map(|hit| {
            let name = name_of(&hit.book_id);
            let abbrev = everywhere
                .and_then(|bibles| bibles.iter().find(|b| b.id == hit.bible_id))
                .map(|b| b.abbrev.clone());
            let mut view = hit_to_view(hit, name.as_deref());
            if let Some(abbrev) = abbrev {
                view.reference = format!("{} · {abbrev}", view.reference);
            }
            view
        })
        .collect();
    if let Some(place) = reference(reader, &books, query) {
        views.insert(0, place);
    }
    #[cfg(feature = "sismo")]
    sismo::counter!("bible.search_results", views.len());
    views
}

/// `query` read as a reference to a verse that exists in the open
/// translation.
#[cfg_attr(feature = "sismo", sismo::function("bible::search_reference"))]
fn reference(reader: &Reader, books: &[BookRow], query: &str) -> Option<SearchHitView> {
    let db = &reader.db;
    let bible_id = reader.bible_id();
    let parsed = crate::reference::parse(query)?;
    let book_id = crate::resolver::resolve_book_token(db, &bible_id, &parsed.book_token)?;
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
        reference: tr!("search-go-to", "place" => place),
        snippet_runs: Vec::new(),
        source: SearchHit {
            bible_id,
            book_id,
            chapter: parsed.chapter,
            verse: if parsed.start_verse.is_some() {
                verse
            } else {
                0
            },
            snippet: String::new(),
            score: 0.0,
        },
    })
}
