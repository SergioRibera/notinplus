//! Bible reader bound to a library item.
//!
//! Highlights and the last place read live in the item's snapshot, so
//! export and sync carry them; the plugin database only holds the
//! installed text and its search index.

mod highlight;
mod nav;
mod search;
mod translation;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use freya::components::{ScrollConfig, ScrollController, use_scroll_controller};
use freya::prelude::*;
use notein_bible::db::{ChapterRow, Db};
use notein_bible::plugin_config;
use notein_bible::state::{BibleBookmark, BibleSnapshot, BibleViewerState};
use notein_bible::ui::chapter::plan_chapter;
use notein_bible::ui::reader::{
    self, CrossRefView, FootnoteView, ScrollTarget, VerseScrollCtx, cross_refs_by_verse,
    footnotes_by_verse,
};
use notein_bible::ui::bottom_nav;
use notein_bible::BibleAnchor;
use notein_plugin::face::HeaderAction;
use notein_plugin::services::HostServices;
use notein_ui_kit::theme::palette;

use crate::library::{ItemId, ViewSlot};

pub(crate) const KIND: &str = notein_bible::kind::KIND;

/// How long a jump target stays washed.
const FLASH: Duration = Duration::from_secs(3);

/// The reader's buttons in the pane header.
pub(crate) fn header_actions() -> Vec<HeaderAction> {
    use freya::icons::lucide;
    vec![
        HeaderAction::new("translation", "Traducción", lucide::languages()),
        HeaderAction::new("search", "Buscar en la Biblia", lucide::search()),
    ]
}

/// The reader of `item`; the first-use card until a translation is
/// installed.
#[derive(Debug, PartialEq, Eq)]
pub struct BibleFace {
    pub item: ItemId,
    pub slot: Option<ViewSlot>,
}

impl Component for BibleFace {
    fn render(&self) -> impl IntoElement {
        // Re-render when an install finishes, so the first-use card
        // gives way to the reader.
        let _installs = *crate::screens::bible::use_installs().read();
        let db = crate::plugins::builtin::bible_kind().map(|kind| kind.db().clone());
        let inner = match db {
            None => notein_ui_kit::EmptyState::new(
                freya::icons::lucide::book_x(),
                "La Biblia no está disponible",
            )
            .body("No se pudo abrir la base de datos de traducciones.")
            .into_element(),
            Some(db) if db.list_bibles().unwrap_or_default().is_empty() => FirstUseFace {
                item: self.item,
                slot: self.slot,
            }
            .into_element(),
            Some(db) => BibleViewer {
                item: self.item,
                slot: self.slot,
                db,
            }
            .into_element(),
        };
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .background(palette().bg)
            .child(inner)
    }
}

/// The first-use card in a pane: with nothing installed yet, both header
/// buttons lead to the translations to download.
#[derive(Debug, PartialEq, Eq)]
struct FirstUseFace {
    item: ItemId,
    slot: Option<ViewSlot>,
}

impl Component for FirstUseFace {
    fn render(&self) -> impl IntoElement {
        crate::plugins::faces::use_header_presses(KIND, self.item, self.slot, |_| {
            crate::screens::bible::open_catalog();
        });
        crate::screens::bible::FirstUse
    }
}

/// Handles the parts of the reader share.
#[derive(Clone)]
pub(super) struct Reader {
    item: ItemId,
    db: Db,
    state: State<BibleViewerState>,
    flash: State<Option<u16>>,
    scroll_to: State<Option<ScrollTarget>>,
    scroll: ScrollController,
}

impl PartialEq for Reader {
    fn eq(&self, other: &Self) -> bool {
        self.item == other.item
    }
}

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reader")
            .field("item", &self.item)
            .finish_non_exhaustive()
    }
}

impl Reader {
    fn bible_id(&self) -> String {
        self.state.peek().bible_id.clone()
    }

    /// Go to `book` `chapter`, bring `verse` into view and wash it.
    fn go(&self, book: &str, chapter: u16, verse: u16) {
        let mut state = self.state;
        if let Err(e) = state.write().jump_to(&self.db, book, chapter) {
            log::warn!("bible: go to {book} {chapter}: {e}");
            return;
        }
        self.show(verse);
    }

