//! The page of a text note.
//!
//! The note body is a text the shell merges with every device and person
//! the note is shared with, so two people typing merge per character;
//! the stored content keeps a plain markdown copy for the index. Edited
//! with the editor the shell sets up everywhere (chips, `/ref`
//! references, the user's markdown syntax setting) plus block
//! reordering. The page saves the body as it changes and scrolls to a
//! heading when the outline or a chip asks for it.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use freya::prelude::*;
use freya_markdown_editor::document::BlockId;
use freya_markdown_editor::prelude::*;
use notein_sdk::doc::DocId;
use notein_sdk::face::{ContentStore, FaceContext, HeaderAction};
use notein_sdk::services::{HistoryStep, Peer, Presence, TextBody};
use notein_sdk::tr;

use crate::anchor::NoteAnchor;
use crate::kind::{NoteContent, NotesKind};

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
    static OPEN_NOTES: RefCell<HashMap<DocId, EditorController>> = RefCell::new(HashMap::new());
}

/// The note's buttons in the pane header.
pub(crate) fn header_actions() -> Vec<HeaderAction> {
    use freya::icons::lucide;
    vec![
        HeaderAction::new(
            "reading",
            tr!("notes-header-read-or-edit"),
            lucide::book_open(),
        ),
        HeaderAction::new("share", tr!("notes-header-share"), lucide::share_2()),
    ]
}

/// The editor of `doc`, when the note is on screen.
pub(crate) fn open_editor(doc: DocId) -> Option<EditorController> {
    OPEN_NOTES.with(|open| open.borrow().get(&doc).copied())
}

pub(crate) struct NotePage {
    pub(crate) ctx: FaceContext<NotesKind>,
}

impl PartialEq for NotePage {
    fn eq(&self, other: &Self) -> bool {
        self.ctx.doc == other.ctx.doc && self.ctx.pane == other.ctx.pane
    }
}

impl Component for NotePage {
    #[allow(clippy::too_many_lines, reason = "declarative layout of the page")]
    fn render(&self) -> impl IntoElement {
        let ctx = &self.ctx;
        let doc = ctx.doc;
        let body = use_hook({
            let ctx = ctx.clone();
            move || open_body(&ctx)
        });
        let markdown = use_state({
            let body = Arc::clone(&body);
            move || body.text()
        });
        let locked = use_state({
            let store = ctx.store.clone();
            move || store.read_only()
        });
        let controller = use_editor_controller();
        let scroll = use_scroll_controller(ScrollConfig::default);
        let viewport_area = try_consume_context::<notein_ui_kit::ViewportArea>();
        let (mut viewport, content_top) = use_follow_typing(controller, scroll);
        use_side_effect({
            let (body, store) = (Arc::clone(&body), ctx.store.clone());
            move || {
                let text = markdown.read();
                body.set(&text);
                store.save(&NoteContent {
                    markdown: text.clone(),
                });
            }
        });
        use_hook({
            let body = Arc::clone(&body);
            move || follow_remote(body, markdown)
        });
        let presence = use_hook({
            let host = Arc::clone(&ctx.host);
            move || host.presence(doc)
        });
        let peers = use_state({
            let presence = Arc::clone(&presence);
            move || presence.peers()
        });
        use_hook({
            let presence = Arc::clone(&presence);
            move || follow_peers(presence, peers, controller, scroll)
        });
        use_side_effect({
            let presence = Arc::clone(&presence);
            move || presence.publish(encode_caret(controller.text_position()))
        });
        ctx.use_jumps(move |anchor: NoteAnchor| jump(controller, scroll, anchor));
        use_hook({
            let store = ctx.store.clone();
            move || follow_access(store, locked)
        });
        use_side_effect(move || controller.set_read_only(*locked.read()));
        use_hook(move || {
            OPEN_NOTES.with(|open| open.borrow_mut().insert(doc, controller));
        });
        use_drop(move || {
            OPEN_NOTES.with(|open| {
                let mut open = open.borrow_mut();
                if open.get(&doc) == Some(&controller) {
                    open.remove(&doc);
                }
            });
        });

        // A focused editor already undid it from its own key handler.
        ctx.use_history(move |step| {
            if controller.has_focus() {
                return;
            }
            match step {
                HistoryStep::Undo => controller.undo(),
                HistoryStep::Redo => controller.redo(),
            }
        });
        let host = Arc::clone(&ctx.host);
        ctx.use_header_presses(move |press| match press.action.as_str() {
            "reading" if !*locked.peek() => controller.toggle_read_only(),
            "share" => {
                let window = viewport_area.map(|area| *area.0.peek());
                let title = host.doc_name(doc).unwrap_or_default();
                crate::share::open_menu(&host, controller, title, Some(press.at), window);
            }
            _ => {}
        });
        let locked_now = *locked.read();
        let read_only = controller.active_format().read_only || locked_now;

        let editor = ctx
            .ui
            .editor_for(
                MarkdownEditor::new(markdown).controller(controller),
                ctx.doc,
            )
            .remote_carets(peers.read().iter().filter_map(remote_caret).collect())
            .reorderable(true)
            .placeholder(tr!("notes-editor-placeholder"))
            .width(Size::fill())
            .max_width(Size::px(COLUMN_WIDTH))
            .padding((24., 20., 160., 20.));

        rect()
            .expanded()
            .background(notein_ui_kit::theme::palette().bg)
            .content(Content::flex())
            .maybe_child(locked_now.then(|| banner(tr!("notes-read-only-banner"))))
            .maybe_child((!read_only).then(|| toolbar(ctx.ui.editor_theme(), controller)))
            .child(
                ScrollView::new_controlled(scroll)
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .on_sized(move |event: Event<SizedEventData>| {
                        viewport.set_if_modified(Some(event.area));
                    })
                    .child(notein_ui_kit::ScrollContent::new(
                        rect()
                            .width(Size::fill())
                            .cross_align(Alignment::Center)
                            .on_sized(move |event: Event<SizedEventData>| {
                                content_top.set(event.area.min_y());
                            })
                            .child(editor),
                    )),
            )
    }
}

