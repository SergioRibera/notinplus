//! Slide-in left sidebar modal with three tabs:
//!
//! * **Docs** — library-wide item list with name + tag search; tapping a
//!   row pushes the appropriate `Route` for the item's plugin.
//! * **Contents** — hierarchical table-of-contents of the currently
//!   open doc. Primary source is `CanvasBackground::outline()` so
//!   plugins describe their own structure (PDF bookmarks, future
//!   mindmaps / text headings / ...); when a background returns an
//!   empty outline the shell falls back to a flat list derived from
//!   `pages()`. `Board::layers_snapshot` is always appended as the
//!   layer section.
//! * **Bookmarks** — the pre-rewrite behaviour, now navigable: a tap
//!   centres the viewport on the pin and surfaces the edit sheet.
//!
//! Every tab's body is a `VirtualScrollView` with a fixed `item_size` so
//! long libraries / long books do not materialise N rects per render.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use freya::animation::{AnimNum, Ease, Function, OnCreation, use_animation};
use freya::icons::lucide::{
    bookmark, chevron_down, chevron_right, ellipsis_vertical, file_text, layers, search,
    square_split_horizontal, square_split_vertical,
};
use freya::prelude::*;
use freya::router::*;

use crate::app::ChromeState;
use crate::bookmark_ui::BookmarkSort;
use crate::canvas::{Board, lock};
use crate::hooks::use_safe_area_insets;
use crate::library::{DocKind, Item, ItemId, SplitOrientation, SplitSide, Tag, TagId, WorkspaceId};
use crate::route::Route;

use super::{FormInput, theme::*};
use super::{Modal, ModalController};

/// Which pane is active inside the sidebar. Persisted on `ChromeState`
/// so flipping away and back lands on the same tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidebarTab {
    #[default]
    Docs,
    Contents,
    Bookmarks,
}

impl SidebarTab {
    const fn label(self) -> &'static str {
        match self {
            Self::Docs => "Documentos",
            Self::Contents => "Contenido",
            Self::Bookmarks => "Marcadores",
        }
    }
}

/// Fixed row height for every virtual list. Rows are rendered inside
/// `VirtualScrollView`, which needs a constant `item_size` to compute
/// viewport windowing.
const ROW_SIZE: f32 = 56.0;

#[derive(Clone)]
pub struct SidebarModal {
    state: ChromeState,
    contents: SidebarContents,
    /// Captured at the open site so the modal body — which renders
    /// inside `ModalPortal`, a sibling of `Router` — can navigate
    /// without panicking on a missing `RouterContext`.
    router: RouterContext,
    /// Snapshot of the active workspace at open time (if any). The
    /// context menu's split actions need a workspace to grow a pane
    /// tree inside; outside one they stay hidden.
    workspace: Option<WorkspaceId>,
}

impl PartialEq for SidebarModal {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state && self.workspace == other.workspace
    }
}

/// What the Contents and Bookmarks tabs describe.
#[derive(Clone)]
pub(crate) enum SidebarContents {
    /// A mounted canvas: background outline, pages, layers, bookmarks.
    Board(Arc<Mutex<Board>>),
    /// A plugin page: its outline comes from
    /// [`notein_plugin::CanvasKind::outline`] over the item's snapshot.
    Plugin { plugin_id: String, item: ItemId },
}

impl SidebarContents {
    fn board(&self) -> Option<Arc<Mutex<Board>>> {
        match self {
            Self::Board(board) => Some(Arc::clone(board)),
            Self::Plugin { .. } => None,
        }
    }
}

impl SidebarModal {
    pub(crate) fn open(
        state: ChromeState,
        board: Arc<Mutex<Board>>,
        router: RouterContext,
        workspace: Option<WorkspaceId>,
    ) {
        Self::open_with(state, SidebarContents::Board(board), router, workspace);
    }

    /// Sidebar for a page rendered by the plugin canvas `plugin_id`,
    /// opened on its Contents tab.
    pub(crate) fn open_plugin(state: ChromeState, plugin_id: &str, item: ItemId, router: RouterContext) {
        let workspace = match router.current::<Route>() {
            Route::WorkspaceView { id } => Some(id),
            _ => None,
        };
        let mut tab = state.sidebar_tab;
        tab.set(SidebarTab::Contents);
        let contents = SidebarContents::Plugin {
            plugin_id: plugin_id.to_owned(),
            item,
        };
        Self::open_with(state, contents, router, workspace);
    }

    fn open_with(
        state: ChromeState,
        contents: SidebarContents,
        router: RouterContext,
        workspace: Option<WorkspaceId>,
    ) {
        ModalController::get().open(
            Modal::new(Self {
                state,
                contents,
                router,
                workspace,
            })
            .manual()
            .width(SIDEBAR_WIDTH)
            .dismiss_on_backdrop(true),
        );
    }
}

/// Row shape consumed by the Docs virtual list. The library is loaded
/// off-thread into one of these flat vectors so the UI never holds the
/// `AsyncMutex<Library>` guard across a render.
#[derive(Clone, PartialEq)]
struct DocEntry {
    id: ItemId,
    kind: DocKind,
    name: String,
    color: Option<[u8; 4]>,
    tags: Vec<String>,
}

#[derive(Clone, Default, PartialEq)]
struct LibrarySnapshot {
    docs: Vec<DocEntry>,
}

/// What's under the context menu overlay. `y` is the sidebar-relative
/// pixel offset of the row the user tapped, so the menu card can be
/// anchored right next to the ellipsis trigger.
#[derive(Clone, PartialEq)]
struct DocMenuTarget {
    entry: DocEntry,
    y: f32,
}