    /// Scroll to `verse` (the top for 0) and wash it for a moment.
    fn show(&self, verse: u16) {
        if verse == 0 {
            let mut scroll = self.scroll;
            scroll.scroll_to_y(0);
        } else {
            let mut scroll_to = self.scroll_to;
            scroll_to.set(Some(ScrollTarget::new(verse)));
        }
        let mut flash = self.flash;
        flash.set(Some(verse));
        spawn_in_window(async move {
            async_io::Timer::after(FLASH).await;
            if *flash.peek() == Some(verse) {
                flash.set(None);
            }
        });
    }

    /// Follow `anchor`, switching translation when it names another one.
    fn follow(&self, anchor: &BibleAnchor) {
        if self.bible_id() != anchor.bible_id {
            self.switch_bible(&anchor.bible_id);
        }
        let book = canonical_book_id(&self.db, &anchor.bible_id, &anchor.book_id);
        self.go(&book, anchor.start_chapter, anchor.start_verse);
    }

    fn switch_bible(&self, bible_id: &str) {
        let mut state = self.state;
        if let Err(e) = state.write().switch_bible(&self.db, bible_id) {
            log::warn!("bible: switch to {bible_id}: {e}");
        }
    }

    /// The chapter after (or before) the current one, across books.
    fn step(&self, forward: bool) {
        let books = self.db.list_books(&self.bible_id()).unwrap_or_default();
        let mut state = self.state;
        let moved = if forward {
            state.write().go_next_chapter(&self.db, &books)
        } else {
            state.write().go_prev_chapter(&self.db, &books)
        };
        if let Err(e) = moved {
            log::warn!("bible: chapter step: {e}");
        }
    }

    /// Back (or forward) through the passages visited.
    fn history(&self, back: bool) {
        let mut state = self.state;
        let entry = if back {
            state.write().pop_nav()
        } else {
            state.write().pop_nav_forward()
        };
        if let Some(entry) = entry
            && let Err(e) = state.write().jump_to_entry(&self.db, &entry)
        {
            log::warn!("bible: history: {e}");
        }
    }
}

/// The installed `book_id` of `bible_id` matching `candidate` in any
/// case, else `candidate`. Stored anchors may carry the upper-case OSIS
/// id while an install keys its books in lower case.
fn canonical_book_id(db: &Db, bible_id: &str, candidate: &str) -> String {
    db.list_books(bible_id)
        .ok()
        .and_then(|books| {
            books
                .into_iter()
                .find(|b| b.book_id.eq_ignore_ascii_case(candidate))
                .map(|b| b.book_id)
        })
        .unwrap_or_else(|| candidate.to_owned())
}

/// The reader state at mount: the saved place, overridden by a jump
/// queued while the item was closed. Returns the verse to wash.
fn initial_state(item: ItemId, db: &Db) -> (BibleViewerState, Option<u16>) {
    let saved = crate::library::bodies::read_plugin_snapshot(item, KIND)
        .ok()
        .flatten()
        .and_then(|bytes| BibleSnapshot::decode(&bytes));
    let jump = crate::plugins::host::shell()
        .take_jump(KIND, crate::plugins::content::doc_id(item))
        .and_then(|bytes| BibleAnchor::decode(&bytes));
    let first_bible = db
        .list_bibles()
        .ok()
        .and_then(|bibles| bibles.into_iter().next())
        .map(|b| b.id)
        .unwrap_or_default();
    let mut state = BibleViewerState::new(first_bible.as_str(), "GEN", 1);
    match &saved {
        Some(snapshot) => snapshot.clone().apply_to(&mut state),
        None => state.current_book = canonical_book_id(db, &first_bible, "GEN"),
    }
    if let Some(jump) = &jump {
        state.bible_id.clone_from(&jump.bible_id);
        state.current_book = canonical_book_id(db, &jump.bible_id, &jump.book_id);
        state.current_chapter = jump.start_chapter;
    }
    if let Ok(chapter) = db.load_chapter(&state.bible_id, &state.current_book, state.current_chapter)
    {
        state.push_chapter(chapter);
    }
    (state, jump.map(|j| j.start_verse))
}

