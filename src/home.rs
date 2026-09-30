//! Home / library grid rendered by [`crate::route::Route::Home`].
//!
//! Layout mirrors Noteshelf's landing page:
//!
//! * top action bar (menu / notifications / sync on the left, home crest
//!   in the middle, multiselect / search / settings on the right),
//! * breadcrumb + toolbar row (`Inicio > Folder > …` on the left, view
//!   / sort affordances on the right),
//! * a wrapping grid of tinted "notebook" cards — folders and items
//!   sit side by side and sort by most-recently-touched first,
//! * a floating FAB stack (pen = new blank canvas, `+` = new folder)
//!   anchored bottom-right.
//!
//! Navigation is single-view: a `nav` [`Vec<FolderId>`] tracks the
//! stack of entered folders. Tapping a folder pushes it; a breadcrumb
//! crumb truncates the stack to that depth. Tapping an item pushes
//! [`Route::CanvasView`] (both canvas and PDF-backed items go there
//! for now — per-item body loading is a follow-up).

use std::sync::Arc;
use std::time::Duration;

use async_lock::Mutex as AsyncMutex;
use freya::animation::*;
use freya::prelude::*;
use freya::router::*;
use istmo::plugins::EdgeInsets;

use crate::components::{
    CanvasCreateSheet, CreateCanvasRequest, Modal, ModalController, auto_color,
};
use crate::hooks::use_safe_area_insets;
use crate::library::{
    BackgroundStyle, Folder, FolderId, Item, ItemId, ItemKind, Library, LibraryIndex, ROOT_FOLDER,
    Rgba, TagId,
};
use crate::route::Route;

/// Identifier for a home card. Lets the shared rename / drag state key by
/// the concrete row without knowing whether it's a folder or an item.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
enum CardId {
    Folder(FolderId),
    Item(ItemId),
}

/// Payload attached to a `DragZone` while a card is being dragged.
#[derive(Clone, Copy, PartialEq, Debug)]
enum MoveTarget {
    Item(ItemId),
    Folder(FolderId),
}

/// Long-press dwell (both mouse hold and touch) that opens the context
/// menu on platforms without a right mouse button.
const LONG_PRESS: Duration = Duration::from_millis(550);

/// Shared library handle. Wrapped in an async-aware mutex so every
/// `spawn`ed handler can `.await` the lock without blocking the freya
/// executor.
pub(crate) type LibHandle = Arc<AsyncMutex<Library>>;

/// Bring-up state for the async library open. Split from the loaded
/// handle so `Loading` / `Error` branches don't need a dummy Arc.
#[derive(Clone)]
enum LibState {
    Loading,
    Ready(LibHandle),
    Error(String),
}

/// Landing page.
#[derive(Debug, PartialEq)]
pub struct Home;

impl Component for Home {
    fn render(&self) -> impl IntoElement {
        let lib_state = use_state(|| LibState::Loading);
        let snap = use_state(|| Option::<LibraryIndex>::None);
        let nav = use_state(Vec::<FolderId>::new);
        let renaming = use_state(|| Option::<CardId>::None);
        let rename_buffer = use_state(String::new);
        let pad = *use_safe_area_insets().read();

        use_hook({
            let mut lib_state = lib_state;
            let mut snap = snap;
            move || {
                spawn(async move {
                    match open_library().await {
                        Ok(handle) => {
                            let index = handle.lock().await.index().clone();
                            snap.set(Some(index));
                            lib_state.set(LibState::Ready(handle));
                        }
                        Err(msg) => {
                            log::error!("library open failed: {msg}");
                            lib_state.set(LibState::Error(msg));
                        }
                    }
                });
            }
        });

        let bg = Color::from_rgb(15, 15, 18);
        let state_val = lib_state.read().clone();
        let snapshot = snap.read().clone();
        let nav_stack = nav.read().clone();
        let current = nav_stack.last().copied().unwrap_or(ROOT_FOLDER);

        let mut column = rect()
            .vertical()
            .expanded()
            .padding((pad.top, pad.right, 0.0, pad.left))
            .child(top_bar());
        let mut fab_slot: Option<Rect> = None;

        match (state_val, snapshot) {
            (LibState::Loading, _) => {
                column = column.child(centered("Cargando biblioteca…"));
            }
            (LibState::Error(msg), _) => {
                column = column.child(centered(&format!("No pude abrir la biblioteca: {msg}")));
            }
            (LibState::Ready(handle), Some(index)) => {
                column = column
                    .child(breadcrumb_bar(&index, &nav_stack, nav, handle.clone(), snap))
                    .child(grid(
                        &index,
                        current,
                        snap,
                        nav,
                        handle.clone(),
                        renaming,
                        rename_buffer,
                    ));
                fab_slot = Some(fab_stack(handle, snap, current, pad));
            }
            (LibState::Ready(_), None) => {
                column = column.child(centered("Preparando…"));
            }
        }

        let mut root = rect()
            .expanded()
            .background(bg)
            .child(ContextMenuViewer::new())
            .child(column);
        if let Some(fab) = fab_slot {
            root = root.child(fab);
        }
        root
    }
}

