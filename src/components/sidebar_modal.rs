//! Slide-in left sidebar modal with three tabs:
//!
//! * **Docs** — library-wide item list with name + tag search; tapping a
//!   row pushes the appropriate `Route` for the item's `ItemKind`.
//! * **Contents** — table-of-contents of the currently open doc. Backed
//!   by `Board::layers_snapshot` as a universal fallback; PDF outlines
//!   and plugin-specific TOCs are a follow-up (no plugin TOC trait
//!   exists today).
//! * **Bookmarks** — the pre-rewrite behaviour, now navigable: a tap
//!   centres the viewport on the pin and surfaces the edit sheet.
//!
//! Every tab's body is a `VirtualScrollView` with a fixed `item_size` so
//! long libraries / long books do not materialise N rects per render.

use std::sync::{Arc, Mutex};

use freya::animation::{AnimNum, Ease, Function, OnCreation, use_animation};
use freya::icons::lucide::{bookmark, file_text, layers, search};
use freya::prelude::*;
use freya::router::*;
use istmo::plugins::EdgeInsets;

use crate::app::ChromeState;
use crate::bookmark_ui::BookmarkSort;
use crate::canvas::{Board, lock};
use crate::library::{Item, ItemId, ItemKind, Tag, TagId};
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
    pad: EdgeInsets,
    state: ChromeState,
    board: Arc<Mutex<Board>>,
}

impl PartialEq for SidebarModal {
    fn eq(&self, other: &Self) -> bool {
        self.pad == other.pad && self.state == other.state
    }
}