/// The reader's state and the hooks that keep it: jumps sent while it is
/// open, saving, and the lazy footnote fetch.
fn use_reader(item: ItemId, db: &Db) -> Reader {
    // The verse a queued jump asked for, read once the state exists.
    let start = use_hook(|| Rc::new(Cell::new(None::<u16>)));
    let state = use_state({
        let (db, start) = (db.clone(), Rc::clone(&start));
        move || {
            let (state, verse) = initial_state(item, &db);
            start.set(verse);
            state
        }
    });
    let flash = use_state(|| None);
    let scroll_to = use_state(|| None);
    let scroll = use_scroll_controller(ScrollConfig::default);
    let reader = Reader {
        item,
        db: db.clone(),
        state,
        flash,
        scroll_to,
        scroll,
    };
    use_hook({
        let reader = reader.clone();
        move || {
            if let Some(verse) = start.get() {
                reader.show(verse);
            }
            let jumps = crate::plugins::host::shell().jumps(KIND);
            let doc = crate::plugins::content::doc_id(item);
            spawn(async move {
                while let Ok(jump) = jumps.recv_async().await {
                    if jump.doc != doc {
                        continue;
                    }
                    crate::plugins::host::shell().take_jump(KIND, doc);
                    match BibleAnchor::decode(&jump.anchor) {
                        Some(anchor) => reader.follow(&anchor),
                        None => log::warn!("bible: undecodable jump"),
                    }
                }
            });
        }
    });
    use_side_effect(move || {
        let bytes = BibleSnapshot::from_state(&state.read()).encode();
        if let Err(e) = crate::library::bodies::write_plugin_snapshot(item, KIND, &bytes) {
            log::warn!("bible: snapshot write failed: {e}");
        }
        crate::sync::snapshot_changed(item);
    });
    // Footnotes arrive lazily per chapter; headings came with the
    // install. The fetch returns at once when the rows exist.
    use_side_effect({
        let db = db.clone();
        move || {
            let st = state.read();
            notein_bible::install_background::spawn_fetch_footnotes(
                db.clone(),
                st.bible_id.clone(),
                st.current_book.clone(),
                st.current_chapter,
            );
        }
    });
    reader
}

#[derive(Debug)]
struct BibleViewer {
    item: ItemId,
    slot: Option<ViewSlot>,
    db: Db,
}

impl PartialEq for BibleViewer {
    fn eq(&self, other: &Self) -> bool {
        self.item == other.item && self.slot == other.slot
    }
}

impl Component for BibleViewer {
    fn render(&self) -> impl IntoElement {
        let reader = use_reader(self.item, &self.db);
        let gesture = highlight::Gesture::use_gesture();
        let viewport_top = use_state(|| 0.0_f32);
        crate::plugins::faces::use_header_presses(KIND, self.item, self.slot, {
            let reader = reader.clone();
            move |press| match press.action.as_str() {
                "translation" => translation::open(&reader, press.at),
                "search" => search::open(&reader),
                _ => {}
            }
        });

        let st = reader.state.read();
        let chapter = st.current_chapter_data().cloned();
        let highlights: Vec<BibleBookmark> = st
            .highlights_on(&st.current_book, st.current_chapter)
            .into_iter()
            .cloned()
            .collect();
        let book_name = chapter
            .as_ref()
            .map_or_else(|| st.current_book.clone(), |c| c.book_name.clone());
        let current_chapter = st.current_chapter;
        let (has_back, has_forward) = (st.has_back(), st.has_forward());
        drop(st);

        let scroll_ctx = VerseScrollCtx {
            controller: reader.scroll,
            flash_verse: reader.flash,
            scroll_to: reader.scroll_to,
            viewport_top,
        };
        let body = chapter.as_ref().map_or_else(
            || {
                rect()
                    .expanded()
                    .center()
                    .child(label().color(palette().t2).text("Cargando capítulo…"))
                    .into_element()
            },
            |data| chapter_body(&reader, data, &highlights, gesture, scroll_ctx),
        );
        let on_back = has_back.then(|| {
            let reader = reader.clone();
            Box::new(move || reader.history(true)) as bottom_nav::NavHandler
        });
        let on_forward = has_forward.then(|| {
            let reader = reader.clone();
            Box::new(move || reader.history(false)) as bottom_nav::NavHandler
        });
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .content(Content::Flex)
            .child(rect().width(Size::fill()).height(Size::flex(1.)).child(body))
            .child(bottom_nav::bottom_nav(
                &book_name,
                current_chapter,
                bottom_nav::BottomNavCallbacks {
                    on_prev: {
                        let reader = reader.clone();
                        move || reader.step(false)
                    },
                    on_next: {
                        let reader = reader.clone();
                        move || reader.step(true)
                    },
                    on_open_picker: {
                        let reader = reader.clone();
                        move || nav::open_books(&reader)
                    },
                    on_back,
                    on_forward,
                },
            ))
    }
}

