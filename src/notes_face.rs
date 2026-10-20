//! Text note page bound to a library item.
//!
//! The note body lives in the item's document as a Loro text, so two
//! people typing merge per character; the plugin snapshot keeps a plain
//! markdown copy for the index and older builds. Edited with
//! the same editor notein uses for bookmarks (chips, `/ref` references,
//! the user's markdown syntax setting) plus block reordering. The page
//! saves the body as it changes, keeps the notes index fresh so other
//! notes can reference this one, and scrolls to a heading when the
//! sidebar outline or a chip asks for it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use freya::prelude::*;
use freya::router::RouterContext;
use freya_markdown_editor::document::BlockId;
use freya_markdown_editor::prelude::*;
use notein_notes::ui::{self, Action, TopbarActions};
use notein_notes::{KIND_ID, NoteAnchor};

use crate::components::note_editor::{note_theme, notein_editor, use_note_keyboard};
use crate::doc::Doc;
use crate::hooks::{provide_safe_area_override, use_safe_area_insets};
use crate::library::ItemId;
use crate::sync::live::{LiveEvent, LiveMessage};
use crate::sync::open_docs;

/// A remote author is shown as typing this long after their last key.
const TYPING_TTL: Duration = Duration::from_millis(2500);

/// Width of the reading column.
const COLUMN_WIDTH: f32 = 760.0;
/// How long a jump waits for the note to be laid out.
const JUMP_ATTEMPTS: u32 = 40;
const JUMP_RETRY: Duration = Duration::from_millis(25);
/// Room kept between the line being typed and the viewport edges.
const CARET_MARGIN: f32 = 24.0;
/// Wait for the edit to be laid out before following it.
const FOLLOW_DELAY: Duration = Duration::from_millis(30);

thread_local! {
    /// Editors of the notes on screen, so a reference made elsewhere can
    /// fix a heading anchor in the live document instead of under it.
    static OPEN_NOTES: RefCell<HashMap<ItemId, EditorController>> = RefCell::new(HashMap::new());
}

#[derive(Debug, PartialEq, Eq)]
pub struct NotesFace {
    pub item: ItemId,
}

