//! Bible reader page bound to a library item.
//!
//! The item carries [`ItemKind::PluginCanvas`] with `plugin_id =
//! "io.notein.bible.viewer"` + `init_bytes` holding the chosen
//! translation id. Highlights + the user's last-read coordinates live
//! in a notein-managed snapshot sidecar
//! ([`crate::library::bodies::plugin_snapshot_path`]) that the shell
//! writes via `CanvasKind::serialize_snapshot`. This keeps the Bible
//! plugin's own SQLite focused on the installed text + search index
//! only — user-authored data flows through notein's generic storage
//! so bundle export + Phase 10 CRDT sync pick it up for free.

use std::sync::Arc;

use freya::prelude::*;
use freya::router::RouterContext;

use notein_bible::db::{BibleRow, BookRow, ChapterRow, Db};
use notein_bible::state::{BibleBookmark, BibleSnapshot, BibleViewerState};
use notein_bible::ui::chapter::plan_chapter;
use notein_bible::ui::reader::{CrossRefView, cross_refs_by_verse};
use notein_bible::ui::{bottom_nav, reader, toolbar, translation_switcher};

use crate::library::ItemId;

const BIBLE_PLUGIN_ID: &str = "io.notein.bible.viewer";

#[derive(Debug, PartialEq)]
pub struct BibleFace {
    pub item: ItemId,
}