impl Component for SidebarModal {
    fn render(&self) -> impl freya::prelude::IntoElement {
        // Modals mount at the root scope — outside any
        // `SafeAreaOverride` the Router subtree installs — so reading
        // insets here always resolves to the live platform value.
        // Passing `pad` in through `open(..)` would carry whatever
        // override the caller saw (zero inside the workspace), which
        // is why the sidebar used to clip under the Android status
        // bar.
        let pad = *use_safe_area_insets().read();
        let contents = &self.contents;
        let query = self.state.sidebar_query;
        let sort = self.state.sidebar_sort;
        let mut tab_state = self.state.sidebar_tab;
        let selected_bookmark = self.state.selected_bookmark;
        let body_buffer = self.state.bookmark_body;

        let pos_anim = use_animation(move |conf| {
            conf.on_creation(OnCreation::Run);
            AnimNum::new(-SIDEBAR_WIDTH, pad.left)
                .time(250)
                .ease(Ease::Out)
                .function(Function::Cubic)
        });
        let left_pos = pos_anim.get().value();

        let library_snapshot = use_library_snapshot();
        let current_tab = *tab_state.read();
        // Local overlay state — stashing it on ChromeState would leak
        // sidebar lifecycle into every pane. The State is scoped to
        // this render tree and dropped when the modal closes.
        let menu_target = use_state(|| Option::<DocMenuTarget>::None);
        // Collapsed outline-node ids in the Contents tab. Session-
        // scoped — reopening the sidebar starts expanded, except for
        // long plugin outlines (a whole Bible) that start folded.
        let collapsed = use_state({
            let contents = contents.clone();
            move || initially_collapsed(&contents)
        });

        let header = sidebar_header(current_tab, query, sort, move |next| tab_state.set(next));

        let router = self.router;
        let workspace = self.workspace;
        let body = match current_tab {
            SidebarTab::Docs => docs_body(library_snapshot, query, menu_target, router),
            SidebarTab::Contents => contents_body(contents, query, collapsed),
            SidebarTab::Bookmarks => match contents {
                SidebarContents::Board(board) => {
                    bookmarks_body(board, query, sort, selected_bookmark, body_buffer)
                }
                SidebarContents::Plugin { .. } => {
                    empty_state("Este documento no tiene marcadores.")
                }
            },
        };

        // `Size::percent(100.)` would resolve against the modal portal's
        // parent — which has no explicit height for a manually-placed
        // body — and the shell would collapse (or, when the parent
        // falls back to window-height, extend past the bottom safe
        // area). Compute the carved height from the window directly so
        // the sidebar always fits between top and bottom insets.
        let pad_top = pad.top;
        let pad_bottom = pad.bottom;
        let height_fn = Size::func_data(
            move |ctx| Some((ctx.root - pad_top - pad_bottom).max(0.0)),
            &(pad_top.to_bits(), pad_bottom.to_bits()),
        );
        let mut shell = rect()
            .position(Position::new_absolute().top(pad.top).left(left_pos))
            .width(Size::px(SIDEBAR_WIDTH))
            .height(height_fn)
            .background(PANEL_BG)
            .overflow(Overflow::Clip)
            .color(TEXT_PRIMARY)
            .direction(Direction::Vertical)
            .content(Content::Flex)
            .child(header);
        shell = shell.child(rect().width(Size::fill()).height(Size::fill()).child(body));
        if let Some(target) = menu_target.read().clone() {
            shell = shell.child(doc_context_overlay(
                target,
                pad.top,
                menu_target,
                router,
                workspace,
            ));
        }
        shell
    }
}

// ---------- Header / tab bar ----------------------------------------------

fn sidebar_header(
    current: SidebarTab,
    query: State<String>,
    sort: State<BookmarkSort>,
    on_pick: impl FnMut(SidebarTab) + Clone + 'static,
) -> Rect {
    let tabs = [
        (SidebarTab::Docs, file_text()),
        (SidebarTab::Contents, layers()),
        (SidebarTab::Bookmarks, bookmark()),
    ];
    let mut tab_row = rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .content(Content::Flex)
        .main_align(Alignment::SpaceBetween)
        .width(Size::fill())
        .padding((8.0, 10.0))
        .spacing(6.0);
    for (tab, icon) in tabs {
        let mut picker = on_pick.clone();
        tab_row = tab_row.child(tab_chip(tab, icon, current == tab, move || picker(tab)));
    }
    rect()
        .direction(Direction::Vertical)
        .width(Size::fill())
        .background(SIDEBAR_BG)
        .child(tab_row)
        .child(search_input(query, current, sort))
}

fn tab_chip(
    tab: SidebarTab,
    icon: Bytes,
    selected: bool,
    mut on_press: impl FnMut() + 'static,
) -> Element {
    let (bg, glyph) = if selected {
        (CARD_BG, TEXT_PRIMARY)
    } else {
        (SIDEBAR_BG, TEXT_MUTED)
    };

    let flex = if selected { 2. } else { 1. };

    let mut chip = rect()
        .width(Size::flex(flex))
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .spacing(6.0)
        .padding((6.0, 10.0))
        .with_corner_radius(8.0)
        .background(bg)
        .on_press(move |_: Event<PressEventData>| on_press())
        .maybe(!selected, |r| r.center())
        .child(
            SvgViewer::new(icon)
                .fill(glyph)
                .width(Size::px(14.))
                .height(Size::px(14.)),
        );
    // Collapse to icon-only when inactive so the three chips stay on
    // one line regardless of the sidebar width.
    if selected {
        chip = chip.child(label().color(glyph).font_size(12.0).text(tab.label()));
    }
    chip.into_element()
}