impl Component for NotesFace {
    fn render(&self) -> impl IntoElement {
        let item = self.item;
        let doc = use_hook(move || open_note_doc(item));
        let markdown = use_state({
            let doc = Arc::clone(&doc);
            move || open_docs::lock(&doc).body()
        });
        let typing = use_state(Vec::<(String, Instant)>::new);
        let title = use_state(String::new);
        let controller = use_editor_controller();
        let scroll = use_scroll_controller(ScrollConfig::default);
        let chrome = crate::app::use_chrome_state();
        let router = RouterContext::get();
        let viewport_area = try_consume_context::<notein_ui_kit::ViewportArea>();
        let insets = *use_safe_area_insets().read();
        provide_safe_area_override(istmo::plugins::EdgeInsets::default());
        let (mut viewport, content_top) = use_follow_typing(controller, scroll);
        use_hook(move || load_title(item, title));
        use_side_effect({
            let doc = Arc::clone(&doc);
            move || save(item, &doc, &markdown.read(), &title.read())
        });
        use_hook({
            let doc = Arc::clone(&doc);
            let mut markdown = markdown;
            move || follow_remote(item, doc, markdown)
        });
        use_hook({
            let mut typing = typing;
            move || follow_typing(item, typing)
        });
        use_hook(move || listen_for_jumps(item, controller, scroll));
        use_hook(move || {
            OPEN_NOTES.with(|open| open.borrow_mut().insert(item, controller));
        });
        use_drop({
            let doc = Arc::clone(&doc);
            move || open_docs::unregister(item, &doc)
        });
        use_drop(move || {
            OPEN_NOTES.with(|open| {
                let mut open = open.borrow_mut();
                if open.get(&item) == Some(&controller) {
                    open.remove(&item);
                }
            });
        });

        let read_only = controller.active_format().read_only;
        let actions = TopbarActions {
            on_home: Some(action(move || {
                let _ = router.push(crate::route::Route::Home);
            })),
            on_sidebar: Some(action(move || {
                crate::components::SidebarModal::open_plugin(chrome, KIND_ID, item, router);
            })),
            on_share: Some(Box::new(move |at| {
                let window = viewport_area.map(|area| *area.0.peek());
                crate::notes_share::open_menu(controller, title.peek().clone(), at, window);
            })),
            on_toggle_mode: Some(action(move || controller.toggle_read_only())),
            on_notebook: Some(action(move || crate::notebooks::open_notebook(item))),
            on_links: Some(action(move || {
                crate::components::DocActionsMenu::new(Some(item))
                    .on_show_links(move || {
                        crate::components::SidebarModal::open_plugin_tab(
                            chrome,
                            KIND_ID,
                            item,
                            router,
                            crate::components::SidebarTab::Links,
                        );
                    })
                    .open();
            })),
        };

        let editor = notein_editor(
            markdown,
            controller,
            Some(crate::bookmark_ui::plugin_completions_provider().into()),
        )
        .reorderable(true)
        .placeholder("Escribí tu nota… (/ para insertar)")
        .width(Size::fill())
        .max_width(Size::px(COLUMN_WIDTH))
        .padding((24., 20., 160., 20.));

        rect()
            .expanded()
            .padding((insets.top, insets.right, insets.bottom, insets.left))
            .background(ui::BACKGROUND)
            .content(Content::flex())
            .child(ui::topbar(&title.read(), read_only, actions))
            .maybe_child(typing_banner(&typing.read()))
            .maybe_child((!read_only).then(|| {
                rect()
                    .width(Size::fill())
                    .padding((0., 16., 8., 16.))
                    .child(
                        MarkdownToolbar::new(controller)
                            .theme(note_theme())
                            .mode_toggle(false),
                    )
            }))
            .child(
                ScrollView::new_controlled(scroll)
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .on_sized(move |event: Event<SizedEventData>| {
                        viewport.set_if_modified(Some(event.area));
                    })
                    .child(
                        rect()
                            .width(Size::fill())
                            .cross_align(Alignment::Center)
                            .on_sized(move |event: Event<SizedEventData>| {
                                content_top.set(event.area.min_y());
                            })
                            .child(editor),
                    ),
            )
    }
}

/// Fix the anchor of heading `slug` in note `item` before something links
/// to it, so the link survives renaming the heading. Works on the open
/// editor when the note is on screen, on the stored body otherwise.
pub(crate) fn pin_heading(item: ItemId, slug: &str) {
    let open = OPEN_NOTES.with(|open| open.borrow().get(&item).copied());
    if let Some(controller) = open {
        let heading = controller
            .document()
            .outline()
            .into_iter()
            .find(|heading| heading.slug == slug && !heading.pinned);
        if let Some(heading) = heading {
            controller.set_heading_anchor(heading.block, Some(heading.slug));
        }
        return;
    }
    let Some(pinned) = notein_notes::pin_heading(&read_body(item), slug) else {
        return;
    };
    if let Err(err) =
        crate::library::bodies::write_plugin_snapshot(item, KIND_ID, pinned.as_bytes())
    {
        log::warn!("notes: pin heading write failed: {err}");
        return;
    }
    if let Some(index) = crate::builtin::notes_index() {
        index.update_body(item.0, &pinned);
    }
}

fn action(on_press: impl FnMut() + 'static) -> Action {
    Box::new(on_press)
}

/// Load the note's document and publish it as the live copy. A note
/// from before bodies lived in the document takes its markdown from
/// the plugin snapshot once.
fn open_note_doc(item: ItemId) -> Arc<Mutex<Doc>> {
    let mut doc = crate::library::bodies::read_doc_bytes(item)
        .and_then(|bytes| Doc::from_snapshot(&bytes).ok())
        .unwrap_or_default();
    if doc.body().is_empty() {
        let legacy = read_body(item);
        if !legacy.is_empty() && doc.set_body(&legacy) {
            write_doc(item, &doc);
        }
    }
    let doc = Arc::new(Mutex::new(doc));
    open_docs::register(item, Arc::clone(&doc));
    doc
}

