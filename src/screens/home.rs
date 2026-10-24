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
use self::library::{Columns, EmptyShelf, Grid, LibraryBar, PinnedRow, Rows};
use self::mesas::{ContinueHero, MesaList};
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

impl Loaded {
    /// At the root: the mesa to continue and the other mesas.
    fn mesas(&self, class: WidthClass) -> Element {
        let data = &self.data;
        let compact = class.is_compact();
        let Some(id) = mesas::continue_target(data) else {
            return MesaList {
                data: Arc::clone(data),
                except: None,
                compact,
            }
            .into_element();
        };
        let hero = ContinueHero {
            data: Arc::clone(data),
            workspace: id,
        };
        let others = MesaList {
            data: Arc::clone(data),
            except: Some(id),
            compact,
        };
        if compact {
            return rect()
                .width(Size::fill())
                .spacing(16.0)
                .child(hero)
                .child(others)
                .into_element();
        }
        rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .spacing(20.0)
            .child(
                rect()
                    .width(Size::flex(if class == WidthClass::Expanded { 1.4 } else { 1.0 }))
                    .child(hero),
            )
            .child(rect().width(Size::flex(1.0)).child(others))
            .into_element()
    }

    /// The folder's contents as cards or rows, or why there are none.
    fn listing(
        &self,
        listed: &Arc<Shelf>,
        (folder, filtered): (FolderId, bool),
        columns: Columns,
        selection: State<Selection>,
    ) -> Element {
        if listed.is_empty() {
            return EmptyShelf { folder, filtered }.into_element();
        }
        if SettingsStore::get().read().library.layout == ShelfLayout::List {
            return Rows {
                data: Arc::clone(&self.data),
                shelf: Arc::clone(listed),
                selection,
            }
            .into_element();
        }
        Grid {
            data: Arc::clone(&self.data),
            shelf: Arc::clone(listed),
            columns,
            selection,
        }
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
        let pinned = (at_root && !data.pinned().is_empty()).then(|| PinnedRow {
            data: Arc::clone(data),
            card: if class.is_compact() { 132.0 } else { 148.0 },
            selection,
        });
        let listing = self.listing(
            &listed,
            (folder, current_filter != Filter::All),
            Columns::fit(content_width),
            selection,
        );
        let scroll = ScrollView::new()
            .width(Size::fill())
            .height(Size::fill())
            .child(
                rect()
                    .width(Size::fill())
                    .padding((8.0, pad, 120.0 + insets.bottom, pad))
                    .spacing(24.0)
                    .child(AppBar)
                    .maybe_child(at_root.then(|| self.mesas(class)))
                    .maybe_child(pinned)
                    .child(LibraryBar {
                        data: Arc::clone(data),
                        filter,
                        selection,
                    })
                    .child(listing),
            );
        rect()
            .expanded()
            .a11y_role(AccessibilityRole::Main)
            .on_global_key_down(move |e: Event<KeyboardEventData>| on_key(&e, selection, folder))
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