fn search_input(query: State<String>, tab: SidebarTab, sort: State<BookmarkSort>) -> Element {
    let placeholder = match tab {
        SidebarTab::Docs => "Buscar por nombre o etiqueta",
        SidebarTab::Bookmarks => "Buscar marcadores",
        SidebarTab::Contents => "Buscar en el contenido",
    };
    let mut row = rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .width(Size::fill())
        .padding((6.0, 8.0))
        .spacing(6.0)
        .content(Content::Flex)
        .child(
            FormInput::new(query)
                .left_icon(search())
                .placeholder(placeholder)
                .min_width(Size::px(150.))
                .width(Size::flex(1.)),
        );
    if matches!(tab, SidebarTab::Bookmarks) {
        row = row.child(sort_toggle(sort));
    }
    row.into_element()
}

fn sort_toggle(sort: State<BookmarkSort>) -> Element {
    let mut sort_setter = sort;
    let on_toggle = move |_: Event<PressEventData>| {
        let next = sort_setter.peek().next();
        sort_setter.set(next);
    };
    rect()
        .padding((4.0, 8.0))
        .background(CARD_BG)
        .with_corner_radius(6.0)
        .on_press(on_toggle)
        .child(
            label()
                .color(TEXT_PRIMARY)
                .font_size(12.0)
                .text(sort.read().label()),
        )
        .into_element()
}

// ---------- Docs tab ------------------------------------------------------

fn use_library_snapshot() -> State<LibrarySnapshot> {
    let mut snap = use_state(LibrarySnapshot::default);
    use_hook(move || {
        spawn(async move {
            let Ok(handle) = crate::home::open_library().await else {
                return;
            };
            let docs: Vec<DocEntry> = {
                let guard = handle.lock().await;
                let index = guard.index();
                let tag_name = |id: &TagId| {
                    index
                        .tag(*id)
                        .map(|t: &Tag| t.name.clone())
                        .unwrap_or_default()
                };
                index
                    .items
                    .iter()
                    .map(|it: &Item| DocEntry {
                        id: it.id,
                        kind: it.kind.clone(),
                        name: it.name.clone(),
                        color: it.color,
                        tags: it.tags.iter().map(tag_name).collect(),
                    })
                    .collect()
            };
            snap.set(LibrarySnapshot { docs });
        });
    });
    snap
}

fn docs_body(
    snapshot: State<LibrarySnapshot>,
    query: State<String>,
    menu_target: State<Option<DocMenuTarget>>,
    router: RouterContext,
) -> Element {
    let needle = query.read().trim().to_ascii_lowercase();
    let filtered: Vec<DocEntry> = snapshot
        .read()
        .docs
        .iter()
        .filter(|e| {
            if needle.is_empty() {
                return true;
            }
            e.name.to_ascii_lowercase().contains(&needle)
                || e.tags
                    .iter()
                    .any(|t| t.to_ascii_lowercase().contains(&needle))
        })
        .cloned()
        .collect();

    if filtered.is_empty() {
        return empty_state(if needle.is_empty() {
            "Sin documentos."
        } else {
            "Nada coincide con la búsqueda."
        });
    }

    let length = filtered.len();
    VirtualScrollView::new_with_data(filtered, move |item, data: &Vec<DocEntry>| {
        let entry = data[item.index].clone();
        doc_row(entry, menu_target, router).into_element()
    })
    .length(length)
    .width(Size::fill())
    .item_size(ROW_SIZE)
    .into_element()
}

fn doc_row(
    entry: DocEntry,
    menu_target: State<Option<DocMenuTarget>>,
    router: RouterContext,
) -> Element {
    let DocEntry {
        id,
        kind,
        name,
        color,
        tags,
    } = entry.clone();
    let dot_color = color
        .map(|[r, g, b, _]| Color::from_rgb(r, g, b))
        .unwrap_or(TEXT_MUTED);
    let subtitle = if tags.is_empty() {
        kind_label(&kind).to_owned()
    } else {
        format!("{} · {}", kind_label(&kind), tags.join(", "))
    };
    let on_press = move |_: Event<PressEventData>| {
        let route = route_for_kind(&kind, id);
        let _ = router.push(route);
        ModalController::get().close();
    };
    let title = if name.is_empty() {
        "(sin título)".to_owned()
    } else {
        name
    };
    rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .spacing(10.0)
        .padding((8.0, 10.0))
        .margin((2.0, 6.0))
        .width(Size::fill())
        .height(Size::px(ROW_SIZE - 4.0))
        .background(CARD_BG)
        .with_corner_radius(6.0)
        .on_press(on_press)
        .content(Content::Flex)
        .child(
            rect()
                .width(Size::px(10.0))
                .height(Size::px(10.0))
                .with_corner_radius(5.0)
                .background(dot_color),
        )
        .child(
            rect()
                .direction(Direction::Vertical)
                .width(Size::flex(1.))
                .spacing(2.0)
                .child(label().color(TEXT_PRIMARY).font_size(13.0).text(title))
                .child(label().color(TEXT_MUTED).font_size(11.0).text(subtitle)),
        )
        .child(doc_row_menu_button(entry, menu_target))
        .into_element()
}

