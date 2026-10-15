//! Standalone Bible reader page.
//!
//! Opens the shared `notein_bible::Db` on mount, keeps per-tab state
//! (`BibleViewerState`) in a Freya `State<_>` and renders the plugin's
//! UI builders. Highlights land in the state and persist for the
//! lifetime of the mounted component — a follow-up persists them in
//! the DB alongside the verse rows.

use freya::prelude::*;

use notein_bible::db::{BibleRow, BookRow, ChapterRow, Db};
use notein_bible::state::{BibleBookmark, BibleViewerState};
use notein_bible::ui::chapter::plan_chapter;
use notein_bible::ui::{bottom_nav, reader, toolbar, translation_switcher};

#[derive(Debug, PartialEq)]
pub struct BibleFace;

impl Component for BibleFace {
    fn render(&self) -> impl IntoElement {
        let inner: Element = match Db::open_default() {
            Err(e) => placeholder(format!("No se pudo abrir la DB: {e}")).into_element(),
            Ok(db) => {
                let bibles = db.list_bibles().unwrap_or_default();
                if bibles.is_empty() {
                    placeholder(
                        "No hay Biblias instaladas. Usá «Instalar Biblia» desde el menú +.".into(),
                    )
                    .into_element()
                } else {
                    bible_viewer(db, bibles).into_element()
                }
            }
        };
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .background(Color::from_rgb(10, 10, 10))
            .child(inner)
    }
}

fn bible_viewer(db: Db, bibles: Vec<BibleRow>) -> impl IntoElement {
    let first_book = db
        .list_books(&bibles[0].id)
        .unwrap_or_default()
        .into_iter()
        .next()
        .unwrap_or(BookRow {
            book_id: "GEN".into(),
            name: "Génesis".into(),
            abbrev: "Gn".into(),
            order_num: 1,
            testament: "OT".into(),
        });

    let db_for_init = db.clone();
    let bible_id_for_init = bibles[0].id.clone();
    let book_id_for_init = first_book.book_id.clone();
    let mut state = use_state(move || {
        let mut st = BibleViewerState::new(&bible_id_for_init, &book_id_for_init, 1);
        if let Ok(chap) = db_for_init.load_chapter(
            &st.bible_id,
            &st.current_book,
            st.current_chapter,
        ) {
            st.push_chapter(chap);
        }
        if let Err(e) = st.hydrate_highlights(&db_for_init) {
            log::warn!("bible: hydrate highlights: {e}");
        }
        st
    });
    let mut picker_open = use_state(|| false);

    let st_snapshot = {
        let s = state.read();
        ReaderSnapshot::from_state(&s)
    };
    let abbrev = bibles
        .iter()
        .find(|b| b.id == st_snapshot.bible_id)
        .map(|b| b.abbrev.clone())
        .unwrap_or_else(|| st_snapshot.bible_id.clone());

    let db_prev = db.clone();
    let on_prev = move || {
        let bible_id = state.read().bible_id.clone();
        let books = db_prev.list_books(&bible_id).unwrap_or_default();
        if let Err(e) = state.write().go_prev_chapter(&db_prev, &books) {
            log::warn!("bible prev: {e}");
        }
    };
    let db_next = db.clone();
    let on_next = move || {
        let bible_id = state.read().bible_id.clone();
        let books = db_next.list_books(&bible_id).unwrap_or_default();
        if let Err(e) = state.write().go_next_chapter(&db_next, &books) {
            log::warn!("bible next: {e}");
        }
    };
    let translation_cb = move || {
        let current = *picker_open.read();
        picker_open.set(!current);
    };

    let picker_overlay = if *picker_open.read() {
        let db_pick = db.clone();
        let bibles_pick = bibles.clone();
        let current_bible = st_snapshot.bible_id.clone();
        Some(
            rect()
                .position(Position::new_absolute().right(16.).top(60.))
                .child(translation_switcher::translation_switcher(
                    bibles_pick,
                    &current_bible,
                    move |new_bible_id: String| {
                        if let Err(e) = state.write().switch_bible(&db_pick, &new_bible_id) {
                            log::warn!("bible switch: {e}");
                        }
                        picker_open.set(false);
                    },
                    || log::info!("bible: use «Instalar Biblia» from the + menu"),
                )),
        )
    } else {
        None
    };

    let mut column = rect()
        .width(Size::fill())
        .height(Size::fill())
        .child(toolbar::top_toolbar(
            &abbrev,
            toolbar::ToolbarCallbacks {
                on_search: || log::info!("bible: search sheet not wired yet"),
                on_more: || log::info!("bible: more menu not wired yet"),
                on_translation: translation_cb,
            },
        ))
        .child(chapter_body(st_snapshot.chapter.as_ref(), &st_snapshot.highlights))
        .child(bottom_nav::bottom_nav(
            &st_snapshot.book_name,
            st_snapshot.current_chapter,
            bottom_nav::BottomNavCallbacks { on_prev, on_next },
        ));
    if let Some(overlay) = picker_overlay {
        column = column.child(overlay);
    }
    column
}

struct ReaderSnapshot {
    bible_id: String,
    book_name: String,
    current_chapter: u16,
    chapter: Option<ChapterRow>,
    highlights: Vec<BibleBookmark>,
}

impl ReaderSnapshot {
    fn from_state(st: &BibleViewerState) -> Self {
        let chapter = st.current_chapter_data().cloned();
        let book_name = chapter
            .as_ref()
            .map(|c| c.book_name.clone())
            .unwrap_or_else(|| st.current_book.clone());
        let highlights = st
            .highlights_on(&st.current_book, st.current_chapter)
            .into_iter()
            .cloned()
            .collect();
        Self {
            bible_id: st.bible_id.clone(),
            book_name,
            current_chapter: st.current_chapter,
            chapter,
            highlights,
        }
    }
}

fn chapter_body(
    chapter: Option<&ChapterRow>,
    highlights: &[BibleBookmark],
) -> impl IntoElement {
    let base = rect().width(Size::fill()).height(Size::fill());
    match chapter {
        Some(data) => {
            let plan = plan_chapter(data);
            let hl_refs: Vec<&BibleBookmark> = highlights.iter().collect();
            base.child(reader::chapter_view(&plan, &hl_refs, 22.0, 1.65))
        }
        None => base.center().child(
            label()
                .color(Color::from_rgb(200, 200, 200))
                .text("Cargando capítulo…"),
        ),
    }
}

fn placeholder(message: String) -> impl IntoElement {
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .center()
        .padding((24., 32.))
        .child(
            label()
                .color(Color::from_rgb(220, 220, 220))
                .font_size(16.0)
                .text(message),
        )
}
