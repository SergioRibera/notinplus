//! Bible reader page bound to a library item.
//!
//! The item carries a [`crate::library::DocKind`] with `plugin_id =
//! "io.notein.bible.viewer"` + `init_bytes` holding the chosen
//! translation id. Highlights + the user's last-read coordinates live
//! in a notein-managed snapshot sidecar
//! ([`crate::library::bodies::plugin_snapshot_path`]) that the shell
//! writes via `CanvasKind::serialize_snapshot`. This keeps the Bible
//! plugin's own SQLite focused on the installed text + search index
//! only — user-authored data flows through notein's generic storage
//! so bundle export + Phase 10 CRDT sync pick it up for free.

use std::sync::Arc;

use freya::icons::lucide::arrow_left;
use freya::prelude::*;

use notein_bible::db::{BibleRow, ChapterRow, Db};
use notein_bible::plugin_config;
use notein_bible::state::{BibleBookmark, BibleSnapshot, BibleViewerState};
use notein_bible::ui::chapter::plan_chapter;
use notein_bible::ui::reader::{
    CrossRefView, FootnoteView, cross_refs_by_verse, footnotes_by_verse,
};
use notein_bible::ui::{bottom_nav, reader, toolbar, translation_switcher};

use std::time::Duration;

use crate::library::ItemId;

const BIBLE_PLUGIN_ID: &str = notein_bible::kind::KIND;

#[derive(Debug, PartialEq)]
pub struct BibleFace {
    pub item: ItemId,
}