fn doc_row_menu_button(entry: DocEntry, mut menu_target: State<Option<DocMenuTarget>>) -> Element {
    let open_menu = move |e: Event<PressEventData>| {
        // Stop the press from bubbling to the row's own navigate
        // handler — otherwise the ellipsis tap would jump to the item.
        e.stop_propagation();
        let y = press_screen_y(&e).unwrap_or(0.0);
        menu_target.set(Some(DocMenuTarget {
            entry: entry.clone(),
            y,
        }));
    };
    rect()
        .width(Size::px(28.0))
        .height(Size::px(28.0))
        .center()
        .with_corner_radius(6.0)
        .on_press(open_menu)
        .child(
            SvgViewer::new(ellipsis_vertical())
                .fill(TEXT_MUTED)
                .width(Size::px(16.))
                .height(Size::px(16.)),
        )
        .into_element()
}

/// Pull the global y coordinate out of a press event. Mouse / touch
/// carry a `global_location`; keyboard-triggered presses do not, in
/// which case the caller falls back to a sensible default.
fn press_screen_y(e: &Event<PressEventData>) -> Option<f32> {
    match e.data() {
        PressEventData::Mouse(m) => Some(m.global_location.y as f32),
        PressEventData::Touch(t) => Some(t.global_location.y as f32),
        PressEventData::Keyboard(_) => None,
    }
}

/// Floating menu panel + transparent backdrop. The backdrop covers the
/// whole sidebar so tapping anywhere outside the menu dismisses it; the
/// menu itself anchors right next to the tapped row.
fn doc_context_overlay(
    target: DocMenuTarget,
    pad_top: f32,
    menu_setter: State<Option<DocMenuTarget>>,
    router: RouterContext,
    workspace: Option<WorkspaceId>,
) -> Element {
    // Sidebar-local y of the ellipsis button. Clamp above the header +
    // some headroom so the card never slides off the top; actual bottom
    // clamping is left to the layout engine (card height is small).
    let anchor_y = (target.y - pad_top).max(60.0);
    let entry = target.entry.clone();

    let mut close_setter = menu_setter;
    let backdrop = rect()
        .width(Size::fill())
        .height(Size::fill())
        .position(Position::new_absolute().top(0.0).left(0.0))
        .on_press(move |_: Event<PressEventData>| close_setter.set(None));

    let card = doc_context_card(entry, workspace, menu_setter, router).position(
        Position::new_absolute()
            .top(anchor_y)
            .left(12.0)
            .right(12.0),
    );

    rect()
        .layer(Layer::OverlayLevel(10))
        .position(
            Position::new_absolute()
                .top(0.0)
                .left(0.0)
                .right(0.0)
                .bottom(0.0),
        )
        .child(backdrop)
        .child(card)
        .into_element()
}

/// Menu card body. Variants depend on workspace vs non-workspace route
/// — split actions only make sense when there is a pane tree to grow.
fn doc_context_card(
    entry: DocEntry,
    workspace: Option<WorkspaceId>,
    menu_setter: State<Option<DocMenuTarget>>,
    router: RouterContext,
) -> Rect {
    let mut card = rect()
        .direction(Direction::Vertical)
        .background(CARD_BG)
        .with_corner_radius(10.0)
        .padding((6.0, 6.0))
        .spacing(2.0);

    // "Abrir" — navigate the top-level route for the item's kind.
    {
        let id = entry.id;
        let kind = entry.kind.clone();
        let mut setter = menu_setter;
        card = card.child(menu_row(
            file_text(),
            "Abrir",
            move |_: Event<PressEventData>| {
                let route = route_for_kind(&kind, id);
                let _ = router.push(route);
                setter.set(None);
                ModalController::get().close();
            },
        ));
    }

    // Split actions require a workspace to split within. Outside one
    // (plain CanvasView etc.) they would have no referent, so skip.
    if let Some(ws_id) = workspace {
        let entry_v = entry.clone();
        let mut setter_v = menu_setter;
        card = card.child(menu_row(
            square_split_vertical(),
            "Dividir vertical",
            move |_: Event<PressEventData>| {
                schedule_split_to(ws_id, entry_v.id, SplitOrientation::Vertical);
                setter_v.set(None);
                ModalController::get().close();
            },
        ));
        let entry_h = entry.clone();
        let mut setter_h = menu_setter;
        card = card.child(menu_row(
            square_split_horizontal(),
            "Dividir horizontal",
            move |_: Event<PressEventData>| {
                schedule_split_to(ws_id, entry_h.id, SplitOrientation::Horizontal);
                setter_h.set(None);
                ModalController::get().close();
            },
        ));
    }

    card
}

