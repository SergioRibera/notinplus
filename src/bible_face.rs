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
use notein_bible::plugin_config;
use notein_bible::state::{BibleBookmark, BibleSnapshot, BibleViewerState};
use notein_bible::ui::chapter::plan_chapter;
use notein_bible::ui::reader::{
    CrossRefView, FootnoteView, cross_refs_by_verse, footnotes_by_verse,
};
use notein_bible::ui::{bottom_nav, reader, toolbar, translation_switcher};

use std::time::Duration;

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
    let starting_chapter = saved_snapshot
        .as_ref()
        .map(|s| s.current_chapter)
        .unwrap_or(1);

    let db_for_init = db.clone();
    let snap_for_init = saved_snapshot.clone();
    let mut state = use_state(move || {
        let mut st = BibleViewerState::new(&starting_bible, &starting_book, starting_chapter);
        if let Some(snap) = snap_for_init {
            snap.apply_to(&mut st);
        }
        if let Ok(chap) =
            db_for_init.load_chapter(&st.bible_id, &st.current_book, st.current_chapter)
        {
            st.push_chapter(chap);
        }
        st
    });
    let mut picker_open = use_state(|| false);
    let mut search_open = use_state(|| false);
    let mut search_query: State<String> = use_state(String::new);
    let mut search_hits: State<Vec<notein_bible::ui::search_sheet::SearchHitView>> =
        use_state(Vec::new);
    // Transient highlight on the destination verse of a TSK / xref jump
    // — set on click, cleared by a spawned timer after a few seconds so
    // the user eye lands on the referenced text before the colour fades.
    let mut flash_verse: State<Option<u16>> = use_state(|| None);

    // Save snapshot whenever `state` changes. `use_side_effect` reruns
    // on signal mutations so highlights + chapter navigation persist
    // without the user explicitly saving.
    use_side_effect(move || {
        let st = state.read();
        let snap = BibleSnapshot::from_state(&st);
        let bytes = snap.encode();
        if let Err(e) = crate::library::bodies::write_plugin_snapshot(item, BIBLE_PLUGIN_ID, &bytes)
        {
            log::warn!("bible: snapshot write failed: {e}");
        }
    });

    // Kick a lazy helloao fetch each time the viewer settles on a new
    // chapter. The spawned worker short-circuits when rows already
    // exist, so the real network traffic only fires on first open.
    use_side_effect(move || {
        let st = state.read();
        let bid = st.bible_id.clone();
        let book = st.current_book.clone();
        let chap = st.current_chapter;
        drop(st);
        if let Some(canvas) = crate::builtin::bible_canvas() {
            notein_bible::install_background::spawn_fetch_footnotes(
                canvas.db().clone(),
                canvas.config().clone(),
                bid,
                book,
                chap,
            );
        }
    });

    let st_snapshot = {
        let s = state.read();
        ReaderSnapshot::from_state(&s)
    };
    let abbrev = bibles
        .iter()
        .find(|b| b.id == st_snapshot.bible_id)
        .map(|b| b.name_local.clone())
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
    let on_search_toggle = move || {
        let current = *search_open.read();
        search_open.set(!current);
    };
    let router = RouterContext::get();
    let on_home = move || {
        let _ = router.push(crate::route::Route::Home);
    };
    let db_back = db.clone();
    let has_back = state.read().has_back();
    let on_back_cb = move || {
        let popped = state.write().pop_nav();
        if let Some(entry) = popped {
            if let Err(e) = state.write().jump_to_entry(&db_back, &entry) {
                log::warn!("bible: back nav: {e}");
            }
        }
    };
    let db_fwd = db.clone();
    let has_forward = state.read().has_forward();
    let on_forward_cb = move || {
        let popped = state.write().pop_nav_forward();
        if let Some(entry) = popped {
            if let Err(e) = state.write().jump_to_entry(&db_fwd, &entry) {
                log::warn!("bible: forward nav: {e}");
            }
        }
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
        .content(Content::Flex)
        .child(toolbar::top_toolbar(
            &abbrev,
            toolbar::ToolbarCallbacks {
                on_home: Some(Box::new(on_home)),
                on_back: has_back.then(|| Box::new(on_back_cb) as toolbar::BackHandler),
                on_forward: has_forward.then(|| Box::new(on_forward_cb) as toolbar::BackHandler),
                on_search: on_search_toggle,
                on_more: || log::info!("bible: more menu not wired yet"),
                on_translation: translation_cb,
            },
        ))
        .child(chapter_body(
            &db,
            st_snapshot.chapter.as_ref(),
            &st_snapshot.highlights,
            *flash_verse.read(),
            {
                let db = db.clone();
                move |(book, chap, verse): (String, u16, u16)| {
                    if let Err(e) = state.write().jump_to(&db, &book, chap) {
                        log::warn!("bible xref jump: {e}");
                    }
                    flash_verse.set(Some(verse));
                    // Clear after 3 seconds so the wash fades without
                    // the user needing a second click.
                    let mut flash_verse = flash_verse;
                    spawn_forever(async move {
                        async_io::Timer::after(Duration::from_secs(3)).await;
                        flash_verse.set(None);
                    });
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
    if *search_open.read() {
        let db_search = db.clone();
        let bible_for_search = st_snapshot.bible_id.clone();
        let books_for_search = db
            .list_books(&bible_for_search)
            .unwrap_or_default();
        let on_submit = move |query: String| {
            let hits = db_search
                .search(Some(&bible_for_search), &query, 50)
                .unwrap_or_default();
            let views: Vec<notein_bible::ui::search_sheet::SearchHitView> = hits
                .into_iter()
                .map(|h| {
                    let book_name = books_for_search
                        .iter()
                        .find(|b| b.book_id == h.book_id)
                        .map(|b| b.name.clone());
                    notein_bible::ui::search_sheet::hit_to_view(h, book_name.as_deref())
                })
                .collect();
            search_hits.set(views);
        };

        let hits_snapshot = search_hits.read().clone();
        let db_pick = db.clone();
        let picker_handler = EventHandler::<usize>::new(move |idx: usize| {
            let hits = search_hits.read().clone();
            if let Some(view) = hits.get(idx) {
                let src = view.source.clone();
                if let Err(e) = state.write().jump_to(&db_pick, &src.book_id, src.chapter) {
                    log::warn!("bible search jump: {e}");
                }
                flash_verse.set(Some(src.verse));
                let mut flash_verse = flash_verse;
                spawn_forever(async move {
                    async_io::Timer::after(Duration::from_secs(3)).await;
                    flash_verse.set(None);
                });
                search_open.set(false);
            }
        });

        column = column.child(
            rect()
                .position(Position::new_absolute().top(56.).right(16.))
                .width(Size::px(420.0))
                .max_height(Size::px(540.0))
                .padding((10., 12.))
                .with_corner_radius(10.0)
                .background(Color::from_rgb(32, 32, 32))
                .spacing(8.0)
                .child(notein_bible::ui::search_sheet::search_input(
                    Writable::<String>::from(search_query),
                    on_submit,
                ))
                .child(notein_bible::ui::search_sheet::search_results_list(
                    hits_snapshot,
                    picker_handler,
                )),
        );
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
    flash_verse: Option<u16>,
    on_jump: impl FnMut((String, u16, u16)) + 'static,
) -> impl IntoElement {
    let base = rect().width(Size::fill()).height(Size::flex(1.));
    match chapter {
        Some(data) => {
            let plan = plan_chapter(data);
            let hl_refs: Vec<&BibleBookmark> = highlights.iter().collect();
            // Advanced cross-refs honour the plugin config toggle so the
            // user can declutter the paragraph when the TSK letters are
            // enough. TSK letters always render regardless.
            let enabled = crate::builtin::bible_canvas()
                .map(|c| plugin_config::effective_enable_crossref(c.config()))
                .unwrap_or(true);
            let refs = if enabled {
                build_cross_refs(db, &data.bible_id, &data.book_id, data.chapter)
            } else {
                Vec::new()
            };
            let refs_map = cross_refs_by_verse(&refs);
            let footnotes = build_footnotes(db, &data.bible_id, &data.book_id, data.chapter);
            let footnotes_map = footnotes_by_verse(&footnotes);
            let handler = EventHandler::<(String, u16, u16)>::new(on_jump);
            base.child(reader::chapter_view(
                &plan,
                &hl_refs,
                &refs_map,
                &footnotes_map,
                handler,
                flash_verse,
                22.0,
                1.65,
            ))
        }
        None => base.center().child(
            label()
                .color(Color::from_rgb(200, 200, 200))
                .text("Cargando capítulo…"),
        ),
    }
}

/// Pull publisher footnotes for `(book, chapter)` from the helloao
/// cache + resolve each `Cited in …` style text into a navigable
/// target. Footnotes without a parseable target render as hover-only
/// (no click action).
fn build_footnotes(db: &Db, bible_id: &str, book_id: &str, chapter: u16) -> Vec<FootnoteView> {
    let rows = db
        .footnotes_for_chapter(bible_id, book_id, chapter)
        .unwrap_or_default();
    rows.into_iter()
        .map(|r| {
            let target = extract_target_from_note_text(&r.text);
            FootnoteView {
                src_verse: r.verse,
                caller: r.caller,
                text: r.text,
                target,
            }
        })
        .collect()
}

/// Parse `Cited in 2 Corinthians 4:6` / `See John 3:16` style refs out
/// of a footnote body. Returns the first match. Falls back to `None`
/// for prose-only footnotes.
fn extract_target_from_note_text(text: &str) -> Option<(String, u16, u16)> {
    use notein_bible::reference;
    use notein_bible::resolver;
    let parsed = reference::parse(text)?;
    let db_bible = crate::builtin::bible_canvas()?.db().clone();
    let bibles = db_bible.list_bibles().ok()?;
    let first_bible = bibles.first()?.id.clone();
    let anchor = resolver::resolve_parsed(&db_bible, &first_bible, &parsed)?;
    Some((anchor.book_id, anchor.start_chapter, anchor.start_verse))
}

/// Fetch every cross-reference the DB holds for `(book, chapter)` and
/// resolve the target verse text so the reader can render inline chips
/// with hover popups. One DB round-trip per unique target — Bible
/// chapters cap at ~150 refs so the cost stays tiny.
fn build_cross_refs(db: &Db, bible_id: &str, book_id: &str, chapter: u16) -> Vec<CrossRefView> {
    let rows = db
        .cross_refs_for_chapter(bible_id, book_id, chapter)
        .unwrap_or_default();
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
