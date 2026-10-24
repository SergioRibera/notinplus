//! The library part of Home: where you are, how it is filtered and
//! sorted, and the documents themselves as cards or rows.

use std::sync::Arc;

use freya::prelude::*;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    ActionEntry, Button, EmptyState, IconButton, ListRow, Popover, PopoverAlign, PopoverSide, Tag,
    Text, TextRole, tokens, width_class,
};

use super::Browse;
use super::card::{CardActions, DocCard, FolderCard, Selecting};
use super::selection::Selection;
use super::shelf::{DocEntry, Filter, FolderEntry, Shelf, filter_chips};
use crate::library::{FolderId, ItemId, LibraryData, ROOT_FOLDER};
use crate::references::kind_color;
use crate::settings::{SettingsStore, ShelfLayout, SortOrder};

/// Folders from the root down to `folder`.
fn crumbs(data: &LibraryData, folder: FolderId) -> Vec<(FolderId, String)> {
    let mut out = Vec::new();
    let mut cursor = folder;
    while cursor != ROOT_FOLDER {
        let Some(f) = data.folder(cursor) else {
            break;
        };
        out.push((f.id, f.name.clone()));
        cursor = f.parent;
        if out.len() > 32 {
            break;
        }
    }
    out.push((ROOT_FOLDER, "Biblioteca".to_owned()));
    out.reverse();
    out
}

/// Breadcrumb, filters, sort, layout, trash and Seleccionar.
#[derive(Clone, PartialEq)]
pub struct LibraryBar {
    pub data: Arc<LibraryData>,
    pub filter: State<Filter>,
    pub selection: State<Selection>,
}

impl std::fmt::Debug for LibraryBar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibraryBar").finish_non_exhaustive()
    }
}

/// Where you are; every folder above is a link.
fn breadcrumb(data: &LibraryData, browse: Browse) -> Rect {
    let p = palette();
    let path = crumbs(data, browse.folder());
    let last = path.len().saturating_sub(1);
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(4.0)
        .children(path.into_iter().enumerate().flat_map(|(i, (id, name))| {
            let here = i == last;
            let crumb = rect()
                .key(id.0)
                .padding((4.0, 6.0))
                .corner_radius(tokens::radius::XS)
                .a11y_role(AccessibilityRole::Link)
                .maybe(!here, |r| r.on_press(move |_| browse.enter(id)))
                .child(
                    Text::new(name)
                        .role(if here {
                            TextRole::Heading
                        } else {
                            TextRole::Body
                        })
                        .color(if here { p.t1 } else { p.t2 })
                        .lines(1),
                )
                .into_element();
            let sep = (i > 0).then(|| {
                notein_ui_kit::Icon::new(freya::icons::lucide::chevron_right())
                    .size(14.0)
                    .color(p.t3)
                    .into_element()
            });
            sep.into_iter().chain(std::iter::once(crumb))
        }))
}

/// One chip per filter, kinds with their colour.
fn chips(mut filter: State<Filter>) -> Vec<Element> {
    let p = palette();
    let current = filter.read().clone();
    filter_chips()
        .into_iter()
        .map(|chip| {
            let tint = match &chip.filter {
                Filter::Kind(kind) => {
                    crate::plugins::kind_info(kind).map(|k| kind_color(&p, k.color))
                }
                Filter::All | Filter::Folders => None,
            };
            let tag = Tag::new(chip.label).active(chip.filter == current);
            let tag = match tint {
                Some(c) => tag.dot(c),
                None => tag,
            };
            let value = chip.filter;
            tag.on_press(move |_| filter.set(value.clone()))
                .into_element()
        })
        .collect()
}

/// The sort menu, hanging from the button.
fn sort_button(sort: SortOrder, mut area: State<Area>) -> Rect {
    let button = Button::new(sort.label())
        .ghost()
        .small()
        .icon(freya::icons::lucide::arrow_down_wide_narrow())
        .on_press(move |_| {
            let anchor = *area.peek();
            let entries = SortOrder::ALL.iter().map(|order| {
                let order = *order;
                ActionEntry::Item(
                    notein_ui_kit::Action::new(order.label(), move |()| {
                        notein_ui_kit::ModalController::get().close();
                        SettingsStore::get().update(|s| s.library.sort = order);
                    })
                    .checked(order == sort),
                )
            });
            Popover::new(anchor, notein_ui_kit::ActionList::new(entries).width(200.0))
                .width(200.0)
                .estimated_height(170.0)
                .side(PopoverSide::Below)
                .align(PopoverAlign::End)
                .sheet_title("Ordenar por")
                .open();
        });
    rect()
        .on_sized(move |e: Event<SizedEventData>| area.set_if_modified(e.area))
        .child(button)
}

