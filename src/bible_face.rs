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
    let mut search_query: State<String> = use_state(String::new);
    let mut search_hits: State<Vec<notein_bible::ui::search_sheet::SearchHitView>> =
        use_state(Vec::new);
    // Bookmark gesture target — hosts the colour-picker overlay. A
    // double-tap / long-press on one verse marker produces `Verse(n)`;
    // a drag between two markers produces `Range(lo, hi)` so the
    // picker applies the chosen colour across the whole span.
    let mut pending_highlight: State<Option<notein_bible::ui::reader::HighlightTarget>> =
        use_state(|| None);
    // Live preview tracks the ColorWheel's current colour so the
    // paragraph updates in real time as the user drags. Commit happens
    // only on explicit "Guardar"; Cancel discards.
    let mut preview_color: State<Option<freya::prelude::Color>> = use_state(|| None);
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
                bid.clone(),
                book.clone(),
                chap,
            );
            // Translations without inline section headings (e.g.
            // RV1960 via biblionlabs) borrow them from a sibling
            // helloao id — same chapter JSON cache is reused so the
            // enrichment adds one HTTP call max, then zero after
            // first hit.
            notein_bible::install_background::spawn_fetch_headings(
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

    // Book + chapter picker sheet state. `nav_sheet_open` toggles the
    // sheet from the bottom-nav tap; `nav_sheet_page` is the SheetPager
    // index (0 = book grid, 1 = chapter grid); `nav_sheet_book` keeps
    // the book_id the user drilled into so page 1 renders chapter cells
    // without a parent re-render.
    let mut nav_sheet_open = use_state(|| false);
    let mut nav_sheet_page = use_state(|| 0usize);
    let mut nav_sheet_book: State<Option<String>> = use_state(|| None);
    let open_nav_sheet = move || {
        nav_sheet_page.set(0);
        nav_sheet_book.set(None);
        nav_sheet_open.set(true);
    };
    // Inline FTS search driven by the toolbar's expandable
    // FormInput. Fires on every keystroke OR suggestion pick. We
    // distinguish by exact-matching the incoming string against the
    // current hits' labels — a match means "picked a suggestion",
    // anything else is a fresh filter query.
    let db_search = db.clone();
    let bible_for_search = st_snapshot.bible_id.clone();
    let books_for_search = db.list_books(&bible_for_search).unwrap_or_default();
    let db_jump = db.clone();
    let on_search = EventHandler::<String>::new(move |query: String| {
        // Pick path: query exactly matches an existing hit's label →
        // jump + reset.
        let hits_snapshot = search_hits.read().clone();
        if let Some(view) = hits_snapshot.iter().find(|v| v.suggestion_label == query) {
            let src = view.source.clone();
            if let Err(e) = state.write().jump_to(&db_jump, &src.book_id, src.chapter) {
                log::warn!("bible search jump: {e}");
            }
            flash_verse.set(Some(src.verse));
            let mut flash_verse = flash_verse;
            spawn_forever(async move {
                async_io::Timer::after(Duration::from_secs(3)).await;
                flash_verse.set(None);
            });
            search_hits.set(Vec::new());
            search_query.set(String::new());
            return;
        }
        // Filter path.
        if query.trim().is_empty() {
            search_hits.set(Vec::new());
            return;
        }
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
    });

    // Snapshot of the current hits as suggestion labels. The
    // FormInput filters this list client-side against the typed
    // query, so even if the FTS lag is a few frames the dropdown
    // reflects the latest query.
    let search_suggestions: Vec<std::borrow::Cow<'static, str>> = search_hits
        .read()
        .iter()
        .map(|v| std::borrow::Cow::Owned(v.suggestion_label.clone()))
        .collect();

    // Read the DocLinked chrome extension so the Bible toolbar can
    // surface split H/V (overlay mode) or close-split (split mode) in
    // sync with the shell. Standalone `BibleView` mounts see `None` and
    // the toolbar renders without split buttons.
    let doc_linked_chrome = try_consume_context::<crate::doc_linked::DocLinkedChromeExt>();
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
                .layer(Layer::Overlay)
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
                on_split_horizontal: doc_linked_chrome.as_ref().filter(|c| !c.is_split).map(|c| {
                    let cb = c.on_split_horizontal.clone();
                    Box::new(move || cb.call()) as toolbar::BackHandler
                }),
                on_split_vertical: doc_linked_chrome.as_ref().filter(|c| !c.is_split).map(|c| {
                    let cb = c.on_split_vertical.clone();
                    Box::new(move || cb.call()) as toolbar::BackHandler
                }),
                on_close_split: doc_linked_chrome.as_ref().filter(|c| c.is_split).map(|c| {
                    let cb = c.on_close.clone();
                    Box::new(move || cb.call()) as toolbar::BackHandler
                }),
                search_query: Writable::<String>::from(search_query),
                on_search,
                search_suggestions,
                on_translation: translation_cb,
            },
        ))
        .child(chapter_body(
            &db,
            st_snapshot.chapter.as_ref(),
            &st_snapshot.highlights,
            *flash_verse.read(),
            {
                let pending = *pending_highlight.read();
                let color = *preview_color.read();
                match (pending, color) {
                    (Some(t), Some(c)) => {
                        use notein_bible::ui::reader::{HighlightKind, HighlightPreview};
                        let (lo, hi) = match t.kind {
                            HighlightKind::Verse(v) => (v, v),
                            HighlightKind::Range(a, b) => (a, b),
                        };
                        Some(HighlightPreview { lo, hi, color: c })
                    }
                    _ => None,
                }
            },
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
            move |target: notein_bible::ui::reader::HighlightTarget| {
                // Delegate the full bookmark UX to the shell. The
                // plugin only owns the paint + the (book, chapter,
                // range) anchor; everything else — body + colour
                // picker + autocomplete chips — comes from
                // `host.request_bookmark_edit`, which returns the
                // edited draft when the user commits.
                use notein_bible::ui::reader::HighlightKind;
                let (lo, hi) = match target.kind {
                    HighlightKind::Verse(v) => (v, v),
                    HighlightKind::Range(a, b) => (a, b),
                };
                // Flag the preview as active for THIS range —
                // chapter_body only paints the live-preview overlay
                // when both `pending_highlight` and `preview_color`
                // are set. Cleared on commit / cancel below.
                pending_highlight.set(Some(target));
                let book = {
                    let st = state.read();
                    st.current_book.clone()
                };
                let chapter_num = {
                    let st = state.read();
                    st.current_chapter
                };
                // Seed the modal with the existing bookmark's colour +
                // note if the target range overlaps one so editing
                // never clobbers prior data.
                let existing = {
                    let st = state.read();
                    st.highlights_on(&book, chapter_num)
                        .into_iter()
                        .find(|h| h.start_verse <= hi && h.end_verse >= lo)
                        .cloned()
                };
                let draft = notein_plugin::BookmarkDraft {
                    body: existing.as_ref().map(|h| h.note.clone()).unwrap_or_default(),
                    color: existing.as_ref().and_then(|h| h.color).map(|c| {
                        notein_bible::state::u32_to_rgba(c)
                    }),
                    refs: Vec::new(),
                };
                let title = if lo == hi {
                    format!("Resaltar verso {lo}")
                } else {
                    format!("Resaltar versos {lo}–{hi}")
                };
                // Shell callback crosses threads via a `Send + Sync`
                // bound; Freya `State` isn't Send so we forward
                // commit result + preview updates through flume
                // channels and apply them from UI-thread listeners.
                let host = crate::host_api::host_handle();
                let book_cb = book.clone();
                let (tx, rx) = flume::bounded::<Option<notein_plugin::BookmarkDraft>>(1);
                spawn(async move {
                    if let Ok(Some(draft)) = rx.recv_async().await {
                        let packed = draft
                            .color
                            .map(notein_bible::state::rgba_to_u32);
                        let mut st = state.write();
                        st.upsert_highlight(
                            &book_cb,
                            chapter_num,
                            lo,
                            hi,
                            packed,
                            draft.body,
                        );
                    }
                    // Preview is per-session; drop it on commit AND
                    // on cancel (recv returning Ok(None)).
                    preview_color.set(None);
                    pending_highlight.set(None);
                });
                // Preview sink mirrors wheel drags into the plugin's
                // live-preview state so the paragraph bg rect tracks
                // the user's choice before the Save button fires.
                let (preview_tx, preview_rx) =
                    flume::unbounded::<notein_plugin::BookmarkDraft>();
                spawn_forever(async move {
                    while let Ok(draft) = preview_rx.recv_async().await {
                        if let Some(rgba) = draft.color {
                            preview_color.set(Some(freya::prelude::Color::from_rgb(
                                rgba[0], rgba[1], rgba[2],
                            )));
                        }
                    }
                });
                let request = notein_plugin::BookmarkEditRequest {
                    draft,
                    title: title.into(),
                    anchor: Some((target.anchor_x, target.anchor_y)),
                    on_preview: Some(std::sync::Arc::new(move |draft| {
                        let _ = preview_tx.send(draft);
                    })),
                };
                host.request_bookmark_edit(
                    BIBLE_PLUGIN_ID,
                    request,
                    std::sync::Arc::new(move |result| {
                        let _ = tx.send(result);
                    }),
                );
            },
        ))
        .child(bottom_nav::bottom_nav(
            &st_snapshot.book_name,
            st_snapshot.current_chapter,
            bottom_nav::BottomNavCallbacks {
                on_prev,
                on_next,
                on_open_picker: open_nav_sheet,
            },
        ));
    if let Some(overlay) = picker_overlay {
        column = column.child(overlay);
    }
    // Search results live inside the toolbar FormInput as its
    // suggestion dropdown now; the standalone results popup was
    // dropped — picking a suggestion dispatches the jump via
    // `on_search` matching the hit label.

    // Bookmark edit UX now lives shell-side (host_overlay's
    // `bookmark_edit_modal`), driven through
    // `host.request_bookmark_edit`. The plugin's job is reduced to
    // emitting the gesture + persisting the final draft.

    // Book + chapter picker sheet. Opens from the bottom-nav caption,
    // slides a `BookGrid` → (horizontal slide) → `ChapterGrid` for the
    // selected book. Picking a chapter navigates the viewer + closes
    // the sheet.
    if *nav_sheet_open.read() {
        let bible_id_sheet = st_snapshot.bible_id.clone();
        // Publisher order_num reflects the importer's alphabetical
        // sort, not the canon. Re-sort by the canonical OSIS table so
        // the grid always runs Genesis → Revelation, regardless of
        // which translation is installed.
        let mut books = db.list_books(&bible_id_sheet).unwrap_or_default();
        books.sort_by_key(|b| notein_bible::canon::canonical_order(&b.book_id));
        let selected_book = nav_sheet_book.read().clone();

        let book_page = book_grid_page(books.clone(), {
            let mut nav_sheet_book = nav_sheet_book;
            let mut nav_sheet_page = nav_sheet_page;
            move |book_id: String| {
                nav_sheet_book.set(Some(book_id));
                nav_sheet_page.set(1);
            }
        });

        let chapter_page = match selected_book.as_ref() {
            Some(book_id) => {
                let count = db.chapter_count(&bible_id_sheet, book_id).unwrap_or(1);
                let name = books
                    .iter()
                    .find(|b| b.book_id == *book_id)
                    .map(|b| b.name.clone())
                    .unwrap_or_else(|| book_id.clone());
                let db_nav = db.clone();
                let book_for_nav = book_id.clone();
                chapter_grid_page(
                    &name,
                    count,
                    {
                        let mut nav_sheet_page = nav_sheet_page;
                        move |_: Event<PressEventData>| nav_sheet_page.set(0)
                    },
                    move |chapter: u16| {
                        if let Err(e) = state.write().jump_to(&db_nav, &book_for_nav, chapter) {
                            log::warn!("bible sheet nav: {e}");
                        }
                        nav_sheet_open.set(false);
                    },
                )
                .into_element()
            }
            None => rect().into_element(),
        };

        column = column.child(
            notein_ui_kit::BottomSheet::new(notein_ui_kit::SheetPager::new(
                nav_sheet_page,
                vec![book_page.into_element(), chapter_page],
            ))
            .open(nav_sheet_open)
            .height(460.0),
        );
    }

    column
}