fn menu_row<F>(icon: Bytes, text: &'static str, on_press: F) -> Element
where
    F: FnMut(Event<PressEventData>) + 'static,
{
    rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .width(Size::fill())
        .spacing(10.0)
        .padding((8.0, 10.0))
        .with_corner_radius(6.0)
        .on_press(on_press)
        .child(
            SvgViewer::new(icon)
                .fill(TEXT_PRIMARY)
                .width(Size::px(14.))
                .height(Size::px(14.)),
        )
        .child(label().color(TEXT_PRIMARY).font_size(13.0).text(text))
        .into_element()
}

/// Split the focused pane in `workspace` along `orientation` and
/// mount `item` in the new leaf. Silently no-ops when `item` already
/// appears anywhere in the pane tree — the user asked that no doc
/// show up more than once in the infinite split arrangement.
fn schedule_split_to(workspace: WorkspaceId, item: ItemId, orientation: SplitOrientation) {
    spawn(async move {
        let Ok(handle) = crate::home::open_library().await else {
            return;
        };
        let mut lib = handle.lock().await;
        let Some(ws) = lib.workspace(workspace).cloned() else {
            log::warn!("workspace {workspace} missing on split");
            return;
        };
        let already_open = ws
            .layout
            .leaves()
            .iter()
            .any(|p| p.tabs.iter().any(|t| t.item() == Some(item)));
        if already_open {
            log::info!("item {item} already in workspace tree; skipping split");
            return;
        }
        let Some(focused) = ws.focused_pane else {
            log::warn!("workspace {workspace} has no focused pane");
            return;
        };
        let new_pane = match lib
            .split_workspace_pane(workspace, focused, orientation, SplitSide::Second)
            .await
        {
            Ok(pid) => pid,
            Err(err) => {
                log::error!("split_workspace_pane: {err}");
                return;
            }
        };
        if let Err(err) = lib.add_pane_tab(workspace, new_pane, item).await {
            log::error!("add_pane_tab: {err}");
        }
    });
}

fn kind_label(kind: &DocKind) -> &'static str {
    crate::faces::kind_label(kind)
}

fn route_for_kind(kind: &DocKind, id: ItemId) -> Route {
    crate::route::route_for_kind(kind, id)
}

// ---------- Contents tab --------------------------------------------------

/// Flat TOC entry rendered in the Contents list. Built from three
/// sources, in order:
///
/// 1. `CanvasBackground::outline()` — the plugin-authored, hierarchical
///    table of contents (PDF bookmarks today; future canvases, text
///    documents, mindmaps can plug in their own).
/// 2. If step 1 is empty, fall back to a flat list derived from
///    `CanvasBackground::pages()` so paginated backgrounds without a
///    real outline (lined paper, grid) still get page jumps.
/// 3. `Board::layers_snapshot()` — always appended; universal fallback
///    for stroke-centric work.
#[derive(Clone, PartialEq)]
enum TocEntry {
    /// Visual-only separator rendered as a muted label.
    Section(String),
    /// Outline node authored by the active background. Activation
    /// routes through `CanvasBackground::navigate_to_outline`;
    /// `has_children`/`collapsed` drive the chevron toggle.
    Outline {
        id: freya_canvas_bg::OutlineNodeId,
        label: String,
        subtitle: Option<String>,
        icon: freya_canvas_bg::OutlineIcon,
        depth: u8,
        has_children: bool,
        collapsed: bool,
    },
    /// Pages-fallback leaf. Carries world coords directly because the
    /// background has no outline tree to resolve against.
    Page {
        label: String,
        world_x: f32,
        world_y: f32,
    },
    /// Activate a layer by id (no viewport move).
    Layer {
        id: u32,
        name: String,
        stroke_count: usize,
        is_active: bool,
    },
}

fn collect_toc(
    board: &Arc<Mutex<Board>>,
    query: &str,
    collapsed: &HashSet<freya_canvas_bg::OutlineNodeId>,
) -> Vec<TocEntry> {
    let g = lock(board);
    let mut out = Vec::new();
    let needle = query.trim().to_ascii_lowercase();

    let outline = g.background().outline();
    if !outline.is_empty() {
        let has_match = outline
            .nodes
            .iter()
            .any(|n| outline_subtree_matches(n, &needle));
        if needle.is_empty() || has_match {
            out.push(TocEntry::Section("Contenido".to_owned()));
            for node in &outline.nodes {
                flatten_outline(node, 0, &needle, collapsed, &mut out);
            }
        }
    } else {
        let pages = g.background().pages();
        if !pages.is_empty() {
            let page_label = |i: usize| format!("Página {}", i + 1);
            let visible: Vec<(usize, &freya_canvas_bg::PageLayout)> = pages
                .iter()
                .enumerate()
                .filter(|(i, _)| {
                    needle.is_empty() || page_label(*i).to_ascii_lowercase().contains(&needle)
                })
                .collect();
            if !visible.is_empty() {
                out.push(TocEntry::Section("Páginas".to_owned()));
                for (i, page) in visible {
                    let cx = (page.rect.min_x + page.rect.max_x) * 0.5;
                    let cy = (page.rect.min_y + page.rect.max_y) * 0.5;
                    out.push(TocEntry::Page {
                        label: page_label(i),
                        world_x: cx,
                        world_y: cy,
                    });
                }
            }
        }
    }

    let layers = g.layers_snapshot();
    if !layers.is_empty() {
        let active_id = g.active_layer_id();
        let filtered: Vec<_> = layers
            .into_iter()
            .rev()
            .filter(|l| needle.is_empty() || l.name.to_ascii_lowercase().contains(&needle))
            .collect();
        if !filtered.is_empty() {
            out.push(TocEntry::Section("Capas".to_owned()));
            for layer in filtered {
                out.push(TocEntry::Layer {
                    id: layer.id,
                    name: layer.name,
                    stroke_count: layer.stroke_count,
                    is_active: layer.id == active_id,
                });
            }
        }
    }
    out
}

/// Returns `true` when `node` or any descendant's label contains
/// `needle`. Caller feeds already-lowercased `needle`; node labels
/// are lowercased per-call (cheap compared to the DFS walk).
fn outline_subtree_matches(node: &freya_canvas_bg::OutlineNode, needle: &str) -> bool {
    if needle.is_empty() || node.label.to_ascii_lowercase().contains(needle) {
        return true;
    }
    node.children
        .iter()
        .any(|c| outline_subtree_matches(c, needle))
}

