//! The reader of a Bible document.
//!
//! Highlights and the last place read live in the document's content,
//! so export and sync carry them; the plugin database only holds the
//! installed text and its search index.

mod highlight;
mod nav;
mod presence;
mod refs;
mod search;
mod shared;
mod translation;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use freya::components::{ScrollConfig, ScrollController, use_scroll_controller};
use freya::prelude::*;
use notein_sdk::doc::DocId;
use notein_sdk::face::{ContentStore, FaceContext, HeaderAction};
use notein_sdk::services::Host;
use notein_sdk::ui::Ui;
use notein_ui_kit::theme::palette;

use super::bottom_nav;
use super::chapter::plan_chapter;
use super::reader::{
    self, CrossRefView, FootnoteView, ScrollTarget, VerseScrollCtx, cross_refs_by_verse,
    footnotes_by_verse,
};
use crate::BibleAnchor;
use crate::db::{ChapterRow, Db};
use crate::kind::{BibleContent, BibleKind, KIND};
use crate::plugin_config;
use crate::state::{BibleBookmark, BibleSnapshot, BibleViewerState};

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

/// The reader of one document; the first-use card until a translation
/// is installed.
pub(crate) struct BiblePage {
    pub(crate) ctx: FaceContext<BibleKind>,
}

impl PartialEq for BiblePage {
    fn eq(&self, other: &Self) -> bool {
        self.ctx.doc == other.ctx.doc && self.ctx.pane == other.ctx.pane
    }
}

impl Component for BiblePage {
    fn render(&self) -> impl IntoElement {
        // Re-render when an install finishes, so the first-use card
        // gives way to the reader.
        let _installs = *super::catalog::use_installs().read();
        let db = self.ctx.kind().db().clone();
        let inner = if db.list_bibles().unwrap_or_default().is_empty() {
            FirstUseFace {
                ctx: self.ctx.clone(),
            }
            .into_element()
        } else {
            BibleViewer {
                ctx: self.ctx.clone(),
                db,
            }
            .into_element()
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
struct FirstUseFace {
    ctx: FaceContext<BibleKind>,
}

impl PartialEq for FirstUseFace {
    fn eq(&self, other: &Self) -> bool {
        self.ctx.doc == other.ctx.doc && self.ctx.pane == other.ctx.pane
    }
}

impl Component for FirstUseFace {
    fn render(&self) -> impl IntoElement {
        self.ctx.use_header_presses(|_| super::catalog::open());
        super::catalog::FirstUse
    }
}

/// Handles the parts of the reader share.
#[derive(Clone)]
pub(super) struct Reader {
    doc: DocId,
    host: Host,
    ui: Ui,
    kind: Arc<BibleKind>,
    db: Db,
    state: State<BibleViewerState>,
    flash: State<Option<u16>>,
    scroll_to: State<Option<ScrollTarget>>,
    scroll: ScrollController,
    /// The cross-references showing in the panel beside the text.
    refs: State<Option<refs::VerseRefs>>,
    /// What the current chapter shows besides its text, read once.
    annotations: Rc<std::cell::RefCell<Option<(AnnotationKey, Annotations)>>>,
    /// Bumped when footnotes of a chapter arrive.
    footnotes_landed: State<u32>,
}

impl PartialEq for Reader {
    fn eq(&self, other: &Self) -> bool {
        self.doc == other.doc
    }
}

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reader")
            .field("doc", &self.doc)
            .finish_non_exhaustive()
    }
}

impl Reader {
    fn bible_id(&self) -> String {
        self.state.peek().bible_id.clone()
    }