/// Sort, grid or list, and Seleccionar, all one height.
fn tools(
    layout: ShelfLayout,
    sort: SortOrder,
    sort_area: State<Area>,
    mut selection: State<Selection>,
) -> Rect {
    use freya::icons::lucide;
    let as_list = layout == ShelfLayout::List;
    let selecting = selection.read().active;
    rect()
        .horizontal()
        .height(Size::px(TOOL))
        .cross_align(Alignment::Center)
        .spacing(4.0)
        .child(sort_button(sort, sort_area))
        .child(
            IconButton::new(
                if as_list {
                    lucide::layout_grid()
                } else {
                    lucide::list()
                },
                if as_list {
                    "Ver como cuadrícula"
                } else {
                    "Ver como lista"
                },
            )
            .size(TOOL)
            .on_press(move |_| {
                SettingsStore::get().update(|s| {
                    s.library.layout = if as_list {
                        ShelfLayout::Grid
                    } else {
                        ShelfLayout::List
                    };
                });
            }),
        )
        .child(
            Button::new(if selecting { "Listo" } else { "Seleccionar" })
                .ghost()
                .small()
                .on_press(move |_| {
                    let mut s = selection.write();
                    if s.active {
                        s.clear();
                    } else {
                        s.active = true;
                    }
                }),
        )
}

/// Height of the toolbar's controls.
const TOOL: f32 = tokens::size::BUTTON_SM;
/// Height of the row holding the breadcrumb.
const CRUMBS: f32 = 40.0;
/// Height of the filter chips.
const CHIPS: f32 = 36.0;

impl LibraryBar {
    /// The bar's height: one row with the breadcrumb and the tools, or
    /// two on phones, then the chips.
    #[must_use]
    pub const fn height(compact: bool) -> f32 {
        if compact {
            CRUMBS + 8.0 + TOOL + 10.0 + CHIPS
        } else {
            CRUMBS + 10.0 + CHIPS
        }
    }
}

impl Component for LibraryBar {
    fn render(&self) -> impl IntoElement {
        let library = SettingsStore::get().read().library;
        let sort_area = use_state(Area::default);
        let compact = width_class().is_compact();
        let breadcrumb = rect()
            .height(Size::px(CRUMBS))
            .cross_align(Alignment::Center)
            .horizontal()
            .child(breadcrumb(&self.data, Browse::get()));
        let tools = tools(library.layout, library.sort, sort_area, self.selection);
        let head = if compact {
            rect()
                .width(Size::fill())
                .spacing(8.0)
                .child(breadcrumb)
                .child(tools)
        } else {
            rect()
                .width(Size::fill())
                .height(Size::px(CRUMBS))
                .horizontal()
                .cross_align(Alignment::Center)
                .content(Content::Flex)
                .child(rect().width(Size::flex(1.0)).child(breadcrumb))
                .child(tools)
        };
        rect()
            .width(Size::fill())
            .height(Size::px(Self::height(compact)))
            .spacing(10.0)
            .child(head)
            .child(
                ScrollView::new()
                    .direction(Direction::Horizontal)
                    .width(Size::fill())
                    .height(Size::px(CHIPS))
                    .show_scrollbar(false)
                    .child(
                        rect()
                            .horizontal()
                            .height(Size::px(CHIPS))
                            .cross_align(Alignment::Center)
                            .spacing(6.0)
                            .children(chips(self.filter)),
                    ),
            )
    }
}

/// How cards fit the available width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Columns {
    pub count: usize,
    pub card: f32,
    pub gap: f32,
}

impl Columns {
    /// Columns for a content area `width` px wide.
    #[must_use]
    pub fn fit(width: f32) -> Self {
        let gap = if width < 600.0 { 12.0 } else { 20.0 };
        let min = if width < 600.0 {
            150.0
        } else {
            tokens::size::CARD_WIDTH
        };
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )] // small positive counts
        let count = (((width + gap) / (min + gap)).floor() as usize).max(2);
        #[allow(clippy::cast_precision_loss)]
        let card = ((width - gap * (count as f32 - 1.0)) / count as f32).floor();
        Self { count, card, gap }
    }
}