/// Depth-first flatten of an [`OutlineNode`] tree into `out`. `depth`
/// is clamped at 8 — any outline deeper than that is suspicious and
/// the extra indent levels would truncate anyway. When `needle` is
/// non-empty, a node is emitted iff it or any descendant matches —
/// parents stay visible so the user can see the hierarchy context of
/// a matched leaf.
fn flatten_outline(
    node: &freya_canvas_bg::OutlineNode,
    depth: u8,
    needle: &str,
    collapsed: &HashSet<freya_canvas_bg::OutlineNodeId>,
    out: &mut Vec<TocEntry>,
) {
    if !outline_subtree_matches(node, needle) {
        return;
    }
    let has_children = !node.children.is_empty();
    // Collapsed state ignored during a search — the user wants to see
    // what matched regardless of expand/collapse history.
    let is_collapsed = has_children && needle.is_empty() && collapsed.contains(&node.id);
    out.push(TocEntry::Outline {
        id: node.id,
        label: node.label.clone(),
        subtitle: node.subtitle.clone(),
        icon: node.icon.clone(),
        depth: depth.min(8),
        has_children,
        collapsed: is_collapsed,
    });
    if is_collapsed {
        return;
    }
    for child in &node.children {
        flatten_outline(child, depth.saturating_add(1), needle, collapsed, out);
    }
}

fn outline_icon_element(icon: &freya_canvas_bg::OutlineIcon, color: Color) -> Element {
    use freya_canvas_bg::{IconType, OutlineIcon::*};
    let semantic = match icon {
        Layer => Some(layers()),
        Bookmark => Some(bookmark()),
        Section | Page | Heading => Some(file_text()),
        Custom(_) => None,
    };
    if let Some(bytes) = semantic {
        return SvgViewer::new(bytes)
            .fill(color)
            .width(Size::px(14.))
            .height(Size::px(14.))
            .into_element();
    }
    // Plugin-supplied icon.
    let Custom(custom) = icon else { unreachable!() };
    match custom {
        IconType::Svg(bytes) => {
            // Freya's SvgViewer wants `Bytes`; the trait hands us an
            // `Arc<[u8]>` so an allocation-free copy via `Bytes::from`
            // is impossible — one `Vec<u8>` clone per render is still
            // trivial compared to the SVG parser itself.
            SvgViewer::new(Bytes::from(bytes.to_vec()))
                .fill(color)
                .width(Size::px(14.))
                .height(Size::px(14.))
                .into_element()
        }
        IconType::Png(bytes) => ImageViewer::new(Bytes::from(bytes.to_vec()))
            .width(Size::px(14.))
            .height(Size::px(14.))
            .into_element(),
    }
}

/// Outline a plugin canvas publishes for `item`, read from the snapshot
/// the shell keeps for it.
fn plugin_outline(plugin_id: &str, item: ItemId) -> (freya_canvas_bg::Outline, Vec<u8>) {
    let snapshot = crate::library::bodies::read_plugin_snapshot(item, plugin_id)
        .ok()
        .flatten()
        .unwrap_or_default();
    let plugin = {
        let registry = crate::plugins::registry();
        let guard = crate::plugins::lock(&registry);
        guard.get_canvas(plugin_id).cloned()
    };
    let outline = plugin.map(|plugin| plugin.outline(&snapshot)).unwrap_or_default();
    (outline, snapshot)
}

/// Long plugin outlines start with every parent folded.
fn initially_collapsed(contents: &SidebarContents) -> HashSet<freya_canvas_bg::OutlineNodeId> {
    const FOLD_ABOVE: usize = 80;
    fn parents(nodes: &[freya_canvas_bg::OutlineNode], out: &mut HashSet<freya_canvas_bg::OutlineNodeId>) {
        for node in nodes {
            if !node.children.is_empty() {
                out.insert(node.id);
                parents(&node.children, out);
            }
        }
    }
    let mut folded = HashSet::new();
    if let SidebarContents::Plugin { plugin_id, item } = contents {
        let (outline, _) = plugin_outline(plugin_id, *item);
        if outline.len() > FOLD_ABOVE {
            parents(&outline.nodes, &mut folded);
        }
    }
    folded
}

fn collect_plugin_toc(
    plugin_id: &str,
    item: ItemId,
    query: &str,
    collapsed: &HashSet<freya_canvas_bg::OutlineNodeId>,
) -> Vec<TocEntry> {
    let (outline, _) = plugin_outline(plugin_id, item);
    let needle = query.trim().to_ascii_lowercase();
    let mut out = Vec::new();
    if outline.nodes.iter().any(|n| outline_subtree_matches(n, &needle)) {
        out.push(TocEntry::Section("Contenido".to_owned()));
        for node in &outline.nodes {
            flatten_outline(node, 0, &needle, collapsed, &mut out);
        }
    }
    out
}

/// Move the open plugin page to an outline node: the anchor travels the
/// same channel as a chip tap, which the page already listens to.
fn activate_plugin_outline(plugin_id: &str, item: ItemId, node: freya_canvas_bg::OutlineNodeId) {
    let (_, snapshot) = plugin_outline(plugin_id, item);
    let plugin = {
        let registry = crate::plugins::registry();
        let guard = crate::plugins::lock(&registry);
        guard.get_canvas(plugin_id).cloned()
    };
    if let Some(anchor) = plugin.and_then(|plugin| plugin.outline_anchor(&snapshot, node)) {
        crate::host_api::host_handle().queue_source_anchor(plugin_id, item.0, anchor);
    }
}