/// The chapter: verses with highlights, footnotes and cross-references.
fn chapter_body(
    reader: &Reader,
    data: &ChapterRow,
    highlights: &[BibleBookmark],
    gesture: highlight::Gesture,
    scroll_ctx: VerseScrollCtx,
) -> Element {
    let db = &reader.db;
    let show_cross_refs = crate::plugins::builtin::bible_kind()
        .is_none_or(|kind| plugin_config::effective_enable_crossref(kind.config()));
    let cross_refs = if show_cross_refs {
        cross_refs(db, data)
    } else {
        Vec::new()
    };
    let reading = reader::Reading {
        cross_refs: cross_refs_by_verse(&cross_refs),
        footnotes: footnotes_by_verse(&footnotes(db, data)),
        on_jump: EventHandler::new({
            let reader = reader.clone();
            move |(book, chapter, verse): (String, u16, u16)| reader.go(&book, chapter, verse)
        }),
        on_highlight: gesture.handler(reader),
        preview: gesture.preview(),
        font_size: 22.0,
        line_height: 1.65,
    };
    let highlights: Vec<&BibleBookmark> = highlights.iter().collect();
    reader::chapter_view(
        &plan_chapter(data),
        &highlights,
        &reading,
        *reader.flash.read(),
        Some(scroll_ctx),
    )
    .into_element()
}

/// Publisher footnotes of the chapter; one citing a verse ("Cited in
/// John 3:16") jumps there.
fn footnotes(db: &Db, data: &ChapterRow) -> Vec<FootnoteView> {
    let first_bible = db
        .list_bibles()
        .ok()
        .and_then(|bibles| bibles.into_iter().next())
        .map(|b| b.id);
    db.footnotes_for_chapter(&data.bible_id, &data.book_id, data.chapter)
        .unwrap_or_default()
        .into_iter()
        .map(|row| {
            let target = notein_bible::reference::parse(&row.text).and_then(|parsed| {
                let anchor =
                    notein_bible::resolver::resolve_parsed(db, first_bible.as_deref()?, &parsed)?;
                Some((anchor.book_id, anchor.start_chapter, anchor.start_verse))
            });
            FootnoteView {
                src_verse: row.verse,
                caller: row.caller,
                text: row.text,
                target,
            }
        })
        .collect()
}

/// Cross-references of the chapter, each with its target verse's text.
fn cross_refs(db: &Db, data: &ChapterRow) -> Vec<CrossRefView> {
    db.cross_refs_for_chapter(&data.bible_id, &data.book_id, data.chapter)
        .unwrap_or_default()
        .into_iter()
        .map(|row| {
            let tooltip = db
                .verse_text(&data.bible_id, &row.tgt_book, row.tgt_chap, row.tgt_verse)
                .ok()
                .flatten()
                .unwrap_or_default();
            CrossRefView {
                src_verse: row.src_verse,
                label: format!(
                    "{} {}:{}",
                    notein_bible::canon::short_name(&row.tgt_book),
                    row.tgt_chap,
                    row.tgt_verse
                ),
                tooltip,
                tgt_book: row.tgt_book,
                tgt_chap: row.tgt_chap,
                tgt_verse: row.tgt_verse,
            }
        })
        .collect()
}