fn open_item(selection: State<Selection>, id: ItemId) {
    let mut selection = selection;
    if selection.peek().active {
        selection.write().toggle_item(id);
    } else {
        crate::flows::open(id);
    }
}

fn open_folder(selection: State<Selection>, id: FolderId) {
    let mut selection = selection;
    if selection.peek().active {
        selection.write().toggle_folder(id);
    } else {
        Browse::get().enter(id);
    }
}

fn doc_menu(
    data: &Arc<LibraryData>,
    selection: State<Selection>,
    id: ItemId,
) -> impl Fn(CursorPoint) + 'static {
    let data = Arc::clone(data);
    move |at| {
        if selection.peek().active {
            let mut selection = selection;
            selection.write().toggle_item(id);
            return;
        }
        let mut entries = crate::actions::for_item(&data, id, crate::actions::Origin::Library);
        entries.insert(
            0,
            ActionEntry::Item(
                notein_ui_kit::Action::new("Seleccionar", move |()| {
                    notein_ui_kit::ModalController::get().close();
                    let mut selection = selection;
                    selection.write().start_with_item(id);
                })
                .icon(freya::icons::lucide::square_check()),
            ),
        );
        crate::actions::show(entries, at);
    }
}

/// One row of folder and document cards, the `row`-th of the shelf.
#[derive(Clone, PartialEq)]
pub struct GridRow {
    pub data: Arc<LibraryData>,
    pub shelf: Arc<Shelf>,
    pub columns: Columns,
    pub selection: State<Selection>,
    pub row: usize,
}

impl std::fmt::Debug for GridRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GridRow")
            .field("row", &self.row)
            .finish_non_exhaustive()
    }
}

impl GridRow {
    /// Rows the shelf takes with `columns` cards across.
    #[must_use]
    pub fn count(shelf: &Shelf, columns: Columns) -> usize {
        (shelf.folders.len() + shelf.docs.len()).div_ceil(columns.count)
    }

    /// Height of a row, with the gap under it.
    #[must_use]
    pub fn height(columns: Columns) -> f32 {
        super::card::card_height(columns.card) + columns.gap
    }

    fn folder_card(&self, entry: &FolderEntry, state: &Selection) -> Element {
        let id = entry.id;
        let selection = self.selection;
        FolderCard {
            data: entry.card.clone(),
            width: self.columns.card,
            selecting: state.of_folder(id),
            drop_target: false,
            actions: CardActions {
                on_open: EventHandler::new(move |()| open_folder(selection, id)),
                on_menu: EventHandler::new(move |at| {
                    if selection.peek().active {
                        let mut selection = selection;
                        selection.write().toggle_folder(id);
                    } else {
                        crate::actions::show(crate::actions::for_folder(id), at);
                    }
                }),
            },
        }
        .into_element()
    }

    fn doc_card(&self, entry: &DocEntry, state: &Selection) -> Element {
        let id = entry.id;
        let selection = self.selection;
        DocCard {
            data: entry.card.clone(),
            width: self.columns.card,
            selecting: state.of_item(id),
            actions: CardActions {
                on_open: EventHandler::new(move |()| open_item(selection, id)),
                on_menu: EventHandler::new(doc_menu(&self.data, selection, id)),
            },
        }
        .into_element()
    }
}

impl Component for GridRow {
    fn render(&self) -> impl IntoElement {
        let state = self.selection.read().clone();
        let start = self.row * self.columns.count;
        let folders = self.shelf.folders.len();
        let cards: Vec<Element> = (start..start + self.columns.count)
            .filter_map(|at| {
                if at < folders {
                    Some(self.folder_card(&self.shelf.folders[at], &state))
                } else {
                    self.shelf
                        .docs
                        .get(at - folders)
                        .map(|d| self.doc_card(d, &state))
                }
            })
            .collect();
        rect()
            .width(Size::fill())
            .horizontal()
            .spacing(self.columns.gap)
            .children(cards)
    }
}

