//! Home: what to continue, the mesas, the pinned documents and the
//! library.

pub mod app_bar;
pub mod card;
pub mod fab;
pub mod library;
pub mod mesas;
pub mod selection;
pub mod shelf;

use std::sync::Arc;

use freya::prelude::*;
use freya::router::RouterContext;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{Button, EmptyState, Skeleton, WidthClass, tokens, width_class};

use self::app_bar::AppBar;
use self::fab::Fab;
use self::library::{Columns, EmptyShelf, GridRow, LibraryBar, PinnedRow, ShelfRow};
use self::mesas::{MESAS_BLOCK, MesaStrip};
use self::selection::{Selection, SelectionActions};
use self::shelf::{Filter, Shelf};
use crate::library::{FolderId, LibraryData, ROOT_FOLDER};
use crate::route::Route;
use crate::settings::{SettingsStore, ShelfLayout};
use crate::state::{LibraryState, LibraryView};

/// The folder Home is browsing, shared by every surface that opens one.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Browse {
    folder: State<FolderId>,
}

impl std::fmt::Debug for Browse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Browse").finish_non_exhaustive()
    }
}

impl Browse {
    /// The browse state installed by [`use_browse_root`].
    #[must_use]
    pub fn get() -> Self {
        consume_root_context::<Self>()
    }

    /// The folder on screen; subscribes the caller.
    #[must_use]
    pub fn folder(&self) -> FolderId {
        *self.folder.read()
    }

    /// Show `folder`.
    pub fn enter(mut self, folder: FolderId) {
        self.folder.set(folder);
    }
}

/// Install the browse state at the app root.
pub fn use_browse_root() {
    let folder = use_state(|| ROOT_FOLDER);
    use_hook(|| provide_context_for_scope_id(Browse { folder }, ScopeId::ROOT));
}

/// Go Home and show `folder`.
pub fn open_folder(folder: FolderId) {
    Browse::get().enter(folder);
    let _ = RouterContext::get().replace(Route::Home);
}

/// Horizontal padding of Home for a width class.
const fn gutter(class: WidthClass) -> f32 {
    match class {
        WidthClass::Compact => tokens::space::GUTTER_COMPACT,
        WidthClass::Medium | WidthClass::Expanded => tokens::space::GUTTER,
    }
}

/// The Home route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Home;

impl Component for Home {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let view = LibraryView::get();
        let insets = *crate::hooks::use_safe_area_insets().read();
        let body = match view.state() {
            LibraryState::Loading => Loading.into_element(),
            LibraryState::Failed(why) => Failed { why }.into_element(),
            LibraryState::Ready(data) => Loaded { data }.into_element(),
        };
        rect()
            .expanded()
            .background(p.bg)
            .padding((insets.top, insets.right, 0.0, insets.left))
            .child(body)
    }
}

/// The library could not be read: a plain explanation, a retry, and the
/// technical cause on the clipboard for a bug report.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Failed {
    pub(super) why: String,
}