// ---------------------------------------------------------------------------
// Top bar
// ---------------------------------------------------------------------------

fn top_bar() -> impl IntoElement {
    let bg = Color::from_rgb(15, 15, 18);
    rect()
        .horizontal()
        .width(Size::fill())
        .height(Size::px(56.0))
        .padding((10.0, 16.0))
        .background(bg)
        .content(Content::Flex)
        .cross_align(Alignment::Center)
        .child(icon_button("☰"))
        .child(spacer_px(6.0))
        .child(flexible_spacer())
        .child(icon_button("🔍"))
        .child(spacer_px(6.0))
        .child(icon_button("⚙"))
}

fn icon_button(glyph: &'static str) -> impl IntoElement {
    rect()
        .width(Size::px(32.0))
        .height(Size::px(32.0))
        .center()
        .child(
            label()
                .color(Color::from_rgb(210, 210, 220))
                .font_size(16.0)
                .text(glyph),
        )
}

fn flexible_spacer() -> impl IntoElement {
    rect().width(Size::flex(1.)).height(Size::px(1.0))
}

fn spacer_px(px: f32) -> impl IntoElement {
    rect().width(Size::px(px)).height(Size::px(1.0))
}

// ---------------------------------------------------------------------------
// Breadcrumb + view toolbar
// ---------------------------------------------------------------------------

fn breadcrumb_bar(
    index: &LibraryIndex,
    nav_stack: &[FolderId],
    nav: State<Vec<FolderId>>,
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
) -> impl IntoElement {
    let mut row = rect()
        .horizontal()
        .width(Size::fill())
        .height(Size::px(44.0))
        .padding((6.0, 20.0))
        .content(Content::Flex)
        .cross_align(Alignment::Center)
        .child(sort_pill())
        .child(spacer_px(15.));

    row = row.child(CrumbDrop {
        text: "Inicio".to_owned(),
        active: nav_stack.is_empty(),
        target: ROOT_FOLDER,
        handle: handle.clone(),
        snap,
        on_click: NoArgCallback::new(move || {
            let mut nav = nav;
            nav.set(Vec::new());
        }),
    });

    for (i, folder_id) in nav_stack.iter().enumerate() {
        let name = index
            .folder(*folder_id)
            .map(|f| f.name.clone())
            .unwrap_or_else(|| "?".to_owned());
        let is_last = i + 1 == nav_stack.len();
        let depth = i + 1;
        let target = *folder_id;
        row = row.child(crumb_separator()).child(CrumbDrop {
            text: name,
            active: is_last,
            target,
            handle: handle.clone(),
            snap,
            on_click: NoArgCallback::new(move || {
                let mut nav = nav;
                nav.write().truncate(depth);
            }),
        });
    }

    row
}

/// Breadcrumb pill that doubles as a drop target for
/// `MoveTarget::{Item,Folder}` — dragging a card onto a crumb moves it
/// into that folder. Hover highlight lives inside the component so
/// freya's hook scoping is per-instance (per-iteration `use_state` would
/// otherwise break the "no hooks in for-loops" rule).
#[derive(Clone)]
struct CrumbDrop {
    text: String,
    active: bool,
    target: FolderId,
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
    on_click: NoArgCallback<()>,
}

impl std::fmt::Debug for CrumbDrop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrumbDrop")
            .field("text", &self.text)
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