/// Grid of every book in the current Bible. Clicking a cell drills
/// into the matching chapter grid in page 1 of the sheet.
fn book_grid_page(
    books: Vec<notein_bible::db::BookRow>,
    on_pick: impl FnMut(String) + Clone + 'static,
) -> impl IntoElement {
    const CELL: f32 = 72.0;
    // Vertical ScrollView over a horizontal wrap-row of cells. The row
    // flows horizontally with `Content::wrap_spacing` so cells wrap at
    // the container's edge; the ScrollView only kicks in once the
    // wrapped rows overflow its height. Without the wrapper the row
    // just clipped at the viewport bottom.
    let mut row = rect()
        .width(Size::fill())
        .horizontal()
        .spacing(8.0)
        .content(Content::wrap_spacing(8.));
    for b in books {
        let book_id = b.book_id.clone();
        let mut on_pick = on_pick.clone();
        row = row.child(
            rect()
                .width(Size::px(CELL))
                .height(Size::px(CELL))
                .center()
                .with_corner_radius(10.0)
                .background(Color::from_rgb(38, 38, 42))
                .on_press(move |_: Event<PressEventData>| on_pick(book_id.clone()))
                .child(
                    label()
                        .font_size(13.0)
                        .color(Color::from_rgb(230, 230, 230))
                        .text(b.abbrev.clone()),
                ),
        );
    }
    rect()
        .expanded()
        .padding((0., 4.))
        .child(ScrollView::new().child(row))
}