impl Component for Failed {
    fn render(&self) -> impl IntoElement {
        let why = self.why.clone();
        let actions = rect()
            .horizontal()
            .spacing(8.0)
            .child(
                Button::new("Copiar detalles")
                    .ghost()
                    .icon(freya::icons::lucide::copy())
                    .on_press(move |_| {
                        if freya::clipboard::Clipboard::set(why.clone()).is_ok() {
                            notein_ui_kit::Toasts::get()
                                .show(notein_ui_kit::Toast::new("Detalles copiados"));
                        }
                    }),
            )
            .child(
                Button::new("Reintentar")
                    .primary()
                    .icon(freya::icons::lucide::rotate_ccw())
                    .on_press(|_| LibraryView::get().retry()),
            );
        rect()
            .expanded()
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .padding(gutter(width_class()))
            .child(
                rect()
                    .width(Size::fill())
                    .max_width(Size::px(480.0))
                    .cross_align(Alignment::Center)
                    .spacing(4.0)
                    .child(
                    EmptyState::new(
                        freya::icons::lucide::triangle_alert(),
                        "No se pudo abrir la biblioteca",
                    )
                    .body(
                        "Los datos guardados en este dispositivo no se pueden leer. Si los \
                         copiaste de otra versión de Notein, borrá la carpeta de datos y \
                         volvé a abrir la app.",
                    )
                    .action(actions),
                    )
                    .child(
                        label()
                            .width(Size::fill())
                            .text(format!(
                                "Carpeta de datos: {}",
                                crate::library::bodies::root_dir().display()
                            ))
                            .font_size(tokens::text::META)
                            .color(palette().t3)
                            .text_align(TextAlign::Center),
                    ),
            )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Loading;

impl Component for Loading {
    fn render(&self) -> impl IntoElement {
        let pad = gutter(width_class());
        rect()
            .expanded()
            .padding(pad)
            .spacing(16.0)
            .child(Skeleton::new(44.0))
            .child(Skeleton::new(180.0))
            .child(rect().horizontal().spacing(16.0).children((0..4).map(|i| {
                rect()
                    .key(i)
                    .child(Skeleton::new(220.0).width(160.0))
                    .into_element()
            })))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Loaded {
    data: Arc<LibraryData>,
}

/// One stretch of Home's scrolling body. Each has a known height, so
/// only the ones on screen are built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Block {
    Top,
    Mesas,
    Pinned,
    Bar,
    /// A row of cards.
    Row(usize),
    /// A row of the list layout.
    Item(usize),
    Empty,
    /// Room for the + button and the system bar.
    End,
}

/// Everything the body's blocks are built from.
#[derive(Clone, PartialEq)]
struct Body {
    data: Arc<LibraryData>,
    shelf: Arc<Shelf>,
    blocks: Vec<Block>,
    columns: Columns,
    selection: State<Selection>,
    filter: State<Filter>,
    folder: FolderId,
    filtered: bool,
    pad: f32,
    width: f32,
    compact: bool,
    pinned_card: f32,
    bottom: f32,
}

impl Body {
    /// Space between the sections.
    const GAP: f32 = 24.0;
    const EMPTY: f32 = 320.0;

    fn height(&self, block: Block) -> f32 {
        match block {
            Block::Top => 8.0,
            Block::Mesas => MESAS_BLOCK + Self::GAP,
            Block::Pinned => PinnedRow::height(self.pinned_card) + Self::GAP,
            Block::Bar => LibraryBar::height(self.compact) + 16.0,
            Block::Row(_) => GridRow::height(self.columns),
            Block::Item(_) => ShelfRow::HEIGHT,
            Block::Empty => Self::EMPTY,
            Block::End => 120.0 + self.bottom,
        }
    }

    fn build(&self, block: Block, height: f32) -> Element {
        let content = match block {
            Block::Top | Block::End => None,
            Block::Mesas => Some(
                MesaStrip {
                    data: Arc::clone(&self.data),
                    width: self.width,
                    compact: self.compact,
                }
                .into_element(),
            ),
            Block::Pinned => Some(
                PinnedRow {
                    data: Arc::clone(&self.data),
                    card: self.pinned_card,
                    selection: self.selection,
                }
                .into_element(),
            ),
            Block::Bar => Some(
                LibraryBar {
                    data: Arc::clone(&self.data),
                    filter: self.filter,
                    selection: self.selection,
                }
                .into_element(),
            ),
            Block::Row(row) => Some(
                GridRow {
                    data: Arc::clone(&self.data),
                    shelf: Arc::clone(&self.shelf),
                    columns: self.columns,
                    selection: self.selection,
                    row,
                }
                .into_element(),
            ),
            Block::Item(index) => Some(
                ShelfRow {
                    data: Arc::clone(&self.data),
                    shelf: Arc::clone(&self.shelf),
                    selection: self.selection,
                    index,
                }
                .into_element(),
            ),
            Block::Empty => Some(
                EmptyShelf {
                    folder: self.folder,
                    filtered: self.filtered,
                }
                .into_element(),
            ),
        };
        rect()
            .width(Size::fill())
            .height(Size::px(height))
            .padding((0.0, self.pad))
            .maybe_child(content)
            .into_element()
    }
}

/// What floats at the bottom: the selection's actions, or the + button.
fn bottom(selection: State<Selection>, folder: FolderId, inset: f32) -> Element {
    if !selection.read().active {
        return Fab {
            folder,
            bottom: inset,
        }
        .into_element();
    }
    rect()
        .position(Position::new_absolute().left(0.0).bottom(inset + 16.0))
        .width(Size::fill())
        .layer(Layer::Relative(10))
        .horizontal()
        .main_align(Alignment::Center)
        .child(SelectionActions { selection, folder })
        .into_element()
}

impl Loaded {
    /// The blocks of the body, top to bottom.
    fn blocks(shelf: &Shelf, columns: Columns, (at_root, pinned): (bool, bool)) -> Vec<Block> {
        let mut blocks = vec![Block::Top];
        if at_root {
            blocks.push(Block::Mesas);
        }
        if pinned {
            blocks.push(Block::Pinned);
        }
        blocks.push(Block::Bar);
        if shelf.is_empty() {
            blocks.push(Block::Empty);
        } else if SettingsStore::get().read().library.layout == ShelfLayout::List {
            blocks.extend((0..ShelfRow::count(shelf)).map(Block::Item));
        } else {
            blocks.extend((0..GridRow::count(shelf, columns)).map(Block::Row));
        }
        blocks.push(Block::End);
        blocks
    }
}

impl Component for Loaded {
    fn render(&self) -> impl IntoElement {
        let class = width_class();
        let pad = gutter(class);
        let insets = *crate::hooks::use_safe_area_insets().read();
        let root_width = Platform::get().root_size.read().width;
        let content_width = pad.mul_add(-2.0, root_width - insets.left - insets.right).max(200.0);
        let folder = Browse::get().folder();
        let sort = SettingsStore::get().read().library.sort;
        let filter = use_state(Filter::default);
        let mut selection = use_state(Selection::default);
        let data = &self.data;

        // A folder that went away (trashed, deleted elsewhere) sends Home
        // back to the root rather than showing nothing.
        if folder != ROOT_FOLDER && data.folder(folder).is_none() {
            Browse::get().enter(ROOT_FOLDER);
        }
        // Entering another folder ends the selection made in this one.
        use_side_effect(move || {
            let _entered = Browse::get().folder();
            selection.write().clear();
        });

        let current_filter = filter.read().clone();
        let listed = Arc::new(Shelf::build(data, folder, &current_filter, sort));
        let at_root = folder == ROOT_FOLDER;
        let columns = Columns::fit(content_width);
        let body = Body {
            blocks: Self::blocks(&listed, columns, (at_root, at_root && !data.pinned().is_empty())),
            data: Arc::clone(data),
            shelf: listed,
            columns,
            selection,
            filter,
            folder,
            filtered: current_filter != Filter::All,
            pad,
            width: content_width,
            compact: class.is_compact(),
            pinned_card: if class.is_compact() { 132.0 } else { 148.0 },
            bottom: insets.bottom,
        };
        let heights: Vec<f32> = body.blocks.iter().map(|b| body.height(*b)).collect();
        let length = heights.len();
        let trash = crate::screens::trash::groups(data).len();
        // Only the body scrolls: the bar with search, sync state and the
        // trash stays in reach.
        let scroll = VirtualScrollView::new_with_data(body, |item, body: &Body| {
            body.blocks.get(item.index).map_or_else(
                || rect().into_element(),
                |block| body.build(*block, item.size),
            )
        })
        .length(length)
        .item_size(move |index: usize| heights.get(index).copied().unwrap_or_default())
        .width(Size::fill())
        .height(Size::flex(1.0));
        rect()
            .expanded()
            .content(Content::Flex)
            .a11y_role(AccessibilityRole::Main)
            .on_global_key_down(move |e: Event<KeyboardEventData>| on_key(&e, selection, folder))
            .child(
                rect()
                    .width(Size::fill())
                    .padding((8.0, pad, 0.0, pad))
                    .child(AppBar { trash }),
            )
            .child(scroll)
            .child(bottom(selection, folder, insets.bottom))
    }
}

/// Esc leaves the selection, Alt+Backspace goes up a folder, and Ctrl+K
/// (search), Ctrl+N (create), Ctrl+Shift+N (new mesa) and Ctrl+,
/// (settings) work as in a mesa.
fn on_key(e: &Event<KeyboardEventData>, mut selection: State<Selection>, folder: FolderId) {
    if notein_ui_kit::ModalController::get().is_open() {
        return;
    }
    let command = e.modifiers.ctrl() || e.modifiers.meta();
    let handled = match &e.key {
        Key::Named(NamedKey::Escape) if selection.peek().active => {
            selection.write().clear();
            true
        }
        Key::Named(NamedKey::Backspace) if e.modifiers.alt() => {
            up_one(folder);
            true
        }
        Key::Character(c) if command => match (c.to_lowercase().as_str(), e.modifiers.shift()) {
            ("k", _) => {
                crate::screens::search::open(String::new());
                true
            }
            ("n", false) => {
                crate::flows::create::open_menu_from_keyboard(crate::flows::create::Context {
                    folder,
                    selection: selection.peek().items.clone(),
                    folders: selection.peek().folders.clone(),
                });
                true
            }
            ("n", true) => {
                crate::flows::create::new_mesa();
                true
            }
            (",", _) => {
                crate::screens::settings::open();
                true
            }
            _ => false,
        },
        _ => false,
    };
    if handled {
        e.stop_propagation();
        e.prevent_default();
    }
}

/// Go to the parent of `folder`.
fn up_one(folder: FolderId) {
    if folder == ROOT_FOLDER {
        return;
    }
    let parent = LibraryView::get()
        .data()
        .and_then(|d| d.folder(folder).map(|f| f.parent))
        .unwrap_or(ROOT_FOLDER);
    Browse::get().enter(parent);
}