/// One folder or document of the shelf as a row (the list layout).
#[derive(Clone, PartialEq)]
pub struct ShelfRow {
    pub data: Arc<LibraryData>,
    pub shelf: Arc<Shelf>,
    pub selection: State<Selection>,
    pub index: usize,
}

impl std::fmt::Debug for ShelfRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShelfRow")
            .field("index", &self.index)
            .finish_non_exhaustive()
    }
}

impl ShelfRow {
    /// Height of a row.
    pub const HEIGHT: f32 = 60.0;

    /// Rows the shelf takes.
    #[must_use]
    pub fn count(shelf: &Shelf) -> usize {
        shelf.folders.len() + shelf.docs.len()
    }

    fn folder_row(&self, f: &FolderEntry, state: &Selection) -> ListRow {
        use freya::icons::lucide;
        let p = palette();
        let id = f.id;
        let selection = self.selection;
        let checked = state.of_folder(id);
        let row = ListRow::new(f.card.name.clone())
            .subtitle(crate::format::count(f.card.count, "elemento", "elementos"))
            .kind(lucide::folder(), p.kinds.folder)
            .on_press(move |_| open_folder(selection, id))
            .on_secondary(move |e: Event<PressEventData>| {
                if let PressEventData::Mouse(m) = e.data() {
                    crate::actions::show(crate::actions::for_folder(id), m.global_location);
                }
            });
        match checked {
            Selecting::No => row.chevron(),
            Selecting::Selected | Selecting::Unselected => {
                row.leading(notein_ui_kit::Leading::Check(checked == Selecting::Selected))
            }
        }
    }

    fn doc_row(&self, d: &DocEntry, state: &Selection) -> ListRow {
        use freya::icons::lucide;
        let p = palette();
        let id = d.id;
        let selection = self.selection;
        let checked = state.of_item(id);
        let info = crate::plugins::kind_info(&d.card.kind);
        let icon = info.as_ref().map_or_else(lucide::file, |k| k.icon.clone());
        let tint = info.as_ref().map_or(p.t3, |k| kind_color(&p, k.color));
        let menu = doc_menu(&self.data, selection, id);
        let row = ListRow::new(d.card.name.clone())
            .subtitle(d.card.meta.clone())
            .on_press(move |_| open_item(selection, id))
            .on_secondary(move |e: Event<PressEventData>| {
                if let PressEventData::Mouse(m) = e.data() {
                    menu(m.global_location);
                }
            });
        match checked {
            Selecting::No => row.kind(icon, tint),
            Selecting::Selected | Selecting::Unselected => {
                row.leading(notein_ui_kit::Leading::Check(checked == Selecting::Selected))
            }
        }
    }
}

impl Component for ShelfRow {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let state = self.selection.read().clone();
        let folders = self.shelf.folders.len();
        let last = Self::count(&self.shelf).saturating_sub(1);
        let row = if self.index < folders {
            self.folder_row(&self.shelf.folders[self.index], &state)
        } else {
            match self.shelf.docs.get(self.index - folders) {
                Some(d) => self.doc_row(d, &state),
                None => return rect().into_element(),
            }
        };
        // The rows of one shelf read as a single rounded group.
        let (top, bottom) = (
            if self.index == 0 { tokens::radius::XL } else { 0.0 },
            if self.index == last { tokens::radius::XL } else { 0.0 },
        );
        rect()
            .width(Size::fill())
            .height(Size::px(Self::HEIGHT))
            .padding((
                if self.index == 0 { 4.0 } else { 0.0 },
                4.0,
                if self.index == last { 4.0 } else { 0.0 },
                4.0,
            ))
            .main_align(Alignment::Center)
            .background(p.s1)
            .corner_radius(CornerRadius::new(top, top, bottom, bottom))
            .child(row)
            .into_element()
    }
}

/// An empty folder or a filter with nothing in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmptyShelf {
    pub folder: FolderId,
    pub filtered: bool,
}