/// Grid of chapter numbers for the drilled-in book, with a back chip
/// at the top that returns to the book grid.
fn chapter_grid_page(
    book_name: &str,
    count: u16,
    on_back: impl FnMut(Event<PressEventData>) + 'static,
    on_pick: impl FnMut(u16) + Clone + 'static,
) -> impl IntoElement {
    const CELL: f32 = 48.0;
    const COLS: usize = 7;
    let header = rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .spacing(10.0)
        .child(
            rect()
                .padding((4., 10.))
                .with_corner_radius(6.0)
                .background(Color::from_rgb(55, 60, 70))
                .on_press(on_back)
                .child(
                    label()
                        .font_size(12.0)
                        .color(Color::from_rgb(230, 230, 230))
                        .text("‹ Libros"),
                ),
        )
        .child(
            label()
                .font_size(14.0)
                .color(Color::from_rgb(230, 230, 230))
                .text(book_name.to_string()),
        );

    let mut grid = rect().width(Size::fill()).spacing(8.0);
    let mut chapter: u16 = 1;
    while chapter <= count {
        let mut row = rect()
            .direction(Direction::Horizontal)
            .spacing(8.0)
            .width(Size::fill());
        let row_end = (chapter as usize + COLS).min(count as usize + 1);
        for c in (chapter as usize)..row_end {
            let c_u16 = c as u16;
            let mut on_pick = on_pick.clone();
            row = row.child(
                rect()
                    .width(Size::px(CELL))
                    .height(Size::px(CELL))
                    .center()
                    .with_corner_radius(8.0)
                    .background(Color::from_rgb(38, 38, 42))
                    .on_press(move |_: Event<PressEventData>| on_pick(c_u16))
                    .child(
                        label()
                            .font_size(13.0)
                            .color(Color::from_rgb(230, 230, 230))
                            .text(format!("{c_u16}")),
                    ),
            );
        }
        grid = grid.child(row);
        chapter = row_end as u16;
    }

    rect()
        .width(Size::fill())
        .spacing(10.0)
        .child(header)
        .child(ScrollView::new().child(grid))
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
    preview: Option<notein_bible::ui::reader::HighlightPreview>,
    on_jump: impl FnMut((String, u16, u16)) + 'static,
    on_highlight: impl FnMut(notein_bible::ui::reader::HighlightTarget) + 'static,
) -> impl IntoElement {
    let base = rect().width(Size::fill()).height(Size::flex(1.));
    match chapter {
        Some(data) => {
            let plan = plan_chapter(data);
            let hl_refs: Vec<&BibleBookmark> = highlights.iter().collect();
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
            let jump_handler = EventHandler::<(String, u16, u16)>::new(on_jump);
            let highlight_handler =
                EventHandler::<notein_bible::ui::reader::HighlightTarget>::new(on_highlight);
            base.child(reader::chapter_view(
                &plan,
                &hl_refs,
                &refs_map,
                &footnotes_map,
                jump_handler,
                highlight_handler,
                flash_verse,
                preview,
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
