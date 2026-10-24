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

impl Component for LibraryBar {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let browse = Browse::get();
        let folder = browse.folder();
        let mut settings = SettingsStore::get();
        let library = settings.read().library;
        let mut filter = self.filter;
        let current = filter.read().clone();
        let mut selection = self.selection;
        let selecting = selection.read().active;
        let compact = width_class().is_compact();
        let trash = crate::screens::trash::groups(&self.data).len();
        let path = crumbs(&self.data, folder);
        let last = path.len().saturating_sub(1);
        let breadcrumb = rect()
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
                    notein_ui_kit::Icon::new(lucide::chevron_right())
                        .size(14.0)
                        .color(p.t3)
                        .into_element()
                });
                sep.into_iter().chain(std::iter::once(crumb))
            }));
        let chips = filter_chips().into_iter().map(|chip| {
            let on = chip.filter == current;
            let tint = match &chip.filter {
                Filter::Kind(kind) => {
                    crate::plugins::kind_info(kind).map(|k| kind_color(&p, k.color))
                }
                Filter::All | Filter::Folders => None,
            };
            let value = chip.filter.clone();
            let tag = Tag::new(chip.label).active(on);
            let tag = match tint {
                Some(c) => tag.dot(c),
                None => tag,
            };
            tag.on_press(move |_| filter.set(value.clone()))
                .into_element()
        });
        let sort_button = {
            let sort = library.sort;
            Button::new(sort.label())
                .ghost()
                .small()
                .icon(lucide::arrow_down_wide_narrow())
                .on_press(move |e: Event<PressEventData>| {
                    let at = match e.data() {
                        PressEventData::Mouse(m) => m.global_location,
                        _ => CursorPoint::new(0.0, 0.0),
                    };
                    #[allow(clippy::cast_possible_truncation)] // window coordinates
                    let anchor = Area::new((at.x as f32, at.y as f32).into(), (1.0, 1.0).into());
                    let entries = SortOrder::ALL.iter().map(|order| {
                        let order = *order;
                        ActionEntry::Item(
                            notein_ui_kit::Action::new(order.label(), move |()| {
                                notein_ui_kit::ModalController::get().close();
                                let mut settings = settings;
                                settings.update(|s| s.library.sort = order);
                            })
                            .checked(order == sort),
                        )
                    });
                    Popover::new(anchor, notein_ui_kit::ActionList::new(entries).width(200.0))
                        .width(200.0)
                        .side(PopoverSide::Below)
                        .align(PopoverAlign::End)
                        .sheet_title("Ordenar por")
                        .open();
                })
        };
        let layout_toggle = {
            let as_list = library.layout == ShelfLayout::List;
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
            .on_press(move |_| {
                settings.update(|s| {
                    s.library.layout = if as_list {
                        ShelfLayout::Grid
                    } else {
                        ShelfLayout::List
                    };
                });
            })
        };
        let trash_button = IconButton::new(lucide::trash_2(), "Papelera")
            .badge(u32::try_from(trash).unwrap_or(u32::MAX))
            .on_press(|_| crate::screens::trash::open());
        let select_button = Button::new(if selecting { "Listo" } else { "Seleccionar" })
            .ghost()
            .small()
            .on_press(move |_| {
                let mut s = selection.write();
                if s.active {
                    s.clear();
                } else {
                    s.active = true;
                }
            });
        let tools = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(4.0)
            .child(sort_button)
            .child(layout_toggle)
            .child(trash_button)
            .child(select_button);
        let head = if compact {
            rect()
                .width(Size::fill())
                .spacing(8.0)
                .child(breadcrumb)
                .child(tools)
        } else {
            rect()
                .width(Size::fill())
                .horizontal()
                .cross_align(Alignment::Center)
                .content(Content::Flex)
                .child(rect().width(Size::flex(1.0)).child(breadcrumb))
                .child(tools)
        };
        rect().width(Size::fill()).spacing(10.0).child(head).child(
            ScrollView::new()
                .direction(Direction::Horizontal)
                .width(Size::fill())
                .height(Size::px(36.0))
                .show_scrollbar(false)
                .child(
                    rect()
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(6.0)
                        .children(chips),
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

/// Folder and document cards in columns.
#[derive(Clone, PartialEq)]
pub struct Grid {
    pub data: Arc<LibraryData>,
    pub shelf: Arc<Shelf>,
    pub columns: Columns,
    pub selection: State<Selection>,
}

impl std::fmt::Debug for Grid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grid")
            .field("columns", &self.columns)
            .finish_non_exhaustive()
    }
}

impl Grid {
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

impl Component for Grid {
    fn render(&self) -> impl IntoElement {
        let state = self.selection.read().clone();
        let cards: Vec<Element> = self
            .shelf
            .folders
            .iter()
            .map(|f| self.folder_card(f, &state))
            .chain(self.shelf.docs.iter().map(|d| self.doc_card(d, &state)))
            .collect();
        rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::wrap_spacing(self.columns.gap + 8.0))
            .spacing(self.columns.gap)
            .children(cards)
    }
}

/// Documents as rows (the list layout).
#[derive(Clone, PartialEq)]
pub struct Rows {
    pub data: Arc<LibraryData>,
    pub shelf: Arc<Shelf>,
    pub selection: State<Selection>,
}

impl std::fmt::Debug for Rows {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rows").finish_non_exhaustive()
    }
}

impl Component for Rows {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let state = self.selection.read().clone();
        let selection = self.selection;
        let folders = self.shelf.folders.iter().map(|f| {
            let id = f.id;
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
                Selecting::Selected | Selecting::Unselected => row.leading(
                    notein_ui_kit::Leading::Check(checked == Selecting::Selected),
                ),
            }
            .into_element()
        });
        let docs = self.shelf.docs.iter().map(|d| {
            let id = d.id;
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
                Selecting::Selected | Selecting::Unselected => row.leading(
                    notein_ui_kit::Leading::Check(checked == Selecting::Selected),
                ),
            }
            .into_element()
        });
        notein_ui_kit::Group::default().children(folders.chain(docs).collect::<Vec<_>>())
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
                                selection: Vec::new(),
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
            .child(Text::new("FIJADOS").role(TextRole::Label).color(p.t3))
            .child(
                ScrollView::new()
                    .direction(Direction::Horizontal)
                    .width(Size::fill())
                    .height(Size::px(
                        (self.card * super::card::COVER_RATIO).round() + 64.0,
                    ))
                    .show_scrollbar(false)
                    .child(rect().horizontal().spacing(12.0).children(cards)),
            )
    }
}