/// The body of the note, taking its markdown from the stored content
/// once for a note from before bodies were merged.
fn open_body(ctx: &FaceContext<NotesKind>) -> Arc<dyn TextBody> {
    let body = ctx.host.body(ctx.doc);
    if body.text().is_empty() {
        let legacy = ctx.store.load().markdown;
        if !legacy.is_empty() {
            body.set(&legacy);
        }
    }
    body
}

/// Keep `locked` in step with what this person may do with the note.
fn follow_access(store: ContentStore<NoteContent>, mut locked: State<bool>) {
    let changes = store.access_changes();
    spawn(async move {
        while changes.recv_async().await.is_ok() {
            while changes.try_recv().is_ok() {}
            locked.set_if_modified(store.read_only());
        }
    });
}

/// Put what other devices typed into the editor as it lands.
fn follow_remote(body: Arc<dyn TextBody>, mut markdown: State<String>) {
    let changes = body.changes();
    spawn(async move {
        while changes.recv_async().await.is_ok() {
            let text = body.text();
            if *markdown.peek() != text {
                markdown.set(text);
            }
        }
    });
}

/// Keep `peers` up to date with whoever else has the note open, and
/// while following one of them keep their caret in view.
fn follow_peers(
    presence: Arc<dyn Presence>,
    mut peers: State<Vec<Peer>>,
    controller: EditorController,
    mut scroll: ScrollController,
) {
    let changes = presence.changes();
    spawn(async move {
        while changes.recv_async().await.is_ok() {
            peers.set(presence.peers());
            let Some(caret) = presence.followed().as_ref().and_then(remote_caret) else {
                continue;
            };
            let offset = controller
                .document()
                .targets()
                .get(caret.position.target)
                .and_then(|target| controller.block_offset(target.block));
            if let Some(offset) = offset {
                // A little room above, so the line is not at the very edge.
                #[allow(clippy::cast_possible_truncation, reason = "pixel offsets")]
                scroll.scroll_to_y(-((offset - FOLLOW_MARGIN).max(0.0) as i32));
            }
        }
    });
}

/// Space kept above a followed caret.
const FOLLOW_MARGIN: f32 = 120.0;

/// This person's caret as the others receive it: field, anchor and head
/// as little endian `u32`s; empty without a caret.
fn encode_caret(position: Option<TextPosition>) -> Vec<u8> {
    let Some(position) = position else {
        return Vec::new();
    };
    [position.target, position.anchor, position.head]
        .into_iter()
        .flat_map(|value| u32::try_from(value).unwrap_or(u32::MAX).to_le_bytes())
        .collect()
}

/// Someone else's caret, from what [`encode_caret`] made of it.
fn remote_caret(peer: &Peer) -> Option<RemoteCaret> {
    let mut values = peer
        .state
        .chunks_exact(4)
        .map(|bytes| bytes.try_into().map(u32::from_le_bytes));
    let mut next = || -> Option<usize> { usize::try_from(values.next()?.ok()?).ok() };
    let position = TextPosition {
        target: next()?,
        anchor: next()?,
        head: next()?,
    };
    let [r, g, b] = peer.color;
    Some(RemoteCaret {
        position,
        name: peer.name.clone(),
        color: Color::from_rgb(r, g, b),
    })
}

/// One muted line under the top bar.
fn banner(text: impl Into<String>) -> Rect {
    rect()
        .width(Size::fill())
        .padding((2., 20., 6., 20.))
        .child(
            label()
                .font_size(12.)
                .color(notein_ui_kit::theme::palette().t2)
                .text(text.into()),
        )
}

/// Follow the block being typed in, see [`follow`]. Returns where the
/// viewport and the note content are laid out, for the page to fill.
fn use_follow_typing(
    controller: EditorController,
    scroll: ScrollController,
) -> (State<Option<Area>>, Rc<Cell<f32>>) {
    let viewport = use_state(|| None::<Area>);
    let content_top = use_hook(|| Rc::new(Cell::new(0.0_f32)));
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
fn jump(controller: EditorController, mut scroll: ScrollController, anchor: NoteAnchor) {
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

/// The formatting bar over the note, its menus kept inside what the
/// system bars and the keyboard leave visible.
fn toolbar(theme: EditorTheme, controller: EditorController) -> Rect {
    rect()
        .width(Size::fill())
        .padding((0., 16., 8., 16.))
        .child(
            MarkdownToolbar::new(controller)
                .theme(theme)
                .mode_toggle(false)
                .popup_area(notein_ui_kit::safe_frame()),
        )
}