impl Component for BibleFace {
    fn render(&self) -> impl IntoElement {
        let item = self.item;
        let inner: Element = match Db::open_default() {
            Err(e) => placeholder(format!("No se pudo abrir la DB: {e}")).into_element(),
            Ok(db) => {
                let bibles = db.list_bibles().unwrap_or_default();
                if bibles.is_empty() {
                    empty_state_with_install().into_element()
                } else {
                    bible_viewer(item, db, bibles).into_element()
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

fn bible_viewer(item: ItemId, db: Db, bibles: Vec<BibleRow>) -> impl IntoElement {
    let saved_snapshot: Option<BibleSnapshot> =
        crate::library::bodies::read_plugin_snapshot(item, BIBLE_PLUGIN_ID)
            .ok()
            .flatten()
            .and_then(|bytes| BibleSnapshot::decode(&bytes));

    let starting_bible = saved_snapshot
        .as_ref()
        .map(|s| s.bible_id.clone())
        .unwrap_or_else(|| bibles[0].id.clone());
    let books = db.list_books(&starting_bible).unwrap_or_default();
    let starting_book = saved_snapshot
        .as_ref()
        .map(|s| s.current_book.clone())
        .unwrap_or_else(|| {
            books
                .iter()
                .find(|b| b.book_id.eq_ignore_ascii_case("gen"))
                .map(|b| b.book_id.clone())
                .or_else(|| books.first().map(|b| b.book_id.clone()))
                .unwrap_or_else(|| "GEN".into())
        });
    let starting_chapter = saved_snapshot.as_ref().map(|s| s.current_chapter).unwrap_or(1);

    let db_for_init = db.clone();
    let snap_for_init = saved_snapshot.clone();
    let mut state = use_state(move || {
        let mut st = BibleViewerState::new(&starting_bible, &starting_book, starting_chapter);
        if let Some(snap) = snap_for_init {
            snap.apply_to(&mut st);
        }
        if let Ok(chap) = db_for_init.load_chapter(&st.bible_id, &st.current_book, st.current_chapter) {
            st.push_chapter(chap);
        }
        st
    });
    let mut picker_open = use_state(|| false);

    // Save snapshot whenever `state` changes. `use_side_effect` reruns
    // on signal mutations so highlights + chapter navigation persist
    // without the user explicitly saving.
    use_side_effect(move || {
        let st = state.read();
        let snap = BibleSnapshot::from_state(&st);
        let bytes = snap.encode();
        if let Err(e) =
            crate::library::bodies::write_plugin_snapshot(item, BIBLE_PLUGIN_ID, &bytes)
        {
            log::warn!("bible: snapshot write failed: {e}");
        }
    });

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
    let router = RouterContext::get();
    let on_back = move || {
        let _ = router.push(crate::route::Route::Home);
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
                on_back: Some(Box::new(on_back)),
                on_search: || log::info!("bible: search sheet not wired yet"),
                on_more: || log::info!("bible: more menu not wired yet"),
                on_translation: translation_cb,
            },
        ))
        .child(chapter_body(
            &db,
            st_snapshot.chapter.as_ref(),
            &st_snapshot.highlights,
            {
                let db = db.clone();
                move |(book, chap, verse): (String, u16, u16)| {
                    if let Err(e) = state.write().jump_to(&db, &book, chap) {
                        log::warn!("bible xref jump: {e}");
                    }
                    let _ = verse; // scroll-to-verse lives on a follow-up
                }
            },
        ))
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
    db: &Db,
    chapter: Option<&ChapterRow>,
    highlights: &[BibleBookmark],
    on_jump: impl FnMut((String, u16, u16)) + 'static,
) -> impl IntoElement {
    let base = rect().width(Size::fill()).height(Size::fill());
    match chapter {
        Some(data) => {
            let plan = plan_chapter(data);
            let hl_refs: Vec<&BibleBookmark> = highlights.iter().collect();
            let refs = build_cross_refs(db, &data.bible_id, &data.book_id, data.chapter);
            let refs_map = cross_refs_by_verse(&refs);
            let handler = EventHandler::<(String, u16, u16)>::new(on_jump);
            base.child(reader::chapter_view(
                &plan, &hl_refs, &refs_map, handler, 22.0, 1.65,
            ))
        }
        None => base.center().child(
            label()
                .color(Color::from_rgb(200, 200, 200))
                .text("Cargando capítulo…"),
        ),
    }
}

/// Fetch every cross-reference the DB holds for `(book, chapter)` and
/// resolve the target verse text so the reader can render inline chips
/// with hover popups. One DB round-trip per unique target — Bible
/// chapters cap at ~150 refs so the cost stays tiny.
fn build_cross_refs(db: &Db, bible_id: &str, book_id: &str, chapter: u16) -> Vec<CrossRefView> {
    let rows = db.cross_refs_for_chapter(bible_id, book_id, chapter).unwrap_or_default();
    rows.into_iter()
        .map(|r| {
            let tooltip = db
                .verse_text(bible_id, &r.tgt_book, r.tgt_chap, r.tgt_verse)
                .ok()
                .flatten()
                .unwrap_or_default();
            let label = format!(
                "{} {}:{}",
                abbreviate_book(&r.tgt_book),
                r.tgt_chap,
                r.tgt_verse
            );
            CrossRefView {
                src_verse: r.src_verse,
                label,
                tooltip,
                tgt_book: r.tgt_book,
                tgt_chap: r.tgt_chap,
                tgt_verse: r.tgt_verse,
            }
        })
        .collect()
}

fn abbreviate_book(book_id: &str) -> String {
    // Rough display mapping: upper-case the first + collapse to 3-4
    // chars so chips stay compact in the paragraph.
    let s = book_id.to_lowercase();
    match s.as_str() {
        "gen" => "Gn".into(),
        "exo" => "Ex".into(),
        "lev" => "Lv".into(),
        "num" => "Nm".into(),
        "deu" => "Dt".into(),
        "jos" => "Jos".into(),
        "jdg" => "Jue".into(),
        "rut" => "Rt".into(),
        "1sa" => "1 Sm".into(),
        "2sa" => "2 Sm".into(),
        "1ki" => "1 R".into(),
        "2ki" => "2 R".into(),
        "1ch" => "1 Cr".into(),
        "2ch" => "2 Cr".into(),
        "ezr" => "Esd".into(),
        "neh" => "Neh".into(),
        "est" => "Est".into(),
        "job" => "Job".into(),
        "psa" => "Sal".into(),
        "pro" => "Pr".into(),
        "ecc" => "Ec".into(),
        "sng" => "Cnt".into(),
        "isa" => "Is".into(),
        "jer" => "Jer".into(),
        "lam" => "Lm".into(),
        "ezk" => "Ez".into(),
        "dan" => "Dn".into(),
        "hos" => "Os".into(),
        "jol" => "Jl".into(),
        "amo" => "Am".into(),
        "oba" => "Abd".into(),
        "jon" => "Jon".into(),
        "mic" => "Miq".into(),
        "nam" => "Nah".into(),
        "hab" => "Hab".into(),
        "zep" => "Sof".into(),
        "hag" => "Hag".into(),
        "zec" => "Zac".into(),
        "mal" => "Mal".into(),
        "mat" => "Mt".into(),
        "mrk" => "Mc".into(),
        "luk" => "Lc".into(),
        "jhn" => "Jn".into(),
        "act" => "Hch".into(),
        "rom" => "Ro".into(),
        "1co" => "1 Co".into(),
        "2co" => "2 Co".into(),
        "gal" => "Gá".into(),
        "eph" => "Ef".into(),
        "php" => "Flp".into(),
        "col" => "Col".into(),
        "1th" => "1 Ts".into(),
        "2th" => "2 Ts".into(),
        "1ti" => "1 Ti".into(),
        "2ti" => "2 Ti".into(),
        "tit" => "Tit".into(),
        "phm" => "Flm".into(),
        "heb" => "Heb".into(),
        "jas" => "Stg".into(),
        "1pe" => "1 P".into(),
        "2pe" => "2 P".into(),
        "1jn" => "1 Jn".into(),
        "2jn" => "2 Jn".into(),
        "3jn" => "3 Jn".into(),
        "jud" => "Jud".into(),
        "rev" => "Ap".into(),
        _ => book_id.to_string(),
    }
}

fn empty_state_with_install() -> impl IntoElement {
    let on_install = move |_: Event<PressEventData>| {
        crate::home::fab_install_bible();
    };
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .center()
        .padding((32., 32.))
        .spacing(14.0)
        .child(
            label()
                .font_size(18.0)
                .color(Color::from_rgb(230, 230, 230))
                .text("No hay Biblias instaladas"),
        )
        .child(
            label()
                .font_size(14.0)
                .color(Color::from_rgb(170, 170, 170))
                .text("Descargá una traducción para empezar a leer."),
        )
        .child(
            rect()
                .padding((10., 20.))
                .with_corner_radius(10.0)
                .background(Color::from_rgb(55, 80, 140))
                .on_press(on_install)
                .child(
                    label()
                        .font_size(14.0)
                        .color(Color::from_rgb(240, 240, 240))
                        .text("Instalar Biblia (RVR1960)"),
                ),
        )
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

// Suppress unused Arc import warning when Arc gets dropped by edits.
#[allow(dead_code)]
fn _unused() -> Option<Arc<()>> {
    None
}