impl SidebarModal {
    pub(crate) fn open(pad: EdgeInsets, state: ChromeState, board: Arc<Mutex<Board>>) {
        ModalController::get().open(
            Modal::new(Self { pad, state, board })
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
    kind: ItemKind,
    name: String,
    color: Option<[u8; 4]>,
    tags: Vec<String>,
}

#[derive(Clone, Default, PartialEq)]
struct LibrarySnapshot {
    docs: Vec<DocEntry>,
}

impl Component for SidebarModal {
    fn render(&self) -> impl freya::prelude::IntoElement {
        let pad = self.pad;
        let board = &self.board;
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

        let header = sidebar_header(current_tab, move |next| tab_state.set(next));

        let body = match current_tab {
            SidebarTab::Docs => docs_body(library_snapshot, query),
            SidebarTab::Contents => contents_body(board),
            SidebarTab::Bookmarks => {
                bookmarks_body(board, query, sort, selected_bookmark, body_buffer)
            }
        };

        let mut shell = rect()
            .position(Position::new_absolute().top(pad.top).left(left_pos))
            .width(Size::px(SIDEBAR_WIDTH))
            .height(Size::percent(100.))
            .background(PANEL_BG)
            .overflow(Overflow::Clip)
            .color(TEXT_PRIMARY)
            .direction(Direction::Vertical)
            .content(Content::Flex)
            .child(header);
        if !matches!(current_tab, SidebarTab::Contents) {
            shell = shell.child(search_input(query, current_tab, sort));
        }
        shell.child(
            rect()
                .width(Size::fill())
                .height(Size::fill())
                .child(body),
        )
    }
}

// ---------- Header / tab bar ----------------------------------------------

fn sidebar_header(
    current: SidebarTab,
    on_pick: impl FnMut(SidebarTab) + Clone + 'static,
) -> Rect {
    let tabs = [
        (SidebarTab::Docs, file_text()),
        (SidebarTab::Contents, layers()),
        (SidebarTab::Bookmarks, bookmark()),
    ];
    let mut row = rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .main_align(Alignment::SpaceBetween)
        .width(Size::fill())
        .padding((8.0, 10.0))
        .background(SIDEBAR_BG)
        .spacing(6.0);
    for (tab, icon) in tabs {
        let mut picker = on_pick.clone();
        row = row.child(tab_chip(tab, icon, current == tab, move || picker(tab)));
    }
    row
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
    rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .spacing(6.0)
        .padding((6.0, 10.0))
        .with_corner_radius(8.0)
        .background(bg)
        .on_press(move |_: Event<PressEventData>| on_press())
        .child(
            SvgViewer::new(icon)
                .fill(glyph)
                .width(Size::px(14.))
                .height(Size::px(14.)),
        )
        .child(label().color(glyph).font_size(12.0).text(tab.label()))
        .into_element()
}

fn search_input(
    query: State<String>,
    tab: SidebarTab,
    sort: State<BookmarkSort>,
) -> Element {
    let placeholder = match tab {
        SidebarTab::Docs => "Buscar por nombre o etiqueta",
        SidebarTab::Bookmarks => "Buscar marcadores",
        SidebarTab::Contents => "",
    };
    let mut row = rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .width(Size::fill())
        .padding((6.0, 8.0))
        .spacing(6.0)
        .child(
            FormInput::new(query)
                .left_icon(search())
                .placeholder(placeholder)
                .width(Size::fill()),
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
                        kind: it.kind,
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

fn docs_body(snapshot: State<LibrarySnapshot>, query: State<String>) -> Element {
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
                || e.tags.iter().any(|t| t.to_ascii_lowercase().contains(&needle))
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
        doc_row(entry).into_element()
    })
    .length(length)
    .item_size(ROW_SIZE)
    .into_element()
}

fn doc_row(entry: DocEntry) -> Element {
    let DocEntry {
        id,
        kind,
        name,
        color,
        tags,
    } = entry;
    let dot_color = color
        .map(|[r, g, b, _]| Color::from_rgb(r, g, b))
        .unwrap_or(TEXT_MUTED);
    let subtitle = if tags.is_empty() {
        kind_label(kind).to_owned()
    } else {
        format!("{} · {}", kind_label(kind), tags.join(", "))
    };
    let on_press = move |_: Event<PressEventData>| {
        let route = route_for_kind(kind, id);
        let _ = RouterContext::get().push(route);
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
                .child(label().color(TEXT_PRIMARY).font_size(13.0).text(title))
                .child(label().color(TEXT_MUTED).font_size(11.0).text(subtitle)),
        )
        .into_element()
}

const fn kind_label(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Canvas => "Lienzo",
        ItemKind::PdfCanvas => "PDF + lienzo",
        ItemKind::SourcePdf => "PDF",
        ItemKind::DocLinked => "Bundle",
    }
}

fn route_for_kind(kind: ItemKind, id: ItemId) -> Route {
    match kind {
        ItemKind::Canvas => Route::CanvasView { id },
        ItemKind::PdfCanvas => Route::CanvasPdfView { id },
        ItemKind::SourcePdf => Route::SourcePdfView { id },
        ItemKind::DocLinked => Route::DocLinkedView { id },
    }
}

// ---------- Contents tab --------------------------------------------------

/// Flat TOC entry rendered in the Contents list. The shell builds the
/// list from two Board-side sources — pagination comes from the active
/// `CanvasBackground::pages()` (PDF-outline fallback until a plugin
/// authors its own TOC), layers come from `Board::layers_snapshot()`.
#[derive(Clone, PartialEq)]
enum TocEntry {
    /// Visual-only separator rendered as a muted label.
    Section(&'static str),
    /// Jump the viewport to a world-space point.
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

fn collect_toc(board: &Arc<Mutex<Board>>) -> Vec<TocEntry> {
    let g = lock(board);
    let mut out = Vec::new();
    let pages = g.background().pages();
    if !pages.is_empty() {
        out.push(TocEntry::Section("Páginas"));
        for (i, page) in pages.iter().enumerate() {
            let cx = (page.rect.min_x + page.rect.max_x) * 0.5;
            let cy = (page.rect.min_y + page.rect.max_y) * 0.5;
            out.push(TocEntry::Page {
                label: format!("Página {}", i + 1),
                world_x: cx,
                world_y: cy,
            });
        }
    }
    let layers = g.layers_snapshot();
    if !layers.is_empty() {
        let active_id = g.active_layer_id();
        out.push(TocEntry::Section("Capas"));
        // Top-first display matches the floating layer panel elsewhere.
        for layer in layers.into_iter().rev() {
            out.push(TocEntry::Layer {
                id: layer.id,
                name: layer.name,
                stroke_count: layer.stroke_count,
                is_active: layer.id == active_id,
            });
        }
    }
    out
}

fn contents_body(board: &Arc<Mutex<Board>>) -> Element {
    let entries = collect_toc(board);
    if entries.is_empty() {
        return empty_state("El documento no tiene contenido todavía.");
    }

    let length = entries.len();
    let board_cell = Arc::clone(board);
    VirtualScrollView::new_with_data(entries, move |item, data: &Vec<TocEntry>| {
        let entry = data[item.index].clone();
        toc_row(entry, Arc::clone(&board_cell)).into_element()
    })
    .length(length)
    .item_size(ROW_SIZE)
    .into_element()
}

fn toc_row(entry: TocEntry, board: Arc<Mutex<Board>>) -> Element {
    match entry {
        TocEntry::Section(title) => rect()
            .direction(Direction::Horizontal)
            .cross_align(Alignment::Center)
            .padding((6.0, 10.0))
            .margin((2.0, 6.0))
            .height(Size::px(ROW_SIZE - 4.0))
            .child(
                label()
                    .color(TEXT_MUTED)
                    .font_size(11.0)
                    .text(title.to_ascii_uppercase()),
            )
            .into_element(),
        TocEntry::Page {
            label: text,
            world_x,
            world_y,
        } => {
            let press_board = Arc::clone(&board);
            let on_press = move |_: Event<PressEventData>| {
                let _ = lock(&press_board).center_on_world(world_x, world_y);
                ModalController::get().close();
            };
            toc_card(file_text(), text, None, false, on_press)
        }
        TocEntry::Layer {
            id,
            name,
            stroke_count,
            is_active,
        } => {
            let press_board = Arc::clone(&board);
            let on_press = move |_: Event<PressEventData>| {
                lock(&press_board).set_active_layer(id);
            };
            toc_card(
                layers(),
                name,
                Some(format!("{stroke_count} trazos")),
                is_active,
                on_press,
            )
        }
    }
}

fn toc_card<F>(
    icon: Bytes,
    title: String,
    subtitle: Option<String>,
    is_active: bool,
    on_press: F,
) -> Element
where
    F: FnMut(Event<PressEventData>) + 'static,
{
    let bg = if is_active { PRIMARY } else { CARD_BG };
    let text = if is_active { TEXT_INVERSE } else { TEXT_PRIMARY };
    let sub = if is_active { TEXT_INVERSE } else { TEXT_MUTED };
    let mut body = rect()
        .direction(Direction::Vertical)
        .width(Size::fill())
        .spacing(2.0)
        .child(label().color(text).font_size(13.0).text(title));
    if let Some(sub_text) = subtitle {
        body = body.child(label().color(sub).font_size(11.0).text(sub_text));
    }
    rect()
        .direction(Direction::Horizontal)
        .cross_align(Alignment::Center)
        .spacing(10.0)
        .padding((8.0, 10.0))
        .margin((2.0, 6.0))
        .height(Size::px(ROW_SIZE - 4.0))
        .background(bg)
        .with_corner_radius(6.0)
        .on_press(on_press)
        .child(
            SvgViewer::new(icon)
                .fill(text)
                .width(Size::px(14.))
                .height(Size::px(14.)),
        )
        .child(body)
        .into_element()
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
    let preview = entry
        .body
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