    /// Go to `book` `chapter`, bring `verse` into view and wash it.
    pub(super) fn go(&self, book: &str, chapter: u16, verse: u16) {
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

/// Point `state` at a place that exists. A Bible opened before any
/// translation was installed saved an empty translation and book; one
/// whose translation was removed names one that is gone. Either way the
/// reader starts at the first chapter of `fallback_bible`.
fn settle_place(db: &Db, state: &mut BibleViewerState, fallback_bible: &str) {
    let installed = db.list_bibles().unwrap_or_default();
    if !installed.iter().any(|b| b.id == state.bible_id) {
        fallback_bible.clone_into(&mut state.bible_id);
        state.current_book.clear();
        state.current_chapter = 1;
    }
    let books = db.list_books(&state.bible_id).unwrap_or_default();
    if let Some(book) = books
        .iter()
        .find(|b| b.book_id.eq_ignore_ascii_case(&state.current_book))
    {
        book.book_id.clone_into(&mut state.current_book);
    } else {
        if let Some(first) = books.first() {
            first.book_id.clone_into(&mut state.current_book);
        }
        state.current_chapter = 1;
    }
    let chapters = db
        .chapter_count(&state.bible_id, &state.current_book)
        .unwrap_or(1);
    if state.current_chapter == 0 || state.current_chapter > chapters {
        state.current_chapter = 1;
    }
}

/// The reader state at mount: the saved place, overridden by a jump
/// queued while the document was closed. Returns the verse to wash.
fn initial_state(ctx: &FaceContext<BibleKind>, db: &Db) -> (BibleViewerState, Option<u16>) {
    let saved: BibleContent = ctx.store.load();
    let jump = ctx
        .host
        .take_jump(KIND, ctx.doc)
        .and_then(|bytes| BibleAnchor::decode(&bytes));
    let first_bible = db
        .list_bibles()
        .ok()
        .and_then(|bibles| bibles.into_iter().next())
        .map(|b| b.id)
        .unwrap_or_default();
    let mut state = BibleViewerState::new(first_bible.as_str(), "GEN", 1);
    saved.apply_to(&mut state);
    if let Some(jump) = &jump {
        state.bible_id.clone_from(&jump.bible_id);
        state.current_book = canonical_book_id(db, &jump.bible_id, &jump.book_id);
        state.current_chapter = jump.start_chapter;
    }
    settle_place(db, &mut state, &first_bible);
    if let Ok(chapter) =
        db.load_chapter(&state.bible_id, &state.current_book, state.current_chapter)
    {
        state.push_chapter(chapter);
    }
    (state, jump.map(|j| j.start_verse))
}

/// The reader's state and the hooks that keep it: jumps sent while it is
/// open, saving, and the lazy footnote fetch.
fn use_reader(ctx: &FaceContext<BibleKind>, db: &Db) -> Reader {
    // The verse a queued jump asked for, read once the state exists.
    let start = use_hook(|| Rc::new(Cell::new(None::<u16>)));
    let state = use_state({
        let (ctx, db, start) = (ctx.clone(), db.clone(), Rc::clone(&start));
        move || {
            let (state, verse) = initial_state(&ctx, &db);
            start.set(verse);
            state
        }
    });
    let records = use_hook({
        let (host, doc) = (Arc::clone(&ctx.host), ctx.doc);
        move || host.records(doc)
    });
    shared::use_shared_highlights(&records, state);
    let reader = Reader {
        doc: ctx.doc,
        host: Arc::clone(&ctx.host),
        ui: Rc::clone(&ctx.ui),
        kind: ctx.kind_handle(),
        db: db.clone(),
        state,
        flash: use_state(|| None),
        scroll_to: use_state(|| None),
        scroll: use_scroll_controller(ScrollConfig::default),
        refs: use_state(|| None),
        annotations: use_hook(Rc::default),
        footnotes_landed: use_state(|| 0),
    };
    use_hook({
        let reader = reader.clone();
        move || {
            if let Some(verse) = start.get() {
                reader.show(verse);
            }
        }
    });
    ctx.use_jumps({
        let reader = reader.clone();
        move |anchor: BibleAnchor| reader.follow(&anchor)
    });
    use_side_effect({
        let store: ContentStore<BibleContent> = ctx.store.clone();
        move || store.save(&BibleSnapshot::from_state(&state.read()))
    });
    // Content written elsewhere: another device, or the translation
    // picked in the edit sheet.
    use_hook({
        let (store, db) = (ctx.store.clone(), db.clone());
        move || {
            let changes = store.changes();
            let mut state = state;
            spawn(async move {
                while changes.recv_async().await.is_ok() {
                    let saved = store.load();
                    // Highlights merge as records; where someone else reads is
                    // theirs. Only another translation (the edit sheet) moves
                    // this reader.
                    if saved.bible_id == state.peek().bible_id {
                        continue;
                    }
                    let mut st = state.write();
                    let highlights = std::mem::take(&mut st.highlights);
                    saved.apply_to(&mut st);
                    st.highlights = highlights;
                    let fallback = st.bible_id.clone();
                    settle_place(&db, &mut st, &fallback);
                    if let Err(e) = st.ensure_current_loaded(&db) {
                        log::warn!("bible: reload after a change: {e}");
                    }
                }
            });
        }
    });
    // Footnotes arrive lazily per chapter; headings came with the
    // install. The fetch returns at once when the rows exist.
    use_side_effect({
        let db = db.clone();
        let landed = use_hook(|| {
            let (tx, rx) = flume::unbounded::<()>();
            let mut count = reader.footnotes_landed;
            spawn(async move {
                while rx.recv_async().await.is_ok() {
                    *count.write() += 1;
                }
            });
            tx
        });
        move || {
            let st = state.read();
            let landed = landed.clone();
            crate::install_background::spawn_fetch_footnotes(
                db.clone(),
                st.bible_id.clone(),
                st.current_book.clone(),
                st.current_chapter,
                move || {
                    let _ = landed.send(());
                },
            );
        }
    });
    reader
}

struct BibleViewer {
    ctx: FaceContext<BibleKind>,
    db: Db,
}

impl PartialEq for BibleViewer {
    fn eq(&self, other: &Self) -> bool {
        self.ctx.doc == other.ctx.doc && self.ctx.pane == other.ctx.pane
    }
}

impl Component for BibleViewer {
    fn render(&self) -> impl IntoElement {
        let reader = use_reader(&self.ctx, &self.db);
        let gesture = highlight::Gesture::use_gesture();
        gesture.use_end_with_reader(&reader);
        let others = presence::use_others(&reader, gesture);
        let viewport_top = use_state(|| 0.0_f32);
        let mut width = use_state(|| 0.0_f32);
        let wide = *width.read() >= refs::ROOM_FOR_PANEL;
        self.ctx.use_header_presses({
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
            |data| {
                chapter_body(
                    &reader,
                    data,
                    &highlights,
                    gesture,
                    others,
                    scroll_ctx,
                    wide,
                )
            },
        );
        let on_back = has_back.then(|| {
            let reader = reader.clone();
            Box::new(move || reader.history(true)) as bottom_nav::NavHandler
        });
        let on_forward = has_forward.then(|| {
            let reader = reader.clone();
            Box::new(move || reader.history(false)) as bottom_nav::NavHandler
        });
        let column = rect()
            .width(Size::flex(1.))
            .height(Size::fill())
            .content(Content::Flex)
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .child(body),
            )
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
            ));
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .on_sized(move |e: Event<SizedEventData>| width.set_if_modified(e.area.width()))
            .child(column)
            .child(refs::Panel { reader })
    }
}

/// The chapter: verses with highlights, footnotes and cross-references.
fn chapter_body(
    reader: &Reader,
    data: &ChapterRow,
    highlights: &[BibleBookmark],
    gesture: highlight::Gesture,
    others: Rc<Vec<reader::Other>>,
    scroll_ctx: VerseScrollCtx,
    wide: bool,
) -> Element {
    let annotations = reader.annotations(data);
    let reading = reader::Reading {
        cross_refs: annotations.cross_refs,
        footnotes: annotations.footnotes,
        on_jump: EventHandler::new({
            let reader = reader.clone();
            move |(book, chapter, verse): (String, u16, u16)| reader.go(&book, chapter, verse)
        }),
        on_refs: EventHandler::new({
            let reader = reader.clone();
            move |verse: u16| refs::open(&reader, verse, wide)
        }),
        refs_open: reader
            .refs
            .read()
            .as_ref()
            .filter(|refs| refs.here(reader))
            .map(|refs| refs.verse),
        on_highlight: gesture.handler(reader),
        on_tap: gesture.tap_handler(reader),
        preview: gesture.preview(),
        selected: gesture.selected(reader),
        others,
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
            let target = crate::reference::parse(&row.text).and_then(|parsed| {
                let anchor = crate::resolver::resolve_parsed(db, first_bible.as_deref()?, &parsed)?;
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

/// Cross-references of the chapter; the panel reads their text.
fn cross_refs(db: &Db, data: &ChapterRow) -> Vec<CrossRefView> {
    db.cross_refs_for_chapter(&data.bible_id, &data.book_id, data.chapter)
        .unwrap_or_default()
        .into_iter()
        .map(|row| CrossRefView {
            src_verse: row.src_verse,
            label: format!(
                "{} {}:{}",
                crate::canon::short_name(&row.tgt_book),
                row.tgt_chap,
                row.tgt_verse
            ),
            tgt_book: row.tgt_book,
            tgt_chap: row.tgt_chap,
            tgt_verse: row.tgt_verse,
        })
        .collect()
}

/// Which chapter, as shown, a set of [`Annotations`] belongs to.
#[derive(Clone, PartialEq, Eq)]
struct AnnotationKey {
    bible: String,
    book: String,
    chapter: u16,
    cross_refs: bool,
    footnotes_landed: u32,
}

/// What a chapter shows besides its text, by verse.
#[derive(Clone)]
struct Annotations {
    cross_refs: Rc<std::collections::HashMap<u16, Vec<CrossRefView>>>,
    footnotes: Rc<std::collections::HashMap<u16, Vec<FootnoteView>>>,
}

impl Reader {
    /// The annotations of `data`, read from the database only when the
    /// chapter, the settings or its footnotes changed: the reader renders
    /// again on every flash, colour tried and panel toggle.
    fn annotations(&self, data: &ChapterRow) -> Annotations {
        let show_cross_refs = plugin_config::effective_enable_crossref(self.kind.config());
        let key = AnnotationKey {
            bible: data.bible_id.clone(),
            book: data.book_id.clone(),
            chapter: data.chapter,
            cross_refs: show_cross_refs,
            footnotes_landed: *self.footnotes_landed.read(),
        };
        if let Some((held, annotations)) = self.annotations.borrow().as_ref()
            && *held == key
        {
            return annotations.clone();
        }
        let cross_refs = if show_cross_refs {
            cross_refs(&self.db, data)
        } else {
            Vec::new()
        };
        let annotations = Annotations {
            cross_refs: Rc::new(cross_refs_by_verse(&cross_refs)),
            footnotes: Rc::new(footnotes_by_verse(&footnotes(&self.db, data))),
        };
        *self.annotations.borrow_mut() = Some((key, annotations.clone()));
        annotations
    }
}