fn contents_body(
    contents: &SidebarContents,
    query: State<String>,
    collapsed: State<HashSet<freya_canvas_bg::OutlineNodeId>>,
) -> Element {
    let needle_src = query.read();
    let entries = {
        let guard = collapsed.read();
        match contents {
            SidebarContents::Board(board) => collect_toc(board, &needle_src, &guard),
            SidebarContents::Plugin { plugin_id, item } => {
                collect_plugin_toc(plugin_id, *item, &needle_src, &guard)
            }
        }
    };
    if entries.is_empty() {
        let msg = if needle_src.trim().is_empty() {
            "El documento no tiene contenido todavía."
        } else {
            "Nada coincide con la búsqueda."
        };
        return empty_state(msg);
    }

    let length = entries.len();
    let target = contents.clone();
    VirtualScrollView::new_with_data(entries, move |item, data: &Vec<TocEntry>| {
        let entry = data[item.index].clone();
        toc_row(entry, target.clone(), collapsed).into_element()
    })
    .length(length)
    .width(Size::fill())
    .item_size(ROW_SIZE)
    .into_element()
}

fn toc_row(
    entry: TocEntry,
    target: SidebarContents,
    collapsed: State<HashSet<freya_canvas_bg::OutlineNodeId>>,
) -> Element {
    match entry {
        TocEntry::Section(title) => rect()
            .direction(Direction::Horizontal)
            .cross_align(Alignment::Center)
            .padding((6.0, 10.0))
            .margin((2.0, 6.0))
            .width(Size::fill())
            .height(Size::px(ROW_SIZE - 4.0))
            .child(
                label()
                    .color(TEXT_MUTED)
                    .font_size(11.0)
                    .text(title.to_ascii_uppercase()),
            )
            .into_element(),
        TocEntry::Outline {
            id,
            label: text,
            subtitle,
            icon,
            depth,
            has_children,
            collapsed: is_collapsed,
        } => {
            let on_press = move |_: Event<PressEventData>| {
                match &target {
                    SidebarContents::Board(board) => {
                        let mut guard = lock(board);
                        if let Some((x, y)) = guard.background().navigate_to_outline(id) {
                            let _ = guard.center_on_world(x, y);
                        }
                    }
                    SidebarContents::Plugin { plugin_id, item } => {
                        activate_plugin_outline(plugin_id, *item, id);
                    }
                }
                ModalController::get().close();
            };
            let icon_color = TEXT_PRIMARY;
            let chevron = has_children.then(|| outline_chevron(id, is_collapsed, collapsed));
            toc_card_indented(
                chevron,
                outline_icon_element(&icon, icon_color),
                text,
                subtitle,
                false,
                depth,
                on_press,
            )
        }
        TocEntry::Page {
            label: text,
            world_x,
            world_y,
        } => {
            let press_board = target.board();
            let on_press = move |_: Event<PressEventData>| {
                if let Some(board) = &press_board {
                    let _ = lock(board).center_on_world(world_x, world_y);
                }
                ModalController::get().close();
            };
            toc_card(
                None,
                bundled_icon(file_text(), TEXT_PRIMARY),
                text,
                None,
                false,
                on_press,
            )
        }
        TocEntry::Layer {
            id,
            name,
            stroke_count,
            is_active,
        } => {
            let press_board = target.board();
            let on_press = move |_: Event<PressEventData>| {
                if let Some(board) = &press_board {
                    lock(board).set_active_layer(id);
                }
            };
            let icon_color = if is_active {
                TEXT_INVERSE
            } else {
                TEXT_PRIMARY
            };
            toc_card(
                None,
                bundled_icon(layers(), icon_color),
                name,
                Some(format!("{stroke_count} trazos")),
                is_active,
                on_press,
            )
        }
    }
}

/// Chevron button that toggles the outline node's collapsed state.
/// Lives before the icon on the row. Uses `stop_propagation` so the
/// toggle does not also fire the row's navigate handler.
fn outline_chevron(
    id: freya_canvas_bg::OutlineNodeId,
    is_collapsed: bool,
    mut collapsed: State<HashSet<freya_canvas_bg::OutlineNodeId>>,
) -> Element {
    let icon = if is_collapsed {
        chevron_right()
    } else {
        chevron_down()
    };
    let on_press = move |e: Event<PressEventData>| {
        e.stop_propagation();
        collapsed.with_mut(|mut set| {
            if !set.insert(id) {
                set.remove(&id);
            }
        });
    };
    rect()
        .width(Size::px(20.0))
        .height(Size::px(20.0))
        .center()
        .with_corner_radius(4.0)
        .on_press(on_press)
        .child(
            SvgViewer::new(icon)
                .fill(TEXT_MUTED)
                .width(Size::px(12.))
                .height(Size::px(12.)),
        )
        .into_element()
}

fn bundled_icon(bytes: Bytes, color: Color) -> Element {
    SvgViewer::new(bytes)
        .fill(color)
        .width(Size::px(14.))
        .height(Size::px(14.))
        .into_element()
}

fn toc_card_indented<F>(
    leading: Option<Element>,
    icon: Element,
    title: String,
    subtitle: Option<String>,
    is_active: bool,
    depth: u8,
    on_press: F,
) -> Element
where
    F: FnMut(Event<PressEventData>) + 'static,
{
    let indent = f32::from(depth) * 12.0;
    rect()
        .direction(Direction::Horizontal)
        .padding((0.0, 0.0, 0.0, indent))
        .child(toc_card(
            leading, icon, title, subtitle, is_active, on_press,
        ))
        .into_element()
}

