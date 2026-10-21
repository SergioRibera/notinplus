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
//! the mesa (every open document lands as a tab there).

use std::sync::Arc;

use async_lock::Mutex as AsyncMutex;
use freya::animation::*;
use freya::icons::lucide as icons;

use freya::prelude::*;
use istmo::plugins::EdgeInsets;

use crate::components::FormInput;
use crate::components::drag_drop::{DragZone, DropZone};
use crate::components::{
    CanvasCreateSheet, CreateCanvasRequest, GraphView, MenuGesture, Modal, ModalController,
    SettingsModal, SpaceCardData, SpaceEditRequest, SpaceEditSheet, SpaceStrip, auto_color,
    use_menu_gesture,
};
use crate::hooks::use_safe_area_insets;
use crate::library::{
    BackgroundStyle, DocKind, Folder, FolderId, Item, ItemId, Library, LibraryIndex, LinkIndex,
    ROOT_FOLDER, Rgba, TabTarget, TagId, Workspace, WorkspaceId,
};
use crate::space::OpenHint;

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

/// Snapshot of the persisted workspace list consumed by the switcher
/// pill. Cloned onto the UI side so dropdown renders don't need to
/// re-lock the library.
#[derive(Clone, PartialEq, Debug)]
struct WorkspaceSnapshot {
    active: Option<WorkspaceId>,
    workspaces: Vec<Workspace>,
}

/// Landing page.
#[derive(Debug, PartialEq)]
pub struct Home;