fn write_doc(item: ItemId, doc: &Doc) {
    match doc.snapshot_bytes() {
        Ok(bytes) => {
            if let Err(err) = crate::library::bodies::write_doc_bytes(item, &bytes) {
                log::warn!("notes: doc write failed: {err}");
            }
        }
        Err(err) => log::warn!("notes: doc snapshot failed: {err}"),
    }
}

/// Put what other devices typed into the editor as it lands.
fn follow_remote(item: ItemId, doc: Arc<Mutex<Doc>>, mut markdown: State<String>) {
    let changes = open_docs::watch(item);
    spawn(async move {
        while changes.recv_async().await.is_ok() {
            let body = open_docs::lock(&doc).body();
            if *markdown.peek() != body {
                markdown.set(body);
            }
        }
    });
}

/// Keep the list of people typing in this note right now.
fn follow_typing(item: ItemId, mut typing: State<Vec<(String, Instant)>>) {
    let events = crate::sync::live::watch(item);
    spawn(async move {
        while let Ok(LiveEvent { name, message, .. }) = events.recv_async().await {
            let LiveMessage::Caret { typing: active, .. } = message else {
                continue;
            };
            let now = Instant::now();
            let mut list = typing.peek().clone();
            list.retain(|(who, at)| *who != name && now.duration_since(*at) < TYPING_TTL);
            if active {
                list.push((name, now));
            }
            typing.set(list);
            // Clear the banner once the author pauses.
            let mut typing = typing;
            spawn(async move {
                async_io::Timer::after(TYPING_TTL).await;
                let now = Instant::now();
                let list: Vec<(String, Instant)> = typing
                    .peek()
                    .iter()
                    .filter(|(_, at)| now.duration_since(*at) < TYPING_TTL)
                    .cloned()
                    .collect();
                if list.len() != typing.peek().len() {
                    typing.set(list);
                }
            });
        }
    });
}

/// "Ana está escribiendo…" under the top bar while someone else types.
fn typing_banner(typing: &[(String, Instant)]) -> Option<Rect> {
    let now = Instant::now();
    let names: Vec<&str> = typing
        .iter()
        .filter(|(_, at)| now.duration_since(*at) < TYPING_TTL)
        .map(|(name, _)| name.as_str())
        .collect();
    if names.is_empty() {
        return None;
    }
    let text = if names.len() == 1 {
        format!("{} está escribiendo…", names[0])
    } else {
        format!("{} están escribiendo…", names.join(", "))
    };
    Some(
        rect()
            .width(Size::fill())
            .padding((2., 20., 6., 20.))
            .child(label().font_size(12.).color(ui::MUTED).text(text)),
    )
}

fn read_body(item: ItemId) -> String {
    crate::library::bodies::read_plugin_snapshot(item, KIND_ID)
        .ok()
        .flatten()
        .map(|bytes| notein_notes::body_from_snapshot(&bytes))
        .unwrap_or_default()
}

fn save(item: ItemId, doc: &Arc<Mutex<Doc>>, body: &str, title: &str) {
    let changed = open_docs::lock(doc).set_body(body);
    if changed {
        write_doc(item, &open_docs::lock(doc));
        crate::sync::doc_changed(item);
        crate::sync::live(
            item,
            &LiveMessage::Caret {
                offset: 0,
                typing: true,
            },
        );
    }
    if let Err(err) = crate::library::bodies::write_plugin_snapshot(item, KIND_ID, body.as_bytes())
    {
        log::warn!("notes: snapshot write failed: {err}");
    }
    crate::sync::snapshot_changed(item);
    crate::library::refs::record_note(item, body);
    if let Some(index) = crate::builtin::notes_index()
        && !title.is_empty()
    {
        index.update(item.0, title, body);
    }
}

fn load_title(item: ItemId, mut title: State<String>) {
    spawn(async move {
        let Ok(handle) = crate::home::open_library().await else {
            return;
        };
        let lib = handle.lock().await;
        if let Some(found) = lib.index().items.iter().find(|found| found.id == item) {
            title.set(found.name.clone());
        }
    });
}