fn toc_card<F>(
    leading: Option<Element>,
    icon: Element,
    title: String,
    subtitle: Option<String>,
    is_active: bool,
    on_press: F,
) -> Element
where
    F: FnMut(Event<PressEventData>) + 'static,
{
    let bg = if is_active { PRIMARY } else { CARD_BG };
    let text = if is_active {
        TEXT_INVERSE
    } else {
        TEXT_PRIMARY
    };
    let sub = if is_active { TEXT_INVERSE } else { TEXT_MUTED };
    let mut body = rect()
        .direction(Direction::Vertical)
        .width(Size::fill())
        .spacing(2.0)
        .child(label().color(text).font_size(13.0).text(title));
    if let Some(sub_text) = subtitle {
        body = body.child(label().color(sub).font_size(11.0).text(sub_text));
    }
    let mut row = rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .spacing(10.0)
        .padding((8.0, 10.0))
        .margin((2.0, 6.0))
        .height(Size::px(ROW_SIZE - 4.0))
        .background(bg)
        .with_corner_radius(6.0)
        .on_press(on_press);
    if let Some(leading) = leading {
        row = row.child(leading);
    }
    row.child(icon).child(body).into_element()
}

// ---------- Bookmarks tab -------------------------------------------------

fn bookmarks_body(
    board: &Arc<Mutex<Board>>,
    query: State<String>,
    sort: State<BookmarkSort>,
    selected_bookmark: State<Option<crate::ids::BookmarkId>>,
    body_buffer: State<String>,
) -> Element {
    let needle = query.read().trim().to_ascii_lowercase();
    let mut entries = {
        let g = lock(board);
        g.bookmarks()
            .iter()
            .filter(|bm| {
                if needle.is_empty() {
                    return true;
                }
                bm.body.to_ascii_lowercase().contains(&needle)
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    match *sort.read() {
        BookmarkSort::Oldest => entries.sort_by_key(|e| e.created_at),
        BookmarkSort::Recent => entries.sort_by_key(|e| std::cmp::Reverse(e.updated_at)),
    }

    if entries.is_empty() {
        return empty_state(if needle.is_empty() {
            "Sin marcadores. Mantén presionado en el lienzo para crear uno."
        } else {
            "Nada coincide con la búsqueda."
        });
    }

    let length = entries.len();
    let board_cell = Arc::clone(board);
    VirtualScrollView::new_with_data(
        entries,
        move |item, data: &Vec<crate::bookmark::Bookmark>| {
            let bm = data[item.index].clone();
            let row_board = Arc::clone(&board_cell);
            let mut selected_setter = selected_bookmark;
            let mut body_setter = body_buffer;
            let bm_id = bm.id;
            let on_press = move |_: Event<PressEventData>| {
                let body = {
                    let mut g = lock(&row_board);
                    // Best-effort viewport jump; `false` is fine if the
                    // surface size is still unknown — the overlay still
                    // opens so the user can read / edit the pin.
                    let _ = g.center_on_bookmark(bm_id);
                    g.doc()
                        .bookmark(bm_id)
                        .map(|b| b.body.clone())
                        .unwrap_or_default()
                };
                body_setter.set(body);
                selected_setter.set(Some(bm_id));
                ModalController::get().close();
            };
            bookmark_row(bm, on_press).into_element()
        },
    )
    .length(length)
    .item_size(ROW_SIZE)
    .into_element()
}

fn bookmark_row<F>(entry: crate::bookmark::Bookmark, on_press: F) -> Element
where
    F: FnMut(Event<PressEventData>) + 'static,
{
    let dot_color = entry
        .color
        .map(|[r, g, b, _]| Color::from_rgb(r, g, b))
        .unwrap_or_else(|| Color::from_rgb(245, 158, 11));
    let preview = crate::components::NoteEditor::plain_text(&entry.body)
        .lines()
        .next()
        .map(|l| {
            let max = l.char_indices().nth(80).map_or(l.len(), |(i, _)| i);
            l[..max].to_owned()
        })
        .unwrap_or_else(|| "(sin texto)".to_owned());
    rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .spacing(10.0)
        .padding((8.0, 10.0))
        .margin((2.0, 6.0))
        .height(Size::px(ROW_SIZE - 4.0))
        .background(CARD_BG)
        .with_corner_radius(6.0)
        .on_press(on_press)
        .child(
            rect()
                .width(Size::px(10.0))
                .height(Size::px(10.0))
                .with_corner_radius(5.0)
                .background(dot_color),
        )
        .child(
            rect()
                .direction(Direction::Vertical)
                .width(Size::fill())
                .spacing(2.0)
                .child(label().color(TEXT_PRIMARY).font_size(13.0).text(preview))
                .child(
                    label()
                        .color(TEXT_MUTED)
                        .font_size(11.0)
                        .text(format!("{} ref(s)", entry.refs.len())),
                ),
        )
        .into_element()
}

// ---------- Shared bits ---------------------------------------------------

fn empty_state(text: &'static str) -> Element {
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .main_align(Alignment::Center)
        .cross_align(Alignment::Center)
        .padding((12.0, 12.0))
        .child(label().color(TEXT_MUTED).font_size(12.0).text(text))
        .into_element()
}
