//! Text note page bound to a library item.
//!
//! The note body is the item's plugin snapshot: raw markdown, edited with
//! the same editor notein uses for bookmarks (chips, `/ref` references,
//! the user's markdown syntax setting) plus block reordering. The page
//! saves the body as it changes, keeps the notes index fresh so other
//! notes can reference this one, and scrolls to a heading when the
//! sidebar outline or a chip asks for it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use freya::prelude::*;
use freya::router::RouterContext;
use freya_markdown_editor::document::BlockId;
use freya_markdown_editor::prelude::*;
use notein_notes::ui::{self, Action, TopbarActions};
use notein_notes::{KIND_ID, NoteAnchor};

use crate::components::note_editor::{note_theme, notein_editor, use_note_keyboard};
use crate::hooks::{provide_safe_area_override, use_safe_area_insets};
use crate::library::ItemId;

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
        let markdown = use_state(move || read_body(item));
        let title = use_state(String::new);
        let controller = use_editor_controller();
        let scroll = use_scroll_controller(ScrollConfig::default);
        let chrome = crate::app::use_chrome_state();
        let router = RouterContext::get();
        let doc_linked = try_consume_context::<crate::doc_linked::DocLinkedChromeExt>();
        let insets = *use_safe_area_insets().read();
        provide_safe_area_override(istmo::plugins::EdgeInsets::default());
        let (mut viewport, content_top) = use_follow_typing(controller, scroll);
        use_hook(move || load_title(item, title));
        use_side_effect(move || save(item, &markdown.read(), &title.read()));
        use_hook(move || listen_for_jumps(item, controller, scroll));
        use_hook(move || {
            OPEN_NOTES.with(|open| open.borrow_mut().insert(item, controller));
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
        let mut actions = TopbarActions {
            on_home: Some(action(move || {
                let _ = router.push(crate::route::Route::Home);
            })),
            on_sidebar: Some(action(move || {
                crate::components::SidebarModal::open_plugin(chrome, KIND_ID, item, router);
            })),
            on_toggle_mode: Some(action(move || controller.toggle_read_only())),
            ..TopbarActions::default()
        };
        if let Some(chrome) = &doc_linked {
            if chrome.is_split {
                let close = chrome.on_close.clone();
                actions.on_close_split = Some(action(move || close.call()));
            } else {
                let horizontal = chrome.on_split_horizontal.clone();
                let vertical = chrome.on_split_vertical.clone();
                actions.on_split_horizontal = Some(action(move || horizontal.call()));
                actions.on_split_vertical = Some(action(move || vertical.call()));
            }
        }

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

fn read_body(item: ItemId) -> String {
    crate::library::bodies::read_plugin_snapshot(item, KIND_ID)
        .ok()
        .flatten()
        .map(|bytes| notein_notes::body_from_snapshot(&bytes))
        .unwrap_or_default()
}

fn save(item: ItemId, body: &str, title: &str) {
    if let Err(err) = crate::library::bodies::write_plugin_snapshot(item, KIND_ID, body.as_bytes())
    {
        log::warn!("notes: snapshot write failed: {err}");
    }
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
        use_side_effect(move || {
            let _ = controller.revision();
            let block = controller
                .selection()
                .map(|selection| selection.target.block);
            let viewport = *viewport.read();
            if let (true, Some(block), Some(viewport)) = (controller.has_focus(), block, viewport) {
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