impl Component for Home {
    fn render(&self) -> impl IntoElement {
        let lib_state = use_state(|| LibState::Loading);
        let snap = use_state(|| Option::<LibraryIndex>::None);
        let links = use_state(LinkIndex::default);
        let ws_snap = use_state(|| Option::<WorkspaceSnapshot>::None);
        let nav = use_state(Vec::<FolderId>::new);
        let renaming = use_state(|| Option::<CardId>::None);
        let rename_buffer = use_state(String::new);
        let pad = *use_safe_area_insets().read();
        use_hook({
            let mut lib_state = lib_state;
            let mut snap = snap;
            let mut links = links;
            let mut ws_snap = ws_snap;
            move || {
                spawn(async move {
                    match open_library().await {
                        Ok(handle) => {
                            let lib = handle.lock().await;
                            snap.set(Some(lib.index().clone()));
                            links.set(lib.links().clone());
                            ws_snap.set(Some(snapshot_workspaces(&lib)));
                            drop(lib);
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

        // Sync state moves on its own thread; re-render when it does so
        // the pending marks on cards stay honest.
        let sync_tick = use_state(|| 0u64);
        let _ = *sync_tick.read();
        use_hook({
            let mut sync_tick = sync_tick;
            move || {
                let changes = crate::sync::changes();
                spawn(async move {
                    while changes.recv_async().await.is_ok() {
                        let next = sync_tick.peek().wrapping_add(1);
                        sync_tick.set(next);
                    }
                });
            }
        });

        // Links and tabs change from faces and chips while Home is
        // mounted; follow them so cards and the mesa strip stay true.
        use_hook({
            let mut snap = snap;
            let mut links = links;
            let mut ws_snap = ws_snap;
            move || {
                let changes = crate::space::changes();
                spawn(async move {
                    while changes.recv_async().await.is_ok() {
                        let Ok(handle) = open_library().await else {
                            continue;
                        };
                        let lib = handle.lock().await;
                        snap.set(Some(lib.index().clone()));
                        links.set(lib.links().clone());
                        ws_snap.set(Some(snapshot_workspaces(&lib)));
                    }
                });
            }
        });

        let bg = Color::from_rgb(15, 15, 18);
        let state_val = lib_state.read().clone();
        let snapshot = snap.read().clone();
        let links_now = links.read().clone();
        let ws_snapshot = ws_snap.read().clone();
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
                if let Some(ws) = ws_snapshot.as_ref() {
                    column = column.child(space_strip(handle.clone(), ws, ws_snap));
                }
                column = column
                    .child(breadcrumb_bar(
                        &index,
                        &nav_stack,
                        nav,
                        handle.clone(),
                        snap,
                    ))
                    .child(grid(
                        &index,
                        &links_now,
                        current,
                        snap,
                        links,
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
    let search_value = use_state(String::default);
    rect()
        .horizontal()
        .width(Size::fill())
        .height(Size::px(56.0))
        .padding((10.0, 16.0))
        .background(bg)
        .content(Content::Flex)
        .cross_align(Alignment::Center)
        .child(
            label()
                .color(Color::from_rgb(230, 230, 240))
                .font_size(15.0)
                .text("notinplus"),
        )
        .child(flexible_spacer())
        .child(
            FormInput::new(search_value)
                .flat()
                .background(Color::TRANSPARENT)
                .left_icon(icons::search())
                .collapsible()
                .corner_radius(99.)
                .width(Size::px(400.))
                .min_width(Size::px(40.))
                .placeholder("Search"),
        )
        .child(spacer_px(6.0))
        .child(icon_button(icons::waypoints()).on_press(|_| GraphView::open()))
        .child(spacer_px(6.0))
        .child(icon_button(icons::settings()).on_press(|_| SettingsModal::open()))
}

/// Row of mesas under the top bar. Tap opens one; long press edits,
/// duplicates, exports or deletes it.
fn space_strip(
    handle: LibHandle,
    ws: &WorkspaceSnapshot,
    ws_snap: State<Option<WorkspaceSnapshot>>,
) -> SpaceStrip {
    let spaces: Vec<SpaceCardData> = ws
        .workspaces
        .iter()
        .map(|w| SpaceCardData {
            id: w.id,
            name: w.name.clone(),
            color: w.color,
            layout: w.layout.clone(),
            active: ws.active == Some(w.id),
        })
        .collect();
    let on_open = Callback::new(|id: WorkspaceId| crate::space::open_space(id));
    let on_create = {
        let handle = handle.clone();
        NoArgCallback::new(move || {
            let handle = handle.clone();
            spawn_forever(async move {
                let created = {
                    let mut lib = handle.lock().await;
                    let name = format!("Mesa {}", lib.workspaces().len() + 1);
                    lib.create_workspace(&name, None).await
                };
                match created {
                    Ok(id) => crate::space::open_space(id),
                    Err(err) => log::error!("create workspace: {err}"),
                }
            });
        })
    };
    let menu_for = {
        let ws = ws.clone();
        Callback::new(move |id: WorkspaceId| space_menu(id, &ws, handle.clone(), ws_snap))
    };
    SpaceStrip::new(spaces, on_open, on_create, menu_for)
}

fn space_menu(
    id: WorkspaceId,
    ws: &WorkspaceSnapshot,
    handle: LibHandle,
    mut ws_snap: State<Option<WorkspaceSnapshot>>,
) -> Menu {
    let Some(space) = ws.workspaces.iter().find(|w| w.id == id).cloned() else {
        return Menu::new();
    };
    let edit = {
        let handle = handle.clone();
        let name = space.name.clone();
        let color = space.color;
        move |_: Event<PressEventData>| {
            let handle = handle.clone();
            SpaceEditSheet::new(name.clone(), color, move |req: SpaceEditRequest| {
                let handle = handle.clone();
                spawn_forever(async move {
                    let mut lib = handle.lock().await;
                    if let Err(err) = lib.rename_workspace(id, &req.name).await {
                        log::error!("rename_workspace: {err}");
                    }
                    if let Err(err) = lib.set_workspace_color(id, req.color).await {
                        log::error!("set_workspace_color: {err}");
                    }
                    ws_snap.set(Some(snapshot_workspaces(&lib)));
                });
            })
            .open();
        }
    };
    let duplicate = {
        let handle = handle.clone();
        move |_: Event<PressEventData>| {
            let handle = handle.clone();
            spawn_forever(async move {
                let mut lib = handle.lock().await;
                if let Err(err) = lib.duplicate_workspace(id).await {
                    log::error!("duplicate_workspace: {err}");
                }
                ws_snap.set(Some(snapshot_workspaces(&lib)));
            });
        }
    };
    let export = {
        let handle = handle.clone();
        let name = space.name.clone();
        move |_: Event<PressEventData>| {
            open_export_bundle_flow(
                handle.clone(),
                crate::bundle::ExportSelection {
                    workspaces: vec![id],
                    ..Default::default()
                },
                name.clone(),
            );
        }
    };
    let delete = {
        let name = space.name;
        move |_: Event<PressEventData>| {
            let handle = handle.clone();
            confirm_delete_modal(name.clone(), move || {
                let handle = handle.clone();
                spawn_forever(async move {
                    let mut lib = handle.lock().await;
                    if let Err(err) = lib.delete_workspace(id).await {
                        log::error!("delete_workspace: {err}");
                    }
                    ws_snap.set(Some(snapshot_workspaces(&lib)));
                });
            });
        }
    };
    Menu::new()
        .child(MenuButton::new().on_press(edit).child("Renombrar / color"))
        .child(MenuButton::new().on_press(duplicate).child("Duplicar"))
        .child(MenuButton::new().on_press(export).child("Exportar"))
        .child(MenuButton::new().on_press(delete).child("Eliminar"))
}

/// Snapshot the library's workspace index into a UI-side struct. Keeps
/// the switcher independent of the async-locked [`Library`].
fn snapshot_workspaces(lib: &Library) -> WorkspaceSnapshot {
    WorkspaceSnapshot {
        active: lib.active_workspace(),
        workspaces: lib.workspaces().to_vec(),
    }
}

fn icon_button(icon: Bytes) -> Rect {
    rect()
        .width(Size::px(32.0))
        .height(Size::px(32.0))
        .center()
        .child(
            SvgViewer::new(icon)
                .color(Color::from_rgb(210, 210, 220))
                .width(Size::px(18.0))
                .height(Size::px(18.0)),
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
    links_now: &LinkIndex,
    current: FolderId,
    snap: State<Option<LibraryIndex>>,
    links: State<LinkIndex>,
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
        let notebooks = links_now.companions_of(item.id).count();
        let parents: Vec<String> = links_now
            .companion_parents_of(item.id)
            .filter_map(|p| index.item(p).map(|i| i.name.clone()))
            .collect();
        let role = match parents.as_slice() {
            [] => crate::faces::kind_label(&item.kind).to_owned(),
            [only] => format!("Cuaderno de {only}"),
            [first, rest @ ..] => format!("Cuaderno de {first} +{}", rest.len()),
        };
        let subtitle = format!("{role} · {}", format_date(item.updated_at));
        let tint = item
            .color
            .unwrap_or_else(|| color_to_rgba(auto_color(&item.name)));
        wrap = wrap.child(ItemCard {
            id: item.id,
            title: item.name.clone(),
            subtitle,
            notebooks,
            pending: crate::sync::doc_status(item.id) == notein_sync::DocStatus::Pending,
            shared: crate::sync::share_of(item.id).is_some(),
            tint,
            bg_style: item.background,
            handle: handle.clone(),
            snap,
            links,
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
    title: String,
    subtitle: String,
    /// How many notebooks hang off this document (badge).
    notebooks: usize,
    /// Whether another device holds edits this one has not seen yet.
    pending: bool,
    /// Whether the document is shared with other people.
    shared: bool,
    tint: Rgba,
    bg_style: BackgroundStyle,
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
    links: State<LinkIndex>,
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

        let gesture = use_menu_gesture();

        let id = self.id;
        let bg_style = self.bg_style;
        let tint = self.tint;
        let title = self.title.clone();
        let subtitle = self.subtitle.clone();
        let handle = self.handle.clone();
        let snap = self.snap;
        let renaming = self.renaming;
        let rename_buffer = self.rename_buffer;

        let focus = use_a11y();
        let focus_status = use_focus(focus);

        use_side_effect({
            let mut editing = self.renaming.clone();
            move || {
                if !focus_status().is_focused() {
                    editing.set(None);
                    focus.request_unfocus();
                }
            }
        });

        let open = move || {
            crate::route::queue_canvas_background(
                TabTarget::Item(id),
                bg_style,
                rgba_to_color(tint).into(),
            );
            crate::space::open(TabTarget::Item(id), OpenHint::Focused);
        };

        let menu_builder = {
            let handle = handle.clone();
            let title = title.clone();
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
                let snap = snap;
                let links = self.links;
                let name = title.clone();
                move || ask_delete_item(id, name.clone(), handle.clone(), snap, links)
            };
            let on_export = {
                let handle = handle.clone();
                let name = title.clone();
                move || {
                    open_export_bundle_flow(
                        handle.clone(),
                        crate::bundle::ExportSelection {
                            items: vec![id],
                            ..Default::default()
                        },
                        name.clone(),
                    );
                }
            };
            let on_edit_item = {
                let snap = snap;
                move || open_canvas_edit_sheet(id, Some(snap))
            };
            build_item_menu(ItemMenuActions {
                open_beside: Box::new(move || {
                    crate::space::open(TabTarget::Item(id), OpenHint::Beside);
                }),
                open_in_new_space: Box::new(move || {
                    crate::space::open(TabTarget::Item(id), OpenHint::NewSpace);
                }),
                notebook: Box::new(move || crate::notebooks::open_notebook(id)),
                link_with: Box::new(move || crate::notebooks::link_related(id)),
                share: Box::new(move || crate::components::ShareSheet::open(id, None)),
                rename: Box::new(start_rename),
                edit: Box::new(on_edit_item),
                export: Box::new(on_export),
                delete: Box::new(ask_delete),
            })
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
            self.notebooks,
            self.pending,
            self.shared,
            tint,
            false,
            is_editing,
            focus,
            rename_buffer,
            renaming,
            on_commit_rename,
            open,
            menu_builder,
            gesture,
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

        let gesture = use_menu_gesture();

        let id = self.id;
        let tint = self.tint;
        let title = self.title.clone();
        let subtitle = self.subtitle.clone();
        let handle = self.handle.clone();
        let snap = self.snap;
        let nav = self.nav;
        let renaming = self.renaming;
        let rename_buffer = self.rename_buffer;

        let focus = use_a11y();
        let focus_status = use_focus(focus);

        use_side_effect({
            let mut editing = self.renaming.clone();
            move || {
                if !focus_status().is_focused() {
                    editing.set(None);
                    println!("Unfocus input");
                    focus.request_unfocus();
                }
            }
        });

        let open = move || {
            let mut nav = nav;
            nav.write().push(id);
        };

        let menu_builder = {
            let handle = handle.clone();
            let title = title.clone();
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
            let on_export = {
                let handle = handle.clone();
                let name = title.clone();
                move || {
                    open_export_bundle_flow(
                        handle.clone(),
                        crate::bundle::ExportSelection {
                            folders: vec![id],
                            ..Default::default()
                        },
                        name.clone(),
                    );
                }
            };
            let on_edit_folder = {
                let snap = snap;
                move || open_folder_edit_sheet(id, Some(snap))
            };
            build_card_menu(start_rename, on_edit_folder, on_export, ask_delete)
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
            0,
            false,
            false,
            tint,
            true,
            is_editing,
            focus,
            rename_buffer,
            renaming,
            on_commit_rename,
            open,
            menu_builder,
            gesture,
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
/// takes the per-card [`MenuGesture`] by argument so the calling
/// `Component::render` owns its scoping.
#[allow(clippy::too_many_arguments)]
fn card_body_view<Open, Commit>(
    title: String,
    subtitle: String,
    notebooks: usize,
    pending: bool,
    shared: bool,
    tint: Rgba,
    is_folder: bool,
    is_editing: bool,
    focus: AccessibilityId,
    rename_buffer: State<String>,
    renaming: State<Option<CardId>>,
    on_commit_rename: Commit,
    open: Open,
    menu_builder: Menu,
    gesture: MenuGesture,
) -> impl IntoElement
where
    Open: Fn() + Clone + 'static,
    Commit: Fn(String) + Clone + 'static,
{
    let cover = rect()
        .width(Size::px(130.0))
        .height(Size::px(130.0))
        .center()
        .child(
            SvgViewer::new(if is_folder {
                icons::folder()
            } else {
                icons::notebook()
            })
            .color(rgba_to_color(tint))
            .fill(rgba_to_color(tint))
            .width(Size::percent(80.0))
            .height(Size::percent(80.0)),
        )
        .maybe_child((notebooks > 0).then(|| notebook_badge(notebooks)))
        .maybe_child(shared.then(|| shared_badge(6.0)))
        .maybe_child(pending.then(|| sync_badge(if shared { 32.0 } else { 6.0 })));

    let mut column = rect()
        .vertical()
        .width(Size::px(140.0))
        .padding((4.0, 4.0))
        .spacing(6.0)
        .child(cover);

    if is_editing {
        column = column.child(
            crate::components::FormInput::new(rename_buffer)
                .flat()
                .a11y_id(focus)
                .auto_focus(true)
                .width(Size::px(132.0))
                .on_change({
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
        let menu_for_secondary = menu_builder.clone();
        column = column.child(
            rect()
                .spacing(5.)
                .horizontal()
                .content(Content::Flex)
                .child(
                    label()
                        .width(Size::flex(1.))
                        .text_overflow(TextOverflow::Ellipsis)
                        .color(Color::from_rgb(230, 230, 235))
                        .font_size(12.0)
                        .text(title),
                )
                .child(
                    rect()
                        .on_press(move |e: Event<PressEventData>| {
                            e.stop_propagation();
                            ContextMenu::open(menu_for_secondary.clone());
                        })
                        .child(
                            SvgViewer::new(freya::icons::lucide::ellipsis_vertical())
                                .color(Color::WHITE)
                                .width(Size::px(16.))
                                .height(Size::px(16.)),
                        ),
                ),
        );
    }

    column = column.child(
        label()
            .color(Color::from_rgb(130, 130, 145))
            .font_size(10.0)
            .text(subtitle),
    );

    gesture.apply(
        column,
        move || menu_builder.clone(),
        move || {
            if !is_editing {
                open();
            }
        },
    )
}

/// Resolve `selection` into an [`crate::bundle::ExportPreview`],
/// open the review sheet, then — on confirm — hand the trimmed
/// item list + options off to [`export_selection_via_picker`]. This
/// is the single entry point the item / folder / workspace export
/// surfaces call so the user always sees the preview + attachments
/// toggle before the save picker opens.
pub(crate) fn open_export_bundle_flow(
    handle: LibHandle,
    selection: crate::bundle::ExportSelection,
    suggested_stem: String,
) {
    use crate::components::{ExportBundleRequest, ExportBundleSheet};

    let preview_handle = handle.clone();
    spawn_forever(async move {
        let preview = {
            let lib = preview_handle.lock().await;
            match crate::bundle::preview_export(&lib, &selection) {
                Ok(p) => p,
                Err(err) => {
                    log::error!("preview export: {err}");
                    return;
                }
            }
        };

        let launch_handle = handle;
        let launch_stem = suggested_stem;
        ExportBundleSheet::new(preview, move |req: ExportBundleRequest| {
            if req.items.is_empty() {
                return;
            }
            let launch_handle = launch_handle.clone();
            let launch_stem = launch_stem.clone();
            spawn_forever(async move {
                export_selection_via_picker(
                    launch_handle,
                    crate::bundle::ExportSelection {
                        items: req.items,
                        ..Default::default()
                    },
                    req.options,
                    launch_stem,
                )
                .await;
            });
        })
        .open();
    });
}

/// Prompt the user for a destination `.notein` file via the file
/// picker, then stream a bundle of `selection` into it. Logs on
/// failure; no toast infra on home yet.
pub(crate) async fn export_selection_via_picker(
    handle: LibHandle,
    selection: crate::bundle::ExportSelection,
    options: crate::bundle::ExportOptions,
    suggested_stem: String,
) {
    use istmo_file_picker::{FileFilter, FilePickerClient, SaveConfig};

    let picker = match FilePickerClient::acquire() {
        Ok(p) => p,
        Err(err) => {
            log::error!("file picker unavailable: {err:?}");
            return;
        }
    };
    let suggested = format!("{}.notein", sanitize_filename(&suggested_stem));
    let config = SaveConfig {
        dialog_title: Some("Exportar bundle".into()),
        suggested_name: Some(suggested),
        mime_type: Some("application/octet-stream".into()),
        filter: FileFilter {
            mime_types: vec!["application/octet-stream".into()],
            extensions: vec!["notein".into()],
            uti_types: Vec::new(),
        },
        start_directory_hint: None,
        overwrite_prompt: true,
    };
    let picked = match picker.save_file_owned(config).await {
        Ok(Some(f)) => f,
        Ok(None) => return,
        Err(err) => {
            log::error!("save picker: {err:?}");
            return;
        }
    };
    let owned = std::sync::Arc::new(picked);
    let writer = match owned.open_writer(&picker).await {
        Ok(w) => w,
        Err(err) => {
            log::error!("open writer: {err:?}");
            return;
        }
    };
    let lib = handle.lock().await;
    if let Err(err) = crate::bundle::write_bundle(writer, &lib, &selection, options) {
        log::error!("export bundle: {err}");
    } else {
        log::info!("bundle exported to {}", owned.display_name);
    }
    drop(lib);
    drop(owned);
}

fn sanitize_filename(stem: &str) -> String {
    let trimmed = stem.trim();
    let cleaned: String = trimmed
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "bundle".into()
    } else {
        cleaned
    }
}

/// Pill over a card's cover showing how many notebooks it has.
fn notebook_badge(count: usize) -> Rect {
    rect()
        .position(Position::new_absolute().top(6.0).right(6.0))
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(4.0)
        .padding((3.0, 7.0))
        .with_corner_radius(10.0)
        .background(Color::from_rgb(40, 40, 48))
        .child(
            SvgViewer::new(icons::notebook_text())
                .color(Color::from_rgb(230, 230, 240))
                .width(Size::px(12.0))
                .height(Size::px(12.0)),
        )
        .child(
            label()
                .color(Color::from_rgb(230, 230, 240))
                .font_size(11.0)
                .text(count.to_string()),
        )
}

/// Small mark for a document whose latest edits live on another
/// device and have not arrived yet.
fn sync_badge(left: f32) -> Rect {
    rect()
        .position(Position::new_absolute().top(6.0).left(left))
        .padding(4.0)
        .with_corner_radius(10.0)
        .background(Color::from_rgb(40, 40, 48))
        .child(
            SvgViewer::new(icons::refresh_cw())
                .color(Color::from_rgb(230, 200, 120))
                .width(Size::px(12.0))
                .height(Size::px(12.0)),
        )
}

/// Mark for a document shared with other people.
fn shared_badge(left: f32) -> Rect {
    rect()
        .position(Position::new_absolute().top(6.0).left(left))
        .padding(4.0)
        .with_corner_radius(10.0)
        .background(Color::from_rgb(40, 40, 48))
        .child(
            SvgViewer::new(icons::users())
                .color(Color::from_rgb(140, 200, 255))
                .width(Size::px(12.0))
                .height(Size::px(12.0)),
        )
}

/// Every action the long-press menu of a document card offers.
struct ItemMenuActions {
    open_beside: Box<dyn FnMut()>,
    open_in_new_space: Box<dyn FnMut()>,
    notebook: Box<dyn FnMut()>,
    link_with: Box<dyn FnMut()>,
    share: Box<dyn FnMut()>,
    rename: Box<dyn FnMut()>,
    edit: Box<dyn FnMut()>,
    export: Box<dyn FnMut()>,
    delete: Box<dyn FnMut()>,
}

fn build_item_menu(actions: ItemMenuActions) -> Menu {
    let ItemMenuActions {
        mut open_beside,
        mut open_in_new_space,
        mut notebook,
        mut link_with,
        mut share,
        mut rename,
        mut edit,
        mut export,
        mut delete,
    } = actions;
    Menu::new()
        .child(
            MenuButton::new()
                .on_press(move |_: Event<PressEventData>| open_beside())
                .child("Abrir al lado"),
        )
        .child(
            MenuButton::new()
                .on_press(move |_: Event<PressEventData>| open_in_new_space())
                .child("Abrir en mesa nueva"),
        )
        .child(
            MenuButton::new()
                .on_press(move |_: Event<PressEventData>| notebook())
                .child("Cuaderno"),
        )
        .child(
            MenuButton::new()
                .on_press(move |_: Event<PressEventData>| link_with())
                .child("Vincular con…"),
        )
        .child(
            MenuButton::new()
                .on_press(move |_: Event<PressEventData>| share())
                .child("Compartir…"),
        )
        .child(
            MenuButton::new()
                .on_press(move |_: Event<PressEventData>| rename())
                .child("Renombrar"),
        )
        .child(
            MenuButton::new()
                .on_press(move |_: Event<PressEventData>| edit())
                .child("Editar"),
        )
        .child(
            MenuButton::new()
                .on_press(move |_: Event<PressEventData>| export())
                .child("Exportar"),
        )
        .child(
            MenuButton::new()
                .on_press(move |_: Event<PressEventData>| delete())
                .child("Borrar"),
        )
}

/// Confirm deleting a document; notebooks that would be left without
/// any document get offered for deletion alongside it.
fn ask_delete_item(
    id: ItemId,
    name: String,
    handle: LibHandle,
    mut snap: State<Option<LibraryIndex>>,
    mut links: State<LinkIndex>,
) {
    spawn_forever(async move {
        let orphans: Vec<(ItemId, String)> = {
            let lib = handle.lock().await;
            lib.companions_of(id)
                .filter(|c| lib.companion_parents_of(*c).all(|p| p == id))
                .filter_map(|c| lib.item(c).map(|it| (c, it.name.clone())))
                .collect()
        };
        let delete = {
            let handle = handle.clone();
            move |also: Vec<ItemId>| {
                let handle = handle.clone();
                spawn_forever(async move {
                    let mut lib = handle.lock().await;
                    for extra in also {
                        if let Err(err) = lib.delete_item(extra).await {
                            log::error!("delete_item (notebook): {err}");
                        }
                    }
                    if let Err(err) = lib.delete_item(id).await {
                        log::error!("delete_item: {err}");
                    }
                    snap.set(Some(lib.index().clone()));
                    links.set(lib.links().clone());
                });
            }
        };
        if orphans.is_empty() {
            let delete = delete;
            confirm_delete_modal(name, move || delete(Vec::new()));
        } else {
            confirm_delete_with_notebooks_modal(name, orphans, delete);
        }
    });
}

fn confirm_delete_with_notebooks_modal(
    name: String,
    notebooks: Vec<(ItemId, String)>,
    on_delete: impl FnMut(Vec<ItemId>) + Clone + 'static,
) {
    let names: Vec<&str> = notebooks.iter().map(|(_, n)| n.as_str()).collect();
    let ids: Vec<ItemId> = notebooks.iter().map(|(id, _)| *id).collect();
    let only_doc = {
        let mut on_delete = on_delete.clone();
        move |_: Event<PressEventData>| {
            on_delete(Vec::new());
            ModalController::get().close();
        }
    };
    let everything = {
        let mut on_delete = on_delete;
        move |_: Event<PressEventData>| {
            on_delete(ids.clone());
            ModalController::get().close();
        }
    };
    let body = rect()
        .vertical()
        .width(Size::px(320.0))
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
                .text(format!(
                    "Tiene {} cuaderno{} que quedarían sueltos: {}.",
                    names.len(),
                    if names.len() == 1 { "" } else { "s" },
                    names.join(", ")
                )),
        )
        .child(
            rect()
                .vertical()
                .spacing(8.0)
                .child(modal_button(
                    "Cancelar",
                    Color::from_rgb(60, 60, 68),
                    |_| {
                        ModalController::get().close();
                    },
                ))
                .child(modal_button(
                    "Borrar solo el documento",
                    Color::from_rgb(60, 60, 68),
                    only_doc,
                ))
                .child(modal_button(
                    "Borrar todo",
                    Color::from_rgb(200, 70, 70),
                    everything,
                )),
        );
    ModalController::get().open(Modal::new(body).center().width(320.0));
}

fn modal_button(
    text: &'static str,
    bg: Color,
    on_press: impl FnMut(Event<PressEventData>) + 'static,
) -> Rect {
    rect()
        .width(Size::fill())
        .padding((8.0, 14.0))
        .with_corner_radius(8.0)
        .background(bg)
        .center()
        .child(
            label()
                .color(Color::from_rgb(240, 240, 245))
                .font_size(12.0)
                .text(text),
        )
        .on_press(on_press)
}

fn build_card_menu(
    mut on_rename: impl FnMut() + Clone + 'static,
    mut on_edit: impl FnMut() + Clone + 'static,
    mut on_export: impl FnMut() + Clone + 'static,
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
                .on_press(move |_: Event<PressEventData>| on_export())
                .child("Exportar"),
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
                SvgViewer::new(if self.is_folder {
                    icons::folder()
                } else {
                    icons::notebook()
                })
                .color(Color::from_argb(180, 20, 20, 20))
                .width(Size::px(40.0))
                .height(Size::px(40.0)),
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

#[derive(Clone)]
struct FabCtx {
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
    current: FolderId,
}

fn fab_stack(
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
    current: FolderId,
    pad: EdgeInsets,
) -> Rect {
    let ctx = FabCtx {
        handle,
        snap,
        current,
    };

    let plus_bg = Color::from_rgb(65, 105, 220);
    let shortcut_slots = crate::plugins::fab_shortcut_slots();

    // Anchor of the FAB stack, reused when placing the popup above it.
    let anchor_right = 24.0 + pad.right;
    let anchor_bottom = 24.0 + pad.bottom;
    // Two shortcut bubbles + plus = 3 × 52px with 12px gaps, plus 12px
    // breathing room between the stack and the menu card.
    let menu_bottom = anchor_bottom + 52.0 + 12.0 + 52.0 + 12.0 + 52.0 + 12.0;

    let shortcut_top = shortcut_button(shortcut_slots[0].as_ref(), ctx.clone());
    let shortcut_bottom = shortcut_button(shortcut_slots[1].as_ref(), ctx.clone());

    let plus = {
        let ctx = ctx.clone();
        fab_button(icons::plus(), plus_bg, move |_| {
            open_create_menu(ctx.clone(), anchor_right, menu_bottom);
        })
    };

    rect()
        .vertical()
        .spacing(12.0)
        .position(
            Position::new_absolute()
                .right(anchor_right)
                .bottom(anchor_bottom),
        )
        .child(shortcut_top)
        .child(shortcut_bottom)
        .child(plus)
}

fn shortcut_button(slot: Option<&crate::plugin_settings::FabShortcut>, ctx: FabCtx) -> Rect {
    let resolved = slot.and_then(crate::plugins::resolve_shortcut);
    match resolved {
        Some(entry) => {
            let bg = shortcut_color(&entry);
            fab_button(entry.entry.icon.clone(), bg, move |_| {
                trigger_entry(&entry, &ctx);
            })
        }
        None => {
            // Empty slot: subdued bubble that opens settings so the
            // user can pick what sits here.
            let bg = Color::from_rgb(38, 38, 44);
            fab_button(icons::plus(), bg, |_| {
                crate::components::SettingsModal::open();
            })
        }
    }
}

fn shortcut_color(entry: &crate::plugins::AvailableEntry) -> Color {
    use notein_plugin::CreationCategory;
    match entry.entry.category {
        CreationCategory::Canvas => Color::from_rgb(80, 130, 175),
        CreationCategory::Source => Color::from_rgb(90, 110, 85),
        CreationCategory::Other => Color::from_rgb(45, 45, 52),
    }
}

fn open_create_menu(ctx: FabCtx, anchor_right: f32, menu_bottom: f32) {
    use crate::components::{FabMenu, FabMenuEntry};

    let grouped = crate::plugins::group_by_category(crate::plugins::available_entries());
    let mut menu = FabMenu::new().anchor(anchor_right, menu_bottom);

    for (gi, (_cat, group)) in grouped.iter().enumerate() {
        for (ei, entry) in group.iter().enumerate() {
            let entry = entry.clone();
            let ctx = ctx.clone();
            let label = menu_label(&entry);
            let mut row = FabMenuEntry::new(entry.entry.icon.clone(), label, move || {
                trigger_entry(&entry, &ctx);
            });
            // Divider between category groups (first entry of each
            // non-first group).
            if gi > 0 && ei == 0 {
                row = row.with_divider_above();
            }
            menu = menu.entry(row);
        }
    }
    menu.open();
}

fn menu_label(entry: &crate::plugins::AvailableEntry) -> String {
    use crate::plugins::EntrySource;
    match entry.source {
        EntrySource::ShellBuiltin => entry.entry.label.clone(),
        EntrySource::Plugin(_) => {
            // Suffix third-party entries with the owning plugin's label
            // so the user can tell duplicates apart.
            format!("{}  ·  {}", entry.entry.label, entry.plugin_label)
        }
    }
}

fn trigger_entry(entry: &crate::plugins::AvailableEntry, ctx: &FabCtx) {
    use crate::builtin::{pdf_source, stroke_canvas};
    use crate::plugins::EntrySource;
    use crate::shell_builtins::{BUILTIN_PLUGIN_ID, IMPORT_BUNDLE, NEW_FOLDER};
    match entry.source {
        EntrySource::ShellBuiltin if entry.plugin_id == BUILTIN_PLUGIN_ID => {
            match entry.entry.id.as_str() {
                NEW_FOLDER => flow_new_folder(ctx.clone()),
                IMPORT_BUNDLE => flow_import_bundle(ctx.clone()),
                other => log::warn!("unknown shell builtin entry: {other}"),
            }
        }
        EntrySource::ShellBuiltin => {
            log::warn!("mismatched shell builtin plugin_id: {}", entry.plugin_id);
        }
        EntrySource::Plugin(_) if entry.plugin_id == stroke_canvas::KIND_ID => {
            match entry.entry.id.as_str() {
                stroke_canvas::NEW_CANVAS => flow_new_canvas(ctx.clone()),
                stroke_canvas::NEW_EPHEMERAL => flow_new_ephemeral(ctx.clone()),
                stroke_canvas::QUICK_SKETCH => flow_quick_sketch(ctx.clone()),
                other => log::warn!("unknown stroke canvas entry: {other}"),
            }
        }
        EntrySource::Plugin(_) if entry.plugin_id == pdf_source::KIND_ID => {
            match entry.entry.id.as_str() {
                pdf_source::IMPORT_PDF => flow_import_pdf(ctx.clone()),
                other => log::warn!("unknown pdf source entry: {other}"),
            }
        }
        EntrySource::Plugin(_) if entry.plugin_id == "io.notein.bible.viewer" => {
            match entry.entry.id.as_str() {
                "new-bible-viewer" => flow_open_bible(ctx.clone()),
                other => log::warn!("unknown bible viewer entry: {other}"),
            }
        }
        EntrySource::Plugin(_) if entry.plugin_id == notein_notes::KIND_ID => {
            match entry.entry.id.as_str() {
                notein_notes::canvas::NEW_NOTE => flow_new_note(ctx.clone()),
                notein_notes::canvas::IMPORT_MARKDOWN => flow_import_markdown(ctx.clone()),
                other => log::warn!("unknown notes entry: {other}"),
            }
        }
        EntrySource::Plugin(_) if entry.plugin_id == "io.notein.bible" => {
            match entry.entry.id.as_str() {
                "install-bible" => fab_install_bible(),
                other => log::warn!("unknown bible source entry: {other}"),
            }
        }
        EntrySource::Plugin(_) => {
            log::info!(
                "plugin creation entry {}/{} — execute_creation wiring lands with Phase 2.6+",
                entry.plugin_id,
                entry.entry.id
            );
        }
    }
}

fn flow_new_note(ctx: FabCtx) {
    use crate::library::{BackgroundStyle, DocKind};
    let FabCtx {
        handle,
        snap,
        current,
    } = ctx;
    let mut snap = snap;
    spawn_forever(async move {
        let mut lib = handle.lock().await;
        match lib
            .create_item(
                current,
                DocKind::plugin(notein_notes::KIND_ID),
                "Nota",
                None,
                Vec::new(),
                BackgroundStyle::default(),
            )
            .await
        {
            Ok(id) => {
                snap.set(Some(lib.index().clone()));
                crate::builtin::prime_notes(lib.index().items.iter());
                crate::space::open(TabTarget::Item(id), OpenHint::Focused);
            }
            Err(e) => log::warn!("notes: create_item failed: {e}"),
        }
    });
}

/// Pick a markdown file and open it as a new text note.
fn flow_import_markdown(ctx: FabCtx) {
    use crate::library::{BackgroundStyle, DocKind};
    let FabCtx {
        handle,
        snap,
        current,
    } = ctx;
    let mut snap = snap;
    spawn_forever(async move {
        let Some((file_name, markdown)) = pick_markdown().await else {
            return;
        };
        let name = notein_notes::import_name(Some(&file_name), &markdown);
        let mut lib = handle.lock().await;
        let created = lib
            .create_item(
                current,
                DocKind::plugin(notein_notes::KIND_ID),
                &name,
                None,
                Vec::new(),
                BackgroundStyle::default(),
            )
            .await;
        let id = match created {
            Ok(id) => id,
            Err(err) => {
                log::warn!("notes import: create_item failed: {err}");
                return;
            }
        };
        if let Err(err) = crate::library::bodies::write_plugin_snapshot(
            id,
            notein_notes::KIND_ID,
            markdown.as_bytes(),
        ) {
            log::warn!("notes import: snapshot write failed: {err}");
        }
        crate::sync::snapshot_changed(id);
        crate::library::refs::record_note(id, &markdown);
        snap.set(Some(lib.index().clone()));
        crate::builtin::prime_notes(lib.index().items.iter());
        drop(lib);
        crate::space::open(TabTarget::Item(id), OpenHint::Focused);
    });
}

/// Ask for a markdown file; returns its name and text.
async fn pick_markdown() -> Option<(String, String)> {
    use std::io::Read;
    use std::sync::Arc as StdArc;

    use istmo_file_picker::{FileFilter, FilePickerClient, PickConfig};

    let picker = FilePickerClient::acquire()
        .inspect_err(|err| log::error!("file picker unavailable: {err:?}"))
        .ok()?;
    let config = PickConfig {
        dialog_title: Some("Elegí un archivo markdown".into()),
        filter: FileFilter {
            mime_types: notein_notes::IMPORT_MIME_TYPES.map(str::to_owned).to_vec(),
            extensions: notein_notes::IMPORT_EXTENSIONS.map(str::to_owned).to_vec(),
            uti_types: vec![
                "net.daringfireball.markdown".into(),
                "public.plain-text".into(),
            ],
        },
        start_directory_hint: None,
        allow_multiple: false,
    };
    let file = match picker.pick_file_owned(config).await {
        Ok(Some(file)) => file,
        Ok(None) => return None,
        Err(err) => {
            log::error!("picker call error: {err:?}");
            return None;
        }
    };
    let name = file.display_name.clone();
    let file = StdArc::new(file);
    let mut reader = file
        .open_reader(&picker)
        .await
        .inspect_err(|err| log::error!("open reader: {err:?}"))
        .ok()?;
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .inspect_err(|err| log::error!("read markdown: {err}"))
        .ok()?;
    Some((name, String::from_utf8_lossy(&bytes).into_owned()))
}

fn flow_open_bible(ctx: FabCtx) {
    use crate::library::{BackgroundStyle, DocKind};
    let FabCtx {
        handle,
        snap,
        current,
    } = ctx;
    let mut snap = snap;
    spawn_forever(async move {
        let mut lib = handle.lock().await;
        let init_bytes =
            bincode::encode_to_vec(&String::new(), bincode::config::standard()).unwrap_or_default();
        match lib
            .create_item(
                current,
                DocKind::plugin(notein_bible::canvas::KIND_ID).with_init(init_bytes),
                "Biblia",
                None,
                Vec::new(),
                BackgroundStyle::default(),
            )
            .await
        {
            Ok(id) => {
                snap.set(Some(lib.index().clone()));
                crate::space::open(TabTarget::Item(id), OpenHint::Focused);
            }
            Err(e) => log::warn!("bible: create_item failed: {e}"),
        }
    });
}

/// Open the Bible install modal. Shared entry point used by the
/// Bible empty-state, the FAB `+` menu's "install-bible" action and
/// the translation picker's "Agregar traducción…" row.
pub fn fab_install_bible() {
    crate::bible_install_modal::open_install_modal();
}

fn flow_new_ephemeral(_ctx: FabCtx) {
    let target = TabTarget::Scratch(crate::library::ScratchId::fresh());
    crate::route::queue_canvas_background(
        target,
        BackgroundStyle::default(),
        crate::route::DEFAULT_PAPER,
    );
    crate::space::open(target, OpenHint::Focused);
}

fn flow_quick_sketch(ctx: FabCtx) {
    let FabCtx {
        handle,
        snap,
        current,
    } = ctx;
    let mut snap = snap;
    spawn_forever(async move {
        let mut lib = handle.lock().await;
        match lib
            .create_item(
                current,
                DocKind::plugin(crate::builtin::stroke_canvas::KIND_ID),
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
                    TabTarget::Item(id),
                    BackgroundStyle::default(),
                    crate::route::DEFAULT_PAPER,
                );
                crate::space::open(TabTarget::Item(id), OpenHint::Focused);
            }
            Err(err) => log::error!("create_item: {err}"),
        }
    });
}

fn flow_new_canvas(ctx: FabCtx) {
    use crate::components::{CanvasCreateSheet, CreateCanvasRequest};
    let FabCtx {
        handle,
        snap,
        current,
    } = ctx;
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
                    crate::route::queue_canvas_background(
                        TabTarget::Item(id),
                        bg_style,
                        surface.into(),
                    );
                    crate::space::open(TabTarget::Item(id), OpenHint::Focused);
                }
                Err(err) => log::error!("create_item: {err}"),
            }
        });
    })
    .kind(DocKind::plugin(crate::builtin::stroke_canvas::KIND_ID))
    .available_tags(available)
    .open();
}

fn flow_new_folder(ctx: FabCtx) {
    use crate::components::{CreateFolderRequest, FolderCreateSheet};
    let FabCtx {
        handle,
        snap,
        current,
    } = ctx;
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
                .create_folder(current, &req.name, Some(color_to_rgba(req.color)), tag_ids)
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

fn flow_import_pdf(ctx: FabCtx) {
    let FabCtx {
        handle,
        snap,
        current,
    } = ctx;
    spawn_forever(pick_and_open_pdf(handle, snap, current));
}

fn flow_import_bundle(ctx: FabCtx) {
    let FabCtx { handle, snap, .. } = ctx;
    spawn_forever(pick_and_import_bundle(handle, snap));
}

async fn pick_and_import_bundle(handle: LibHandle, snap: State<Option<LibraryIndex>>) {
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
        dialog_title: Some("Importar bundle .notein".into()),
        filter: FileFilter {
            mime_types: vec!["application/octet-stream".into()],
            extensions: vec!["notein".into()],
            uti_types: Vec::new(),
        },
        start_directory_hint: None,
        allow_multiple: false,
    };
    let picked = match picker.pick_file_owned(config).await {
        Ok(Some(f)) => f,
        Ok(None) => return,
        Err(err) => {
            log::error!("pick bundle: {err:?}");
            return;
        }
    };
    let owned = StdArc::new(picked);
    let mut reader = match owned.open_reader(&picker).await {
        Ok(r) => r,
        Err(err) => {
            log::error!("open bundle reader: {err:?}");
            return;
        }
    };
    let mut bytes = Vec::new();
    if let Err(err) = reader.read_to_end(&mut bytes) {
        log::error!("read bundle: {err}");
        return;
    }
    drop(reader);
    drop(owned);

    let parsed = match crate::bundle::parse_bundle(std::io::Cursor::new(bytes)) {
        Ok(p) => p,
        Err(err) => {
            show_import_error_modal(format!("No se pudo leer el bundle: {err}"));
            return;
        }
    };

    let registry_arc = crate::plugins::registry();
    let (collisions, missing_plugins) = {
        let lib = handle.lock().await;
        let registry = crate::plugins::lock(&registry_arc);
        let preflight = parsed.preflight(&lib, &registry);
        (preflight.clone(), preflight.missing_plugins.clone())
    };
    drop(registry_arc);

    if collisions.has_collisions() {
        show_import_collision_modal(&collisions);
        return;
    }

    let parsed = StdArc::new(parsed);
    let on_confirm = {
        let handle = handle.clone();
        let parsed = parsed.clone();
        move || {
            let handle = handle.clone();
            let parsed = parsed.clone();
            let mut snap = snap;
            spawn_forever(async move {
                let mut lib = handle.lock().await;
                match parsed.merge(&mut lib).await {
                    Ok(()) => snap.set(Some(lib.index().clone())),
                    Err(err) => {
                        drop(lib);
                        show_import_error_modal(format!("Error al importar: {err}"));
                    }
                }
            });
        }
    };

    if missing_plugins.is_empty() {
        on_confirm();
    } else {
        show_import_warning_modal(missing_plugins, on_confirm);
    }
}

fn show_import_error_modal(message: String) {
    use crate::components::{Modal, ModalController};
    let body = rect()
        .vertical()
        .width(Size::px(320.0))
        .padding((20.0, 20.0))
        .background(Color::from_rgb(30, 30, 34))
        .with_corner_radius(14.0)
        .spacing(14.0)
        .child(
            label()
                .color(Color::from_rgb(240, 240, 245))
                .font_size(14.0)
                .text("Importar bundle"),
        )
        .child(
            label()
                .color(Color::from_rgb(190, 190, 200))
                .font_size(13.0)
                .text(message),
        )
        .child(
            rect()
                .horizontal()
                .content(Content::Flex)
                .cross_align(Alignment::Center)
                .child(flexible_spacer())
                .child(
                    text_button_card("Cerrar", Color::from_rgb(55, 55, 62))
                        .on_press(|_: Event<PressEventData>| ModalController::get().close()),
                ),
        );
    ModalController::get().open(Modal::new(body).center());
}

fn show_import_collision_modal(preflight: &crate::bundle::Preflight) {
    let mut lines = Vec::new();
    if !preflight.colliding_items.is_empty() {
        lines.push(format!(
            "Items en conflicto: {}",
            preflight.colliding_items.len()
        ));
    }
    if !preflight.colliding_folders.is_empty() {
        lines.push(format!(
            "Carpetas en conflicto: {}",
            preflight.colliding_folders.len()
        ));
    }
    if !preflight.colliding_tags.is_empty() {
        lines.push(format!(
            "Tags en conflicto: {}",
            preflight.colliding_tags.len()
        ));
    }
    if !preflight.colliding_workspaces.is_empty() {
        lines.push(format!(
            "Workspaces en conflicto: {}",
            preflight.colliding_workspaces.len()
        ));
    }
    let body = lines.join("\n");
    show_import_error_modal(format!(
        "El bundle contiene ids que ya existen en tu biblioteca. Importar fue cancelado para evitar sobreescribir contenido.\n\n{body}"
    ));
}

fn show_import_warning_modal(
    missing_plugins: Vec<String>,
    mut on_confirm: impl FnMut() + Clone + 'static,
) {
    use crate::components::{Modal, ModalController};
    let listing = missing_plugins.join(", ");
    let body = rect()
        .vertical()
        .width(Size::px(360.0))
        .padding((20.0, 20.0))
        .background(Color::from_rgb(30, 30, 34))
        .with_corner_radius(14.0)
        .spacing(14.0)
        .child(
            label()
                .color(Color::from_rgb(240, 240, 245))
                .font_size(14.0)
                .text("Plugins faltantes"),
        )
        .child(
            label()
                .color(Color::from_rgb(190, 190, 200))
                .font_size(13.0)
                .text(format!(
                    "Este bundle referencia plugins no instalados. Los chips quedarán con advertencia hasta que instales:\n\n{listing}"
                )),
        )
        .child(
            rect()
                .horizontal()
                .spacing(10.0)
                .content(Content::Flex)
                .cross_align(Alignment::Center)
                .child(flexible_spacer())
                .child(
                    text_button_card("Cancelar", Color::from_rgb(55, 55, 62))
                        .on_press(|_: Event<PressEventData>| ModalController::get().close()),
                )
                .child(
                    text_button_card("Importar igual", Color::from_rgb(65, 105, 220))
                        .on_press(move |_: Event<PressEventData>| {
                            ModalController::get().close();
                            on_confirm();
                        }),
                ),
        );
    ModalController::get().open(Modal::new(body).center());
}

fn text_button_card(text: &str, bg: Color) -> Rect {
    rect()
        .height(Size::px(34.0))
        .padding((0.0, 14.0))
        .center()
        .with_corner_radius(8.0)
        .background(bg)
        .child(
            label()
                .color(Color::from_rgb(240, 240, 245))
                .font_size(13.0)
                .text(text.to_owned()),
        )
}

fn fab_button<F: Fn(()) + 'static>(icon: Bytes, bg: Color, handler: F) -> Rect {
    rect()
        .width(Size::px(52.0))
        .height(Size::px(52.0))
        .background(bg)
        .with_corner_radius(26.0)
        .center()
        .child(
            SvgViewer::new(icon)
                .color(Color::WHITE)
                .width(Size::px(24.0))
                .height(Size::px(24.0)),
        )
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
    // Rebuild the shell's completion catalog from the freshly-loaded
    // library so the `[[` popup lists every PDF the user has
    // imported, even those not opened yet this session.
    crate::completion_catalog::prime_pdf_entries(lib.index().items.iter());
    crate::builtin::prime_notes(lib.index().items.iter());
    Ok(Arc::new(AsyncMutex::new(lib)))
}

/// Open the canvas-config sheet pre-filled with the current state of
/// `item_id`. Resolves name / color / background / tags from the
/// library, then opens the same `CanvasCreateSheet` the create flow
/// uses — the sheet grew `default_*` + `title_override` + `submit_label`
/// builders so this edit flow reuses it instead of maintaining a
/// parallel component. On confirm, applies the changes via the
/// `rename_item` / `set_item_*` setters.
pub(crate) fn open_canvas_edit_sheet(
    item_id: ItemId,
    mut snap: Option<State<Option<LibraryIndex>>>,
) {
    use crate::components::{CanvasCreateSheet, CreateCanvasRequest};
    // `spawn_forever` so the task lives on the root scope — the
    // context-menu button that calls us unmounts right after press,
    // which would cancel a plain `spawn` task before the library
    // handle even resolves.
    spawn_forever(async move {
        let handle = match open_library().await {
            Ok(h) => h,
            Err(err) => {
                log::error!("open library on edit sheet: {err}");
                return;
            }
        };
        let (item, current_tag_names, available_tag_names) = {
            let lib = handle.lock().await;
            let Some(item) = lib.index().item(item_id).cloned() else {
                log::error!("edit sheet: item {item_id} not found");
                return;
            };
            let current = item
                .tags
                .iter()
                .filter_map(|tid| lib.index().tag(*tid).map(|t| t.name.clone()))
                .collect::<Vec<_>>();
            let available = lib
                .tags()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>();
            (item, current, available)
        };
        let default_color = item.color.map(rgba_to_color);
        let kind = item.kind.clone();
        let apply_handle = handle.clone();
        CanvasCreateSheet::new(move |req: CreateCanvasRequest| {
            let handle = apply_handle.clone();
            // `spawn_forever` so the save survives the sheet's own
            // unmount on confirm.
            spawn_forever(async move {
                let mut lib = handle.lock().await;
                let tag_ids = resolve_tag_names(&mut lib, &req.tag_names).await;
                if let Err(err) = lib.rename_item(item_id, &req.name).await {
                    log::error!("rename_item: {err}");
                }
                if let Err(err) = lib
                    .set_item_color(item_id, Some(color_to_rgba(req.color)))
                    .await
                {
                    log::error!("set_item_color: {err}");
                }
                if let Err(err) = lib.set_item_background(item_id, req.background).await {
                    log::error!("set_item_background: {err}");
                }
                if let Err(err) = lib.set_item_tags(item_id, tag_ids).await {
                    log::error!("set_item_tags: {err}");
                }
                // Push the fresh tint + pattern into the live board if
                // the item is currently mounted — the canvas repaints
                // its background right away instead of waiting for the
                // next route mount.
                if let Some(board) = crate::active_boards::get_item(item_id) {
                    crate::route::apply_canvas_background(&board, req.background, req.color.into());
                }
                // Refresh the home grid snapshot so the renamed title /
                // retinted swatch / new tags land on screen immediately
                // instead of waiting for the next focus-driven reload.
                // `None` when the sheet is opened from canvas chrome —
                // no grid to repaint in that view.
                if let Some(snap) = snap.as_mut() {
                    snap.set(Some(lib.index().clone()));
                }
            });
        })
        .kind(kind)
        .default_name(item.name.clone())
        .default_color(default_color)
        .default_background(item.background)
        .default_tags(current_tag_names)
        .available_tags(available_tag_names)
        .title_override("Editar lienzo")
        .submit_label("Guardar")
        .open();
    });
}

/// Folder-side counterpart to [`open_canvas_edit_sheet`]. Resolves the
/// folder's current name / color / tags from the library, then opens the
/// same `FolderCreateSheet` the create flow uses (configured with
/// `default_*` + `title_override` + `submit_label`). On confirm, applies
/// the changes via the `rename_folder` / `set_folder_*` setters.
pub(crate) fn open_folder_edit_sheet(
    folder_id: FolderId,
    mut snap: Option<State<Option<LibraryIndex>>>,
) {
    use crate::components::{CreateFolderRequest, FolderCreateSheet};
    // See `open_canvas_edit_sheet` — root-scope task so it survives
    // the menu-button unmount that follows the press.
    spawn_forever(async move {
        let handle = match open_library().await {
            Ok(h) => h,
            Err(err) => {
                log::error!("open library on folder edit sheet: {err}");
                return;
            }
        };
        let (folder, current_tag_names, available_tag_names) = {
            let lib = handle.lock().await;
            let Some(folder) = lib.index().folder(folder_id).cloned() else {
                log::error!("folder edit sheet: folder {folder_id:?} not found");
                return;
            };
            let current = folder
                .tags
                .iter()
                .filter_map(|tid| lib.index().tag(*tid).map(|t| t.name.clone()))
                .collect::<Vec<_>>();
            let available = lib
                .tags()
                .iter()
                .map(|t| t.name.clone())
                .collect::<Vec<_>>();
            (folder, current, available)
        };
        let default_color = folder.color.map(rgba_to_color);
        let apply_handle = handle.clone();
        FolderCreateSheet::new(move |req: CreateFolderRequest| {
            let handle = apply_handle.clone();
            spawn_forever(async move {
                let mut lib = handle.lock().await;
                let tag_ids = resolve_tag_names(&mut lib, &req.tag_names).await;
                if let Err(err) = lib.rename_folder(folder_id, &req.name).await {
                    log::error!("rename_folder: {err}");
                }
                if let Err(err) = lib
                    .set_folder_color(folder_id, Some(color_to_rgba(req.color)))
                    .await
                {
                    log::error!("set_folder_color: {err}");
                }
                if let Err(err) = lib.set_folder_tags(folder_id, tag_ids).await {
                    log::error!("set_folder_tags: {err}");
                }
                if let Some(snap) = snap.as_mut() {
                    snap.set(Some(lib.index().clone()));
                }
            });
        })
        .default_name(folder.name.clone())
        .default_color(default_color)
        .default_tags(current_tag_names)
        .available_tags(available_tag_names)
        .title_override("Editar carpeta")
        .submit_label("Guardar")
        .open();
    });
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
                    DocKind::plugin(crate::builtin::pdf_source::KIND_ID),
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
                    crate::space::open(TabTarget::Item(id), OpenHint::Focused);
                }
                Err(err) => log::error!("create_item: {err}"),
            }
        });
    })
    .kind(DocKind::plugin(crate::builtin::pdf_source::KIND_ID))
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