impl Component for EmptyShelf {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let folder = self.folder;
        let (title, body) = match (self.filtered, folder == ROOT_FOLDER) {
            (true, _) => (
                "Nada de este tipo acá",
                "Probá con «Todo» o creá uno nuevo con el botón +.",
            ),
            (false, true) => (
                "Tu biblioteca está vacía",
                "Creá un lienzo para escribir a mano, importá un PDF o abrí la Biblia.",
            ),
            (false, false) => (
                "Esta carpeta está vacía",
                "Creá algo adentro o mové documentos acá desde su menú.",
            ),
        };
        EmptyState::new(lucide::library(), title)
            .body(body)
            .dashed()
            .action(
                Button::new("Crear")
                    .primary()
                    .icon(lucide::plus())
                    .on_press(move |e: Event<PressEventData>| {
                        let at = match e.data() {
                            PressEventData::Mouse(m) => m.global_location,
                            _ => CursorPoint::new(0.0, 0.0),
                        };
                        #[allow(clippy::cast_possible_truncation)] // window coordinates
                        let anchor =
                            Area::new((at.x as f32, at.y as f32).into(), (1.0, 1.0).into());
                        crate::flows::create::open_menu(
                            anchor,
                            crate::flows::create::Context {
                                folder,
                                ..Default::default()
                            },
                        );
                    }),
            )
    }
}

/// Pinned documents, in their order, above the library.
#[derive(Clone, PartialEq)]
pub struct PinnedRow {
    pub data: Arc<LibraryData>,
    pub card: f32,
    pub selection: State<Selection>,
}

impl std::fmt::Debug for PinnedRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinnedRow").finish_non_exhaustive()
    }
}

/// Height of the label over the pinned cards.
const PINNED_LABEL: f32 = 16.0;

impl PinnedRow {
    /// Height of the row for cards `card` px wide.
    #[must_use]
    pub fn height(card: f32) -> f32 {
        PINNED_LABEL + 8.0 + super::card::card_height(card)
    }
}

impl Component for PinnedRow {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let now = crate::format::now();
        let state = self.selection.read().clone();
        let selection = self.selection;
        let pinned = self.data.pinned();
        let last = pinned.len().saturating_sub(1);
        let shared: Vec<ItemId> = crate::sync::snapshot()
            .shares
            .into_iter()
            .flat_map(|s| s.docs)
            .collect();
        let cards: Vec<Element> = pinned
            .iter()
            .enumerate()
            .filter_map(|(index, id)| self.data.item(*id).map(|item| (index, item)))
            .map(|(index, item)| {
                let id = item.id;
                let data = Arc::clone(&self.data);
                let base_menu = doc_menu(&self.data, selection, id);
                DocCard {
                    data: super::shelf::card(&self.data, item, shared.contains(&id), now),
                    width: self.card,
                    selecting: state.of_item(id),
                    actions: CardActions {
                        on_open: EventHandler::new(move |()| open_item(selection, id)),
                        on_menu: EventHandler::new(move |at| {
                            if selection.peek().active {
                                base_menu(at);
                                return;
                            }
                            let mut entries = crate::actions::for_item(
                                &data,
                                id,
                                crate::actions::Origin::Library,
                            );
                            let moves = [
                                (index > 0).then(|| ("Mover a la izquierda", index - 1)),
                                (index < last).then(|| ("Mover a la derecha", index + 1)),
                            ];
                            for (label, to) in moves.into_iter().flatten().rev() {
                                entries.insert(
                                    0,
                                    ActionEntry::Item(
                                        notein_ui_kit::Action::new(label, move |()| {
                                            notein_ui_kit::ModalController::get().close();
                                            crate::state::edit(
                                                "reordenar los fijados",
                                                async move |lib| lib.move_pin(index, to).await,
                                            );
                                        })
                                        .icon(
                                            if to < index {
                                                freya::icons::lucide::arrow_left()
                                            } else {
                                                freya::icons::lucide::arrow_right()
                                            },
                                        ),
                                    ),
                                );
                            }
                            crate::actions::show(entries, at);
                        }),
                    },
                }
                .into_element()
            })
            .collect();
        rect()
            .width(Size::fill())
            .spacing(8.0)
            .child(
                rect()
                    .height(Size::px(PINNED_LABEL))
                    .child(Text::new("FIJADOS").role(TextRole::Label).color(p.t3)),
            )
            .child(
                ScrollView::new()
                    .direction(Direction::Horizontal)
                    .width(Size::fill())
                    .height(Size::px(super::card::card_height(self.card)))
                    .show_scrollbar(false)
                    .child(rect().horizontal().spacing(12.0).children(cards)),
            )
    }
}