/// Apply anchors queued for this note: the one that opened it, then the
/// ones sent while it is on screen (sidebar outline, chip taps).
fn listen_for_jumps(item: ItemId, controller: EditorController, scroll: ScrollController) {
    let host = crate::host_api::host_handle();
    if let Some(anchor) = host.take_source_anchor(KIND_ID, item.0) {
        jump(controller, scroll, &anchor);
    }
    let pending = host.subscribe_source_anchor(KIND_ID);
    spawn(async move {
        while let Ok(target) = pending.recv_async().await {
            if target != item.0 {
                continue;
            }
            if let Some(anchor) = crate::host_api::host_handle().take_source_anchor(KIND_ID, item.0)
            {
                jump(controller, scroll, &anchor);
            }
        }
    });
}

/// Follow the block being typed in, see [`follow`]. Returns where the
/// viewport and the note content are laid out, for the page to fill.
fn use_follow_typing(
    controller: EditorController,
    scroll: ScrollController,
) -> (State<Option<Area>>, Rc<Cell<f32>>) {
    let viewport = use_state(|| None::<Area>);
    let content_top = use_hook(|| Rc::new(Cell::new(0.0_f32)));
    use_note_keyboard(controller);
    {
        let content_top = content_top.clone();
        // Revision, viewport height and focus at the last run: following only
        // on their changes leaves the scroll to the user in between.
        let seen = use_hook(|| Rc::new(Cell::new((0_u64, 0.0_f32, false))));
        use_side_effect(move || {
            let revision = controller.revision();
            let focused = controller.has_focus();
            let viewport = *viewport.read();
            let height = viewport.map_or(0.0, |area| area.height());
            let (last_revision, last_height, was_focused) =
                seen.replace((revision, height, focused));
            let typed = revision != last_revision;
            let keyboard_opened = height < last_height - 1.0;
            if !focused || !(typed || keyboard_opened || !was_focused) {
                return;
            }
            let block = controller
                .selection()
                .map(|selection| selection.target.block);
            if let (Some(block), Some(viewport)) = (block, viewport) {
                follow(controller, scroll, block, viewport, content_top.clone());
            }
        });
    }
    (viewport, content_top)
}

/// Keep the line being typed on screen, above the soft keyboard once it
/// shrinks the viewport.
fn follow(
    controller: EditorController,
    mut scroll: ScrollController,
    block: BlockId,
    viewport: Area,
    content_top: Rc<Cell<f32>>,
) {
    spawn(async move {
        async_io::Timer::after(FOLLOW_DELAY).await;
        let Some((top, bottom)) = controller
            .caret_bounds()
            .or_else(|| controller.block_bounds(block))
        else {
            return;
        };
        let shown = viewport.min_y() - content_top.get();
        let height = viewport.height();
        let target = if bottom + CARET_MARGIN > shown + height {
            bottom + CARET_MARGIN - height
        } else if top - CARET_MARGIN < shown {
            top - CARET_MARGIN
        } else {
            return;
        };
        #[allow(clippy::cast_possible_truncation, reason = "pixel offsets")]
        scroll.scroll_to_y(-(target.max(0.) as i32));
    });
}

/// Scroll to the heading the anchor names, or to the top for the whole
/// note. Waits for the layout when the note was just opened.
fn jump(controller: EditorController, mut scroll: ScrollController, anchor: &[u8]) {
    let Some(anchor) = NoteAnchor::decode(anchor) else {
        log::warn!("notes: anchor decode failed");
        return;
    };
    spawn(async move {
        let Some(slug) = anchor.heading else {
            scroll.scroll_to_y(0);
            return;
        };
        for _ in 0..JUMP_ATTEMPTS {
            let offset = controller
                .document()
                .heading_by_slug(&slug)
                .and_then(|block| controller.block_offset(block));
            if let Some(offset) = offset {
                #[allow(clippy::cast_possible_truncation, reason = "pixel offsets")]
                scroll.scroll_to_y(-(offset as i32));
                return;
            }
            async_io::Timer::after(JUMP_RETRY).await;
        }
        log::info!("notes: heading {slug} not found");
    });
}