impl PartialEq for CrumbDrop {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl Component for CrumbDrop {
    fn render(&self) -> impl IntoElement {
        let hover = use_state(|| false);
        let color = if self.active {
            Color::from_rgb(240, 240, 245)
        } else {
            Color::from_rgb(150, 150, 165)
        };
        let mut inner = rect().padding((4.0, 6.0));
        if *hover.read() {
            inner = inner
                .background(Color::from_argb(60, 100, 140, 240))
                .with_corner_radius(8.0);
        }
        let text = self.text.clone();
        inner = inner.child(label().color(color).font_size(13.0).text(text));
        let on_click = self.on_click.clone();
        let inner = inner.on_press(move |_| on_click.call());

        let handle = self.handle.clone();
        let snap = self.snap;
        let target = self.target;
        DropZone::new(inner.into_element(), move |tgt: MoveTarget| {
            let handle = handle.clone();
            let mut snap = snap;
            spawn_forever(async move {
                move_to_folder(handle, tgt, target, &mut snap).await;
            });
        })
        .on_drag_over(move |over: bool| {
            let mut h = hover;
            h.set(over);
        })
    }
}

fn crumb_separator() -> impl IntoElement {
    rect().padding((4.0, 4.0)).child(
        label()
            .color(Color::from_rgb(90, 90, 100))
            .font_size(13.0)
            .text("›"),
    )
}

fn sort_pill() -> impl IntoElement {
    rect()
        .horizontal()
        .padding((6.0, 12.0))
        .background(Color::from_rgb(30, 30, 36))
        .with_corner_radius(14.0)
        .cross_align(Alignment::Center)
        .child(
            label()
                .color(Color::from_rgb(210, 210, 220))
                .font_size(12.0)
                .text("⇅  Clasificar"),
        )
}

// ---------------------------------------------------------------------------
// Grid
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn grid(
    index: &LibraryIndex,
    current: FolderId,
    snap: State<Option<LibraryIndex>>,
    nav: State<Vec<FolderId>>,
    handle: LibHandle,
    renaming: State<Option<CardId>>,
    rename_buffer: State<String>,
) -> impl IntoElement {
    let mut folders: Vec<&Folder> = index
        .folders
        .iter()
        .filter(|f| f.parent == current)
        .collect();
    let mut items: Vec<&Item> = index.items.iter().filter(|i| i.folder == current).collect();
    folders.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    if folders.is_empty() && items.is_empty() {
        return rect()
            .width(Size::fill())
            .height(Size::fill())
            .center()
            .child(
                label()
                    .color(Color::from_rgb(120, 120, 135))
                    .font_size(13.0)
                    .text("Esta carpeta está vacía. Usa el botón + para crear una nota."),
            );
    }

    let mut wrap = rect()
        .width(Size::fill())
        .height(Size::fill())
        .padding((12.0, 24.0))
        .spacing(20.0)
        .horizontal()
        .content(Content::wrap());

    for folder in folders {
        let folder_id = folder.id;
        let child_count = index.items.iter().filter(|i| i.folder == folder_id).count()
            + index
                .folders
                .iter()
                .filter(|f| f.parent == folder_id)
                .count();
        let subtitle = format!("{} notas · {}", child_count, format_date(folder.updated_at));
        let tint = folder
            .color
            .unwrap_or_else(|| color_to_rgba(auto_color(&folder.name)));
        wrap = wrap.child(FolderCard {
            id: folder.id,
            title: folder.name.clone(),
            subtitle,
            tint,
            handle: handle.clone(),
            snap,
            nav,
            renaming,
            rename_buffer,
        });
    }

    for item in items {
        let kind_label = match item.kind {
            ItemKind::Canvas => "Lienzo",
            ItemKind::PdfCanvas => "PDF",
        };
        let subtitle = format!("{} · {}", kind_label, format_date(item.updated_at));
        let tint = item
            .color
            .unwrap_or_else(|| color_to_rgba(auto_color(&item.name)));
        wrap = wrap.child(ItemCard {
            id: item.id,
            kind: item.kind,
            title: item.name.clone(),
            subtitle,
            tint,
            bg_style: item.background,
            handle: handle.clone(),
            snap,
            renaming,
            rename_buffer,
        });
    }

    wrap
}


// ---------------------------------------------------------------------------
// Card impls (folder / item share gesture wiring but differ in menu actions)
// ---------------------------------------------------------------------------
//
// Each card is a proper `Component` so freya can scope its per-instance
// hooks (`use_state`, `use_animation`) — plain helper fns called from
// `for` loops would trip freya's "no hooks in loops" runtime guard.

/// Card for an `Item` row on the home grid.
#[derive(Clone)]
struct ItemCard {
    id: ItemId,
    kind: ItemKind,
    title: String,
    subtitle: String,
    tint: Rgba,
    bg_style: BackgroundStyle,
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
    renaming: State<Option<CardId>>,
    rename_buffer: State<String>,
}

impl std::fmt::Debug for ItemCard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ItemCard")
            .field("id", &self.id)
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

impl PartialEq for ItemCard {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl Component for ItemCard {
    fn render(&self) -> impl IntoElement {
        let card_id = CardId::Item(self.id);
        let is_editing = *self.renaming.read() == Some(card_id);

        let press_token = use_state(|| 0u64);
        let long_fired = use_state(|| false);

        let id = self.id;
        let kind = self.kind;
        let bg_style = self.bg_style;
        let tint = self.tint;
        let title = self.title.clone();
        let subtitle = self.subtitle.clone();
        let handle = self.handle.clone();
        let snap = self.snap;
        let renaming = self.renaming;
        let rename_buffer = self.rename_buffer;

        let open = move || {
            crate::route::queue_canvas_background(bg_style, rgba_to_color(tint).into());
            crate::route::set_current_canvas_item(Some(id));
            let route = match kind {
                ItemKind::Canvas => Route::CanvasView,
                ItemKind::PdfCanvas => Route::CanvasPdfView,
            };
            let _ = RouterContext::get().push(route);
        };

        let menu_builder = {
            let handle = handle.clone();
            let title = title.clone();
            move || {
                let start_rename = {
                    let mut renaming = renaming;
                    let mut rename_buffer = rename_buffer;
                    let name = title.clone();
                    move || {
                        rename_buffer.set(name.clone());
                        renaming.set(Some(card_id));
                    }
                };
                let ask_delete = {
                    let handle = handle.clone();
                    let mut snap = snap;
                    let name = title.clone();
                    move || {
                        confirm_delete_modal(name.clone(), {
                            let handle = handle.clone();
                            move || {
                                let handle = handle.clone();
                                spawn_forever(async move {
                                    let mut lib = handle.lock().await;
                                    if let Err(err) = lib.delete_item(id).await {
                                        log::error!("delete_item: {err}");
                                    }
                                    snap.set(Some(lib.index().clone()));
                                });
                            }
                        });
                    }
                };
                build_card_menu(start_rename, || log::info!("edit item: TODO"), ask_delete)
            }
        };

        let on_commit_rename = {
            let handle = handle.clone();
            let mut snap = snap;
            move |new_name: String| {
                let handle = handle.clone();
                spawn_forever(async move {
                    let mut lib = handle.lock().await;
                    if let Err(err) = lib.rename_item(id, &new_name).await {
                        log::error!("rename_item: {err}");
                    }
                    snap.set(Some(lib.index().clone()));
                });
            }
        };

        let body = card_body_view(
            title.clone(),
            subtitle,
            tint,
            false,
            is_editing,
            rename_buffer,
            renaming,
            on_commit_rename,
            open,
            menu_builder,
            press_token,
            long_fired,
        );

        DragZone::new(MoveTarget::Item(id), body.into_element())
            .drag_element(
                DragGhost {
                    title,
                    tint,
                    is_folder: false,
                }
                .into_element(),
            )
            .drag_threshold(6.0)
    }
}

/// Card for a `Folder` row on the home grid. Also a drop target for
/// items/other folders being dragged.
#[derive(Clone)]
struct FolderCard {
    id: FolderId,
    title: String,
    subtitle: String,
    tint: Rgba,
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
    nav: State<Vec<FolderId>>,
    renaming: State<Option<CardId>>,
    rename_buffer: State<String>,
}

impl std::fmt::Debug for FolderCard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FolderCard")
            .field("id", &self.id)
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

impl PartialEq for FolderCard {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl Component for FolderCard {
    fn render(&self) -> impl IntoElement {
        let card_id = CardId::Folder(self.id);
        let is_editing = *self.renaming.read() == Some(card_id);

        let press_token = use_state(|| 0u64);
        let long_fired = use_state(|| false);

        let id = self.id;
        let tint = self.tint;
        let title = self.title.clone();
        let subtitle = self.subtitle.clone();
        let handle = self.handle.clone();
        let snap = self.snap;
        let nav = self.nav;
        let renaming = self.renaming;
        let rename_buffer = self.rename_buffer;

        let open = move || {
            let mut nav = nav;
            nav.write().push(id);
        };

        let menu_builder = {
            let handle = handle.clone();
            let title = title.clone();
            move || {
                let start_rename = {
                    let mut renaming = renaming;
                    let mut rename_buffer = rename_buffer;
                    let name = title.clone();
                    move || {
                        rename_buffer.set(name.clone());
                        renaming.set(Some(card_id));
                    }
                };
                let ask_delete = {
                    let handle = handle.clone();
                    let mut snap = snap;
                    let name = title.clone();
                    move || {
                        confirm_delete_modal(name.clone(), {
                            let handle = handle.clone();
                            move || {
                                let handle = handle.clone();
                                spawn_forever(async move {
                                    let mut lib = handle.lock().await;
                                    if let Err(err) = lib.delete_folder(id).await {
                                        log::error!("delete_folder: {err}");
                                    }
                                    snap.set(Some(lib.index().clone()));
                                });
                            }
                        });
                    }
                };
                build_card_menu(
                    start_rename,
                    || log::info!("edit folder: TODO"),
                    ask_delete,
                )
            }
        };

        let on_commit_rename = {
            let handle = handle.clone();
            let mut snap = snap;
            move |new_name: String| {
                let handle = handle.clone();
                spawn_forever(async move {
                    let mut lib = handle.lock().await;
                    if let Err(err) = lib.rename_folder(id, &new_name).await {
                        log::error!("rename_folder: {err}");
                    }
                    snap.set(Some(lib.index().clone()));
                });
            }
        };

        let body = card_body_view(
            title.clone(),
            subtitle,
            tint,
            true,
            is_editing,
            rename_buffer,
            renaming,
            on_commit_rename,
            open,
            menu_builder,
            press_token,
            long_fired,
        );

        let drop = DropZone::new(body.into_element(), {
            let handle = handle.clone();
            let mut snap = snap;
            move |tgt: MoveTarget| {
                let handle = handle.clone();
                spawn_forever(async move {
                    move_to_folder(handle, tgt, id, &mut snap).await;
                });
            }
        });

        DragZone::new(MoveTarget::Folder(id), drop.into_element())
            .drag_element(
                DragGhost {
                    title,
                    tint,
                    is_folder: true,
                }
                .into_element(),
            )
            .drag_threshold(6.0)
    }
}

/// Shared visual + gesture wiring for both card types. Not a component —
/// takes the per-card hook slots (`press_token`, `long_fired`) by
/// argument so the calling `Component::render` owns their scoping.
#[allow(clippy::too_many_arguments)]
fn card_body_view<Open, Commit, MenuFn>(
    title: String,
    subtitle: String,
    tint: Rgba,
    is_folder: bool,
    is_editing: bool,
    rename_buffer: State<String>,
    renaming: State<Option<CardId>>,
    on_commit_rename: Commit,
    open: Open,
    menu_builder: MenuFn,
    press_token: State<u64>,
    long_fired: State<bool>,
) -> impl IntoElement
where
    Open: Fn() + Clone + 'static,
    Commit: Fn(String) + Clone + 'static,
    MenuFn: Fn() -> Menu + Clone + 'static,
{
    let cover = rect()
        .width(Size::px(130.0))
        .height(Size::px(160.0))
        .background(rgba_to_color(tint))
        .with_corner_radius(8.0)
        .center()
        .child(
            label()
                .color(Color::from_argb(180, 20, 20, 20))
                .font_size(32.0)
                .text(if is_folder { "📁" } else { "📓" }),
        );

    let mut column = rect()
        .vertical()
        .width(Size::px(140.0))
        .padding((4.0, 4.0))
        .spacing(6.0)
        .child(cover);

    if is_editing {
        column = column.child(
            Input::new(rename_buffer)
                .auto_focus(true)
                .width(Size::px(132.0))
                .on_submit({
                    let mut renaming = renaming;
                    let commit = on_commit_rename.clone();
                    move |value: String| {
                        let trimmed = value.trim().to_owned();
                        if !trimmed.is_empty() {
                            commit(trimmed);
                        }
                        renaming.set(None);
                    }
                }),
        );
    } else {
        column = column.child(
            label()
                .color(Color::from_rgb(230, 230, 235))
                .font_size(12.0)
                .text(title),
        );
    }

    column = column.child(
        label()
            .color(Color::from_rgb(130, 130, 145))
            .font_size(10.0)
            .text(subtitle),
    );

    let menu_for_secondary = menu_builder.clone();
    let column = column.on_secondary_down(move |e: Event<PressEventData>| {
        ContextMenu::open_from_event(&e, menu_for_secondary());
    });

    let menu_for_long = menu_builder.clone();
    let column = column.on_pointer_down({
        let mut long_fired = long_fired;
        let mut press_token = press_token;
        move |_: Event<PointerEventData>| {
            long_fired.set(false);
            let token = press_token.peek().wrapping_add(1);
            press_token.set(token);
            let menu = menu_for_long.clone();
            spawn(async move {
                async_io::Timer::after(LONG_PRESS).await;
                if *press_token.peek() == token {
                    long_fired.set(true);
                    ContextMenu::open(menu());
                }
            });
        }
    });

    let column = column.on_press({
        let open = open.clone();
        let mut press_token = press_token;
        let long_fired = long_fired;
        let editing_now = is_editing;
        move |_e: Event<PressEventData>| {
            press_token.set(0);
            if editing_now || *long_fired.peek() {
                return;
            }
            open();
        }
    });

    let mut press_token_move = press_token;
    let mut press_token_leave = press_token;
    column
        .on_pointer_move(move |_| {
            press_token_move.set(0);
        })
        .on_pointer_leave(move |_| {
            press_token_leave.set(0);
        })
}

fn build_card_menu(
    mut on_rename: impl FnMut() + Clone + 'static,
    mut on_edit: impl FnMut() + Clone + 'static,
    mut on_delete: impl FnMut() + Clone + 'static,
) -> Menu {
    Menu::new()
        .child(
            MenuButton::new()
                .on_press(move |_: Event<PressEventData>| on_rename())
                .child("Renombrar"),
        )
        .child(
            MenuButton::new()
                .on_press(move |_: Event<PressEventData>| on_edit())
                .child("Editar"),
        )
        .child(
            MenuButton::new()
                .on_press(move |_: Event<PressEventData>| on_delete())
                .child("Borrar"),
        )
}

fn confirm_delete_modal(name: String, mut on_confirm: impl FnMut() + Clone + 'static) {
    let body = rect()
        .vertical()
        .width(Size::px(280.0))
        .padding((20.0, 20.0))
        .background(Color::from_rgb(30, 30, 34))
        .with_corner_radius(14.0)
        .spacing(16.0)
        .child(
            label()
                .color(Color::from_rgb(240, 240, 245))
                .font_size(14.0)
                .text(format!("¿Borrar “{name}”?")),
        )
        .child(
            label()
                .color(Color::from_rgb(170, 170, 180))
                .font_size(12.0)
                .text("Esta acción no se puede deshacer."),
        )
        .child(
            rect()
                .horizontal()
                .spacing(10.0)
                .content(Content::Flex)
                .cross_align(Alignment::Center)
                .child(flexible_spacer())
                .child(
                    rect()
                        .padding((8.0, 14.0))
                        .with_corner_radius(8.0)
                        .background(Color::from_rgb(60, 60, 68))
                        .child(
                            label()
                                .color(Color::from_rgb(230, 230, 235))
                                .font_size(12.0)
                                .text("Cancelar"),
                        )
                        .on_press(|_| ModalController::get().close()),
                )
                .child(
                    rect()
                        .padding((8.0, 14.0))
                        .with_corner_radius(8.0)
                        .background(Color::from_rgb(200, 70, 70))
                        .child(
                            label()
                                .color(Color::from_rgb(255, 255, 255))
                                .font_size(12.0)
                                .text("Borrar"),
                        )
                        .on_press(move |_| {
                            on_confirm();
                            ModalController::get().close();
                        }),
                ),
        );
    ModalController::get().open(Modal::new(body).center().width(280.0));
}

/// Small floating clone rendered while a card is being dragged. Wiggles
/// via a looping rotation animation; own component so `use_animation`
/// stays out of the parent's for-loop.
#[derive(Clone)]
struct DragGhost {
    title: String,
    tint: Rgba,
    is_folder: bool,
}

impl std::fmt::Debug for DragGhost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DragGhost")
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

impl PartialEq for DragGhost {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl Component for DragGhost {
    fn render(&self) -> impl IntoElement {
        let anim = use_animation(|conf| {
            conf.on_creation(OnCreation::Run);
            conf.on_finish(OnFinish::reverse());
            AnimNum::new(-3.5, 3.5)
                .time(120)
                .ease(Ease::InOut)
                .function(Function::Sine)
        });
        let rotation = anim.get().value();
        rect()
            .width(Size::px(120.0))
            .height(Size::px(150.0))
            .background(rgba_to_color(self.tint))
            .with_corner_radius(8.0)
            .rotate(rotation)
            .center()
            .child(
                label()
                    .color(Color::from_argb(180, 20, 20, 20))
                    .font_size(28.0)
                    .text(if self.is_folder { "📁" } else { "📓" }),
            )
            .child(
                label()
                    .color(Color::from_argb(220, 20, 20, 20))
                    .font_size(11.0)
                    .text(self.title.clone()),
            )
    }
}

async fn move_to_folder(
    handle: LibHandle,
    what: MoveTarget,
    into: FolderId,
    snap: &mut State<Option<LibraryIndex>>,
) {
    let mut lib = handle.lock().await;
    let res = match what {
        MoveTarget::Item(id) => lib.move_item(id, into).await,
        MoveTarget::Folder(id) => lib.move_folder(id, into).await,
    };
    if let Err(err) = res {
        log::error!("move to folder: {err}");
    }
    snap.set(Some(lib.index().clone()));
}

fn centered(text: &str) -> impl IntoElement {
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .center()
        .child(
            label()
                .color(Color::from_rgb(180, 180, 195))
                .font_size(13.0)
                .text(text.to_owned()),
        )
}

// ---------------------------------------------------------------------------
// FAB overlay
// ---------------------------------------------------------------------------

fn fab_stack(
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
    current: FolderId,
    pad: EdgeInsets,
) -> Rect {
    use crate::components::{
        CanvasCreateSheet, CreateCanvasRequest, CreateFolderRequest, FabMenu, FabMenuEntry,
        FolderCreateSheet,
    };

    let pen_bg = Color::from_rgb(80, 130, 175);
    let plus_bg = Color::from_rgb(65, 105, 220);

    let pen = fab_button("✎", pen_bg, {
        let handle = handle.clone();
        move |_| {
            let handle = handle.clone();
            let mut snap = snap;
            spawn_forever(async move {
                let mut lib = handle.lock().await;
                match lib
                    .create_item(
                        current,
                        ItemKind::Canvas,
                        "Borrador",
                        None,
                        Vec::new(),
                        BackgroundStyle::default(),
                    )
                    .await
                {
                    Ok(id) => {
                        snap.set(Some(lib.index().clone()));
                        crate::route::queue_canvas_background(
                            BackgroundStyle::default(),
                            crate::route::DEFAULT_PAPER,
                        );
                        crate::route::set_current_canvas_item(Some(id));
                        let _ = RouterContext::get().push(Route::CanvasView);
                    }
                    Err(err) => log::error!("create_item: {err}"),
                }
            });
        }
    });

    // Anchor of the FAB stack, reused when placing the popup above it.
    let anchor_right = 24.0 + pad.right;
    let anchor_bottom = 24.0 + pad.bottom;
    // Pen + plus each 52px tall, 12px stack spacing, 12px extra breathing
    // room between the stack and the menu card.
    let menu_bottom = anchor_bottom + 52.0 + 12.0 + 52.0 + 12.0;

    let plus = fab_button("+", plus_bg, move |_| {
        let handle = handle.clone();
        FabMenu::new()
            .anchor(anchor_right, menu_bottom)
            .entry(FabMenuEntry::new("▢", "Lienzo infinito", {
                let handle = handle.clone();
                move || {
                    let handle = handle.clone();
                    let available = current_tag_names(snap);
                    CanvasCreateSheet::new(move |req: CreateCanvasRequest| {
                        log::info!(
                            "canvas confirm: name={:?} kind={:?} tags={:?}",
                            req.name,
                            req.kind,
                            req.tag_names
                        );
                        let handle = handle.clone();
                        let mut snap = snap;
                        let bg_style = req.background;
                        let surface = req.color;
                        spawn_forever(async move {
                            let mut lib = handle.lock().await;
                            let tag_ids = resolve_tag_names(&mut lib, &req.tag_names).await;
                            log::info!("canvas create → parent={current:?} tag_ids={tag_ids:?}");
                            match lib
                                .create_item(
                                    current,
                                    req.kind,
                                    &req.name,
                                    Some(color_to_rgba(req.color)),
                                    tag_ids,
                                    req.background,
                                )
                                .await
                            {
                                Ok(id) => {
                                    log::info!("canvas created id={id:?}");
                                    snap.set(Some(lib.index().clone()));
                                    crate::route::queue_canvas_background(bg_style, surface.into());
                                    crate::route::set_current_canvas_item(Some(id));
                                    let _ = RouterContext::get().push(Route::CanvasView);
                                }
                                Err(err) => log::error!("create_item: {err}"),
                            }
                        });
                    })
                    .kind(ItemKind::Canvas)
                    .available_tags(available)
                    .open();
                }
            }))
            .entry(FabMenuEntry::new("🗂", "Crear nueva carpeta", {
                let handle = handle.clone();
                move || {
                    let handle = handle.clone();
                    let available = current_tag_names(snap);
                    FolderCreateSheet::new(move |req: CreateFolderRequest| {
                        log::info!(
                            "folder confirm: name={:?} tags={:?}",
                            req.name,
                            req.tag_names
                        );
                        let handle = handle.clone();
                        let mut snap = snap;
                        spawn_forever(async move {
                            let mut lib = handle.lock().await;
                            let tag_ids = resolve_tag_names(&mut lib, &req.tag_names).await;
                            log::info!("folder create → parent={current:?} tag_ids={tag_ids:?}");
                            match lib
                                .create_folder(
                                    current,
                                    &req.name,
                                    Some(color_to_rgba(req.color)),
                                    tag_ids,
                                )
                                .await
                            {
                                Ok(id) => {
                                    log::info!("folder created id={id:?}");
                                    snap.set(Some(lib.index().clone()));
                                }
                                Err(err) => log::error!("create_folder: {err}"),
                            }
                        });
                    })
                    .available_tags(available)
                    .open();
                }
            }))
            .entry(
                FabMenuEntry::new("📄", "Importar PDF", {
                    let handle = handle.clone();
                    move || {
                        let handle = handle.clone();
                        spawn_forever(pick_and_open_pdf(handle, snap, current));
                    }
                })
                .with_divider_above(),
            )
            .open();
    });

    rect()
        .vertical()
        .spacing(12.0)
        .position(
            Position::new_absolute()
                .right(anchor_right)
                .bottom(anchor_bottom),
        )
        .child(pen)
        .child(plus)
}

fn fab_button<F: Fn(()) + 'static>(glyph: &'static str, bg: Color, handler: F) -> impl IntoElement {
    rect()
        .width(Size::px(52.0))
        .height(Size::px(52.0))
        .background(bg)
        .with_corner_radius(26.0)
        .center()
        .child(label().color(Color::WHITE).font_size(22.0).text(glyph))
        .on_press(move |_| handler(()))
}

// ---------------------------------------------------------------------------
// Library plumbing
// ---------------------------------------------------------------------------

pub(crate) async fn open_library() -> Result<LibHandle, String> {
    let runtime = istmo::Runtime::global().map_err(|e| format!("runtime not started: {e}"))?;
    let lib = Library::open(&runtime)
        .await
        .map_err(|e| format!("open library: {e}"))?;
    Ok(Arc::new(AsyncMutex::new(lib)))
}

/// Launch the platform file-picker, read the picked PDF into memory,
/// and open the canvas-create sheet pre-filled with its name.
async fn pick_and_open_pdf(handle: LibHandle, snap: State<Option<LibraryIndex>>, parent: FolderId) {
    use std::io::Read;
    use std::sync::Arc as StdArc;

    use istmo_file_picker::{FileFilter, FilePickerClient, PickConfig};

    let picker = match FilePickerClient::acquire() {
        Ok(p) => p,
        Err(err) => {
            log::error!("file picker unavailable: {err:?}");
            return;
        }
    };
    let config = PickConfig {
        dialog_title: Some("Elegí un PDF".into()),
        filter: FileFilter {
            mime_types: vec!["application/pdf".into()],
            extensions: vec!["pdf".into()],
            uti_types: vec!["com.adobe.pdf".into()],
        },
        start_directory_hint: None,
        allow_multiple: false,
    };
    let picked = match picker.pick_file_owned(config).await {
        Ok(Some(f)) => f,
        Ok(None) => return,
        Err(err) => {
            log::error!("picker call error: {err:?}");
            return;
        }
    };
    let display = picked.display_name.clone();
    let owned = StdArc::new(picked);
    let mut reader = match owned.open_reader(&picker).await {
        Ok(r) => r,
        Err(err) => {
            log::error!("open reader: {err:?}");
            return;
        }
    };
    let mut bytes = Vec::new();
    if let Err(err) = reader.read_to_end(&mut bytes) {
        log::error!("read pdf: {err}");
        return;
    }
    drop(reader);
    drop(owned);

    let default_name = strip_pdf_ext(&display);
    let bytes = StdArc::new(bytes);
    let available = current_tag_names(snap);
    CanvasCreateSheet::new(move |req: CreateCanvasRequest| {
        log::info!(
            "pdf confirm: name={:?} kind={:?} tags={:?}",
            req.name,
            req.kind,
            req.tag_names
        );
        let bytes = StdArc::clone(&bytes);
        let handle = handle.clone();
        let mut snap = snap;
        spawn_forever(async move {
            let mut lib = handle.lock().await;
            let tag_ids = resolve_tag_names(&mut lib, &req.tag_names).await;
            log::info!("pdf create → parent={parent:?} tag_ids={tag_ids:?}");
            match lib
                .create_item(
                    parent,
                    ItemKind::PdfCanvas,
                    &req.name,
                    Some(color_to_rgba(req.color)),
                    tag_ids,
                    req.background,
                )
                .await
            {
                Ok(id) => {
                    log::info!("pdf item created id={id:?}");
                    if let Err(err) = lib.attach_pdf(id, &bytes).await {
                        log::error!("attach_pdf: {err}");
                    }
                    snap.set(Some(lib.index().clone()));
                    crate::route::set_current_canvas_item(Some(id));
                    let _ = RouterContext::get().push(Route::CanvasPdfView);
                }
                Err(err) => log::error!("create_item: {err}"),
            }
        });
    })
    .kind(ItemKind::PdfCanvas)
    .default_name(default_name)
    .available_tags(available)
    .open();
}

fn strip_pdf_ext(name: &str) -> String {
    name.rsplit_once('.')
        .filter(|(_, ext)| ext.eq_ignore_ascii_case("pdf"))
        .map_or_else(|| name.to_owned(), |(stem, _)| stem.to_owned())
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

fn rgba_to_color(rgba: Rgba) -> Color {
    Color::from_argb(rgba[3], rgba[0], rgba[1], rgba[2])
}

fn color_to_rgba(color: Color) -> Rgba {
    [color.r(), color.g(), color.b(), color.a()]
}

/// Snapshot the current library tag names for the picker's suggestion
/// list. Empty when the library hasn't loaded yet.
fn current_tag_names(snap: State<Option<LibraryIndex>>) -> Vec<String> {
    snap.read()
        .as_ref()
        .map(|idx| idx.tags.iter().map(|t| t.name.clone()).collect())
        .unwrap_or_default()
}

/// Resolve raw tag names emitted by the picker into `TagId`s, creating
/// any tag whose name isn't present yet. Names are compared
/// case-insensitively; empty entries are dropped silently. New tags
/// receive a color auto-derived from their name.
async fn resolve_tag_names(lib: &mut Library, names: &[String]) -> Vec<TagId> {
    let mut ids = Vec::with_capacity(names.len());
    for name in names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(existing) = lib
            .tags()
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(trimmed))
        {
            ids.push(existing.id);
            continue;
        }
        let color = color_to_rgba(auto_color(trimmed));
        match lib.create_tag(trimmed, color).await {
            Ok(id) => ids.push(id),
            Err(err) => log::error!("create_tag({trimmed:?}): {err}"),
        }
    }
    ids
}

/// Format a millisecond epoch as `dd/mm/yy` (Spanish convention) using
/// the civil-from-days algorithm — no chrono dep just for one label.
fn format_date(ms: u64) -> String {
    let days = (ms / 86_400_000) as i64;
    let (y, m, d) = civil_from_days(days);
    format!("{:02}/{:02}/{:02}", d, m, (y.rem_euclid(100)) as u32)
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    // Howard Hinnant, "chrono-Compatible Low-Level Date Algorithms".
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}