impl Component for BibleFace {
    fn render(&self) -> impl IntoElement {
        let item = self.item;
        // Subscribe to the install-modal's epoch so an install
        // finishing (or a user picking a different installed Bible)
        // forces this scope to re-render — otherwise the empty-state
        // branch would stay even after a background install lands.
        let _installs = *crate::screens::bible::use_installs().read();
        let inner: Element = match Db::open_default() {
            Err(e) => placeholder(format!("No se pudo abrir la DB: {e}")).into_element(),
            Ok(db) => {
                let bibles = db.list_bibles().unwrap_or_default();
                if bibles.is_empty() {
                    crate::screens::bible::FirstUse.into_element()
                } else {
                    // Delegate to a dedicated component so the viewer's
                    // many hooks are tracked under its own scope. When
                    // the empty-state branch flips to the viewer branch
                    // post-install, mounting a fresh component is safe;
                    // calling hooks inline from this `match` arm is NOT
                    // (freya enforces identical hook order per render).
                    BibleViewer { item }.into_element()
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

#[derive(Debug, PartialEq)]
struct BibleViewer {
    item: ItemId,
}

impl Component for BibleViewer {
    fn render(&self) -> impl IntoElement {
        let item = self.item;
        // Reuse the plugin-owned DB handle so we don't re-run schema
        // migrations on every render. Fall back to a fresh open only
        // if the registry isn't wired yet (should never happen inside
        // a mounted viewer, but keeps the match total).
        let db = crate::builtin::bible_canvas()
            .map(|c| c.db().clone())
            .or_else(|| Db::open_default().ok())
            .expect("bible db available when viewer mounts");
        let bibles = db.list_bibles().unwrap_or_default();
        bible_viewer_body(item, db, bibles)
    }
}

fn bible_viewer_body(item: ItemId, db: Db, bibles: Vec<BibleRow>) -> Element {
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

    // Jump queued on the host (chip tap, panel, search). Pull
    // it once at init so a re-render inside the same mount doesn't
    // re-trigger the jump. Captured by the init closure below (which
    // Freya runs exactly once per mount) and stashed in
    // `initial_flash_verse` for the flash hook further down.
    let db_for_init = db.clone();
    let snap_for_init = saved_snapshot.clone();
    let (mut state, initial_flash_verse) = {
        let jump_cell: State<Option<u16>> = use_state(|| None);
        let mut jump_cell_setter = jump_cell;
        let state = use_state(move || {
            let initial_jump = notein_plugin::services::HostServices::take_jump(
                &*crate::plugins::host::shell(),
                BIBLE_PLUGIN_ID,
                crate::plugins::content::doc_id(item),
            )
                .and_then(|bytes| notein_bible::BibleAnchor::decode(&bytes));
            jump_cell_setter.set(initial_jump.as_ref().map(|j| j.start_verse));
            let mut st = BibleViewerState::new(&starting_bible, &starting_book, starting_chapter);
            if let Some(snap) = snap_for_init {
                snap.apply_to(&mut st);
            }
            if let Some(jump) = initial_jump.as_ref() {
                // Chip-tap landed on this mount — override the restored
                // snapshot so the viewer opens straight at the
                // requested verse. Canonicalise `book_id` against the
                // installed books catalogue: stored anchors may carry
                // the uppercase OSIS form while a biblionlabs install
                // keys its `books` table in lowercase (and vice
                // versa) — mismatch would surface as `MissingBible`
                // down inside `load_chapter`.
                st.bible_id = jump.bible_id.clone();
                st.current_book = canonicalise_book_id(&db_for_init, &jump.bible_id, &jump.book_id);
                st.current_chapter = jump.start_chapter;
            }
            if let Ok(chap) =
                db_for_init.load_chapter(&st.bible_id, &st.current_book, st.current_chapter)
            {
                st.push_chapter(chap);
            }
            st
        });
        (state, *jump_cell.read())
    };
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
    let preview_color: State<Option<freya::prelude::Color>> = use_state(|| None);
    // Transient highlight on the destination verse of a TSK / xref jump
    // — set on click, cleared by a spawned timer after a few seconds so
    // the user eye lands on the referenced text before the colour fades.
    // Chip-tap jumps seed the flash via `initial_flash_verse` so the
    // chip's target verse pulses just like a TSK click would.
    let mut flash_verse: State<Option<u16>> = use_state(|| initial_flash_verse);
    // Scroll-into-view wiring. The controller drives the chapter
    // `ScrollView`; `viewport_top` carries the window-coord top of
    // the chapter container so inline `VerseMarker::on_sized` can
    // compute a content-relative scroll target. Both signals + the
    // controller travel together via `VerseScrollCtx`.
    let scroll_controller =
        freya::components::use_scroll_controller(freya::components::ScrollConfig::default);
    let viewport_top: State<f32> = use_state(|| 0.0_f32);
    // Verse to bring into view, separate from the 3 s flash so the scroll
    // happens once and then leaves the user in control.
    let scroll_to =
        use_state(|| initial_flash_verse.map(notein_bible::ui::reader::ScrollTarget::new));
    let verse_scroll_ctx = notein_bible::ui::reader::VerseScrollCtx {
        controller: scroll_controller,
        flash_verse,
        scroll_to,
        viewport_top,
    };
    // Hook runs unconditionally to keep hook order stable; it only
    // schedules a clear timer when a chip-tap seeded the flash.
    let mut flash_clear = flash_verse;
    use_hook(move || {
        if let Some(verse) = initial_flash_verse {
            spawn_forever(async move {
                async_io::Timer::after(Duration::from_secs(3)).await;
                if flash_clear.peek().as_ref() == Some(&verse) {
                    flash_clear.set(None);
                }
            });
        }
    });

    // Live chip-tap subscription — when the user clicks a Bible chip
    // while this mount is already on screen, the router dedupe
    // suppresses a remount so the snapshot-override path above never
    // re-runs. Subscribe to the host jumps for this kind and
    // apply the jump in-place; mirrors the fresh-mount path
    // (snapshot override + verse flash + flash-clear timer). Uses
    // `spawn` (scope-bound) so the receiver drops on unmount and the
    // host-side `retain` reaps the orphaned sender on the next
    // broadcast.
    let db_notify = db.clone();
    let notify_state = state;
    let notify_flash = flash_verse;
    let notify_scroll = scroll_controller;
    let notify_scroll_to = scroll_to;
    use_hook(move || {
        let rx = notein_plugin::services::HostServices::jumps(
            &*crate::plugins::host::shell(),
            BIBLE_PLUGIN_ID,
        );
        let doc = crate::plugins::content::doc_id(item);
        let mut state_setter = notify_state;
        let mut flash_setter = notify_flash;
        let mut scroll_setter = notify_scroll_to;
        let db_notify = db_notify;
        spawn(async move {
            while let Ok(target) = rx.recv_async().await {
                if target.doc != doc {
                    continue;
                }
                notein_plugin::services::HostServices::take_jump(
                    &*crate::plugins::host::shell(),
                    BIBLE_PLUGIN_ID,
                    doc,
                );
                let bytes = target.anchor;
                let Some(jump) = notein_bible::BibleAnchor::decode(&bytes) else {
                    log::warn!("bible chip: anchor decode failed");
                    continue;
                };
                log::info!(
                    "bible chip: applying jump bible={} book={} chap={} verse={}",
                    jump.bible_id,
                    jump.book_id,
                    jump.start_chapter,
                    jump.start_verse
                );
                let current_bible = { state_setter.peek().bible_id.clone() };
                if current_bible != jump.bible_id
                    && let Err(e) = state_setter
                        .write()
                        .switch_bible(&db_notify, &jump.bible_id)
                {
                    log::warn!("bible chip: switch_bible failed: {e}");
                }
                let canonical_book =
                    canonicalise_book_id(&db_notify, &jump.bible_id, &jump.book_id);
                if let Err(e) =
                    state_setter
                        .write()
                        .jump_to(&db_notify, &canonical_book, jump.start_chapter)
                {
                    log::warn!(
                        "bible chip: jump_to failed (bible={}, book={}, chap={}): {e}",
                        jump.bible_id,
                        canonical_book,
                        jump.start_chapter
                    );
                    continue;
                }
                if jump.start_verse == 0 {
                    let mut scroll = notify_scroll;
                    scroll.scroll_to_y(0);
                } else {
                    scroll_setter.set(Some(notein_bible::ui::reader::ScrollTarget::new(
                        jump.start_verse,
                    )));
                }
                flash_setter.set(Some(jump.start_verse));
                let mut flash_clear = flash_setter;
                let flash_verse_val = jump.start_verse;
                spawn(async move {
                    async_io::Timer::after(Duration::from_secs(3)).await;
                    if flash_clear.peek().as_ref() == Some(&flash_verse_val) {
                        flash_clear.set(None);
                    }
                });
            }
        });
    });

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
        crate::sync::snapshot_changed(item);
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
            // Section headings are prefetched in bulk during the
            // bible install (`install_background::prefetch_all_headings`)
            // so the reader never pays a lazy fetch after first open.
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
        // jump + reset. Reference-shaped queries (`"Juan 3:16"`) land
        // on this path only when the user explicitly picks the
        // suggestion labelled "Ir a Juan 3:16" — never automatically
        // while they're still typing.
        let hits_snapshot = search_hits.read().clone();
        if let Some(view) = hits_snapshot.iter().find(|v| v.suggestion_label == query) {
            let src = view.source.clone();
            if let Err(e) = state.write().jump_to(&db_jump, &src.book_id, src.chapter) {
                log::warn!("bible search jump: {e}");
            }
            flash_verse.set(Some(src.verse));
            let mut scroll_to = scroll_to;
            scroll_to.set(Some(notein_bible::ui::reader::ScrollTarget::new(src.verse)));
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
        let mut views: Vec<notein_bible::ui::search_sheet::SearchHitView> = hits
            .into_iter()
            .map(|h| {
                let book_name = books_for_search
                    .iter()
                    .find(|b| b.book_id == h.book_id)
                    .map(|b| b.name.clone());
                notein_bible::ui::search_sheet::hit_to_view(h, book_name.as_deref())
            })
            .collect();

        // Synthetic nav suggestion for reference-shaped queries
        // (`"Juan 3"`, `"Juan 3:16"`). Validates book + chapter +
        // (optional) verse exist in the installed Bible before
        // offering the pill at the top of the dropdown — invalid
        // refs silently fall through to the plain FTS results so the
        // user still sees snippet hits for e.g. `"Juan 99"`.
        if let Some(parsed) = notein_bible::reference::parse(query.trim())
            && let Some(book_id) = notein_bible::resolver::resolve_book_token(
                &db_search,
                &bible_for_search,
                &parsed.book_token,
            )
        {
            let total_chaps = db_search
                .chapter_count(&bible_for_search, &book_id)
                .unwrap_or(0);
            if parsed.chapter >= 1 && parsed.chapter <= total_chaps {
                let verse = parsed.start_verse.unwrap_or(1);
                let verse_ok = db_search
                    .verse_text(&bible_for_search, &book_id, parsed.chapter, verse)
                    .ok()
                    .flatten()
                    .is_some();
                if verse_ok {
                    let book_name = books_for_search
                        .iter()
                        .find(|b| b.book_id == book_id)
                        .map(|b| b.name.clone())
                        .unwrap_or_else(|| book_id.clone());
                    let label = if parsed.start_verse.is_some() {
                        format!("→ {} {}:{}", book_name, parsed.chapter, verse)
                    } else {
                        format!("→ {} {}", book_name, parsed.chapter)
                    };
                    let reference = format!("{} {}:{}", book_name, parsed.chapter, verse);
                    let source = notein_bible::db::SearchHit {
                        bible_id: bible_for_search.clone(),
                        book_id,
                        chapter: parsed.chapter,
                        verse,
                        snippet: String::new(),
                        score: 0.0,
                    };
                    let nav = notein_bible::ui::search_sheet::SearchHitView {
                        reference,
                        snippet_runs: Vec::new(),
                        suggestion_label: label,
                        source,
                    };
                    views.insert(0, nav);
                }
            }
        }

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
                    move || {
                        picker_open.set(false);
                        crate::screens::bible::open_catalog();
                    },
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
                on_home: None,
                on_sidebar: None,
                on_back: has_back.then(|| Box::new(on_back_cb) as toolbar::BackHandler),
                on_forward: has_forward.then(|| Box::new(on_forward_cb) as toolbar::BackHandler),
                on_notebook: None,
                on_links: None,
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
                    let mut scroll_to = scroll_to;
                    scroll_to.set(Some(notein_bible::ui::reader::ScrollTarget::new(verse)));
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
                use notein_bible::ui::reader::HighlightKind;
                let (lo, hi) = match target.kind {
                    HighlightKind::Verse(v) => (v, v),
                    HighlightKind::Range(a, b) => (a, b),
                };
                pending_highlight.set(Some(target));
                let (bible_id, book, chapter_num, quote) = {
                    let st = state.read();
                    let quote = ReaderSnapshot::from_state(&st)
                        .chapter
                        .map(|c| {
                            c.verses
                                .iter()
                                .filter(|v| v.verse >= lo && v.verse <= hi)
                                .map(|v| v.text.clone())
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default();
                    (st.bible_id.clone(), st.current_book.clone(), st.current_chapter, quote)
                };
                let existing = {
                    let st = state.read();
                    st.highlights_on(&book, chapter_num)
                        .into_iter()
                        .find(|h| h.start_verse <= hi && h.end_verse >= lo)
                        .cloned()
                };
                let anchor = notein_bible::BibleAnchor {
                    bible_id,
                    book_id: book.clone(),
                    start_chapter: chapter_num,
                    start_verse: lo,
                    end_chapter: chapter_num,
                    end_verse: hi,
                    start_char: None,
                    end_char: None,
                };
                let reference = notein_plugin::bookmark::SourceRef::Plugin {
                    plugin_id: BIBLE_PLUGIN_ID.to_owned(),
                    item: item.0,
                    anchor: anchor.encode(),
                    selection: (!quote.is_empty()).then(|| quote.clone()),
                };
                let label = crate::selection::label_of(&reference);
                let existing_rgb = existing
                    .as_ref()
                    .and_then(|h| h.color)
                    .map(notein_bible::state::u32_to_rgba)
                    .map(|c| [c[0], c[1], c[2]]);
                let active = existing_rgb.and_then(|c| crate::selection::inks().iter().position(|ink| *ink == c));
                let note = existing.as_ref().map(|h| h.note.clone()).unwrap_or_default();
                let (color_book, note_book) = (book.clone(), book.clone());
                let color_note = note.clone();
                let state_color = state;
                let mut state_note = state;
                let mut pending_end = pending_highlight;
                let mut preview_end = preview_color;
                crate::selection::SelectionMenu {
                    at: (target.anchor_x, target.anchor_y),
                    active,
                    on_color: std::rc::Rc::new(move |rgb: Option<[u8; 3]>| {
                        let packed = rgb.map(|c| notein_bible::state::rgba_to_u32([c[0], c[1], c[2], 0xFF]));
                        let (mut state_color, mut pending_end) = (state_color, pending_end);
                        state_color.write().upsert_highlight(&color_book, chapter_num, lo, hi, packed, color_note.clone());
                        pending_end.set(None);
                    }),
                    on_note: std::rc::Rc::new(move || {
                        let (tx, rx) = flume::bounded::<notein_plugin::services::AnnotationOutcome>(1);
                        let (preview_tx, preview_rx) = flume::unbounded::<Option<[u8; 3]>>();
                        let book = note_book.clone();
                        let existing_id = existing.as_ref().map(|h| h.id);
                        spawn_forever(async move {
                            while let Ok(rgb) = preview_rx.recv_async().await {
                                preview_end.set(rgb.map(|c| freya::prelude::Color::from_rgb(c[0], c[1], c[2])));
                            }
                        });
                        spawn_forever(async move {
                            match rx.recv_async().await {
                                Ok(notein_plugin::services::AnnotationOutcome::Saved { color, body }) => {
                                    let packed = color.map(|c| notein_bible::state::rgba_to_u32([c[0], c[1], c[2], 0xFF]));
                                    state_note.write().upsert_highlight(&book, chapter_num, lo, hi, packed, body);
                                }
                                Ok(notein_plugin::services::AnnotationOutcome::Deleted) => {
                                    if let Some(id) = existing_id {
                                        state_note.write().remove_highlight(id);
                                    }
                                }
                                Ok(notein_plugin::services::AnnotationOutcome::Cancelled) | Err(_) => {}
                            }
                            preview_end.set(None);
                            pending_end.set(None);
                        });
                        crate::annotation::AnnotationPopover::open(
                            notein_plugin::services::AnnotationRequest {
                                doc: crate::plugins::content::doc_id(item),
                                kind: BIBLE_PLUGIN_ID.to_owned(),
                                title: if lo == hi { format!("Verso {lo}") } else { format!("Versos {lo}–{hi}") },
                                quote: (!quote.is_empty()).then(|| quote.clone()),
                                color: existing_rgb,
                                body: note.clone(),
                                target: Some((target.anchor_x, target.anchor_y, 1.0, 1.0)),
                                existing: existing_id.is_some(),
                            },
                            Some(std::sync::Arc::new(move |rgb| {
                                let _ = preview_tx.send(rgb);
                            })),
                            std::sync::Arc::new(move |outcome| {
                                let _ = tx.send(outcome);
                            }),
                        );
                    }),
                    reference: Some((reference, label)),
                    document: Some(item),
                    extra: Vec::new(),
                    on_dismiss: std::rc::Rc::new(move || {
                        let mut pending = pending_highlight;
                        pending.set(None);
                    }),
                }
                .show();
            },
            Some(verse_scroll_ctx.clone()),
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
            .background((10, 10, 10))
            .open(nav_sheet_open)
            .height(460.0),
        );
    }

    column.into_element()
}

/// Grid of every book in the current Bible. Clicking a cell drills
/// into the matching chapter grid in page 1 of the sheet.
/// Return the installed `book_id` for `bible_id` that matches
/// `candidate` case-insensitively, or `candidate` verbatim when no
/// catalogue entry matches (lets the caller surface a clean
/// MissingBible error further down instead of silently rewriting).
///
/// Chip anchors persist whatever casing the plugin's resolver chose
/// at creation time — typically uppercase OSIS from the alias map —
/// while biblionlabs installs key `books` in lowercase. Normalising
/// here keeps every chip-driven jump + the mount-time snapshot
/// override on the same casing the DB expects.
fn canonicalise_book_id(db: &Db, bible_id: &str, candidate: &str) -> String {
    db.list_books(bible_id)
        .ok()
        .and_then(|books| {
            books
                .iter()
                .find(|b| b.book_id.eq_ignore_ascii_case(candidate))
                .map(|b| b.book_id.clone())
        })
        .unwrap_or_else(|| candidate.to_owned())
}

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
    let mut grid = rect()
        .width(Size::fill())
        .horizontal()
        .spacing(8.0)
        .content(Content::wrap_spacing(8.));
    for b in books {
        let book_id = b.book_id.clone();
        let mut on_pick = on_pick.clone();
        grid = grid.child(
            rect()
                .width(Size::px(CELL))
                .height(Size::px(CELL))
                .center()
                .with_corner_radius(10.0)
                .background(Color::from_rgb(32, 32, 32))
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
        .child(ScrollView::new().child(grid))
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
    let header = rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .spacing(10.0)
        .on_press(on_back)
        .child(
            SvgViewer::new(arrow_left())
                .color(Color::from_rgb(230, 230, 230))
                .width(Size::px(14.))
                .height(Size::px(14.)),
        )
        .child(
            label()
                .font_size(14.0)
                .color(Color::from_rgb(230, 230, 230))
                .text(book_name.to_string()),
        );

    let mut grid = rect()
        .width(Size::fill())
        .horizontal()
        .spacing(8.0)
        .content(Content::wrap_spacing(8.));

    for c in 1..=count {
        let mut on_pick = on_pick.clone();
        grid = grid.child(
            rect()
                .width(Size::px(CELL))
                .height(Size::px(CELL))
                .center()
                .with_corner_radius(8.0)
                .background(Color::from_rgb(32, 32, 32))
                .on_press(move |_: Event<PressEventData>| on_pick(c))
                .child(
                    label()
                        .font_size(13.0)
                        .color(Color::from_rgb(230, 230, 230))
                        .text(format!("{c}")),
                ),
        );
    }

    rect()
        .expanded()
        .padding((0., 4.))
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
    scroll_ctx: Option<notein_bible::ui::reader::VerseScrollCtx>,
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
                scroll_ctx,
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
