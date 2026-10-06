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
use freya::icons::lucide::search;
use freya::prelude::*;
use freya::router::*;
use istmo::plugins::EdgeInsets;

use crate::components::FormInput;
use crate::components::drag_drop::{DragZone, DropZone};
use crate::components::{
    CanvasCreateSheet, CreateCanvasRequest, Modal, ModalController, SettingsModal, WorkspaceSwitcher,
    WorkspaceSwitcherEntry, auto_color,
};
use crate::hooks::use_safe_area_insets;
use crate::library::{
    BackgroundStyle, Folder, FolderId, Item, ItemId, ItemKind, Library, LibraryIndex, ROOT_FOLDER,
    Rgba, TagId, Workspace, WorkspaceId,
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

/// Movement tolerance before a pending long-press is cancelled. Touch
/// and stylus emit tiny `PointerMove`s during a static hold; a strict
/// "any move cancels" rule meant the context menu never opened for
/// finger/pen input.
const LONG_PRESS_SLOP_PX: f64 = 10.0;

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

impl WorkspaceSnapshot {
    fn active_name(&self) -> Option<&str> {
        self.active
            .and_then(|id| self.workspaces.iter().find(|w| w.id == id))
            .map(|w| w.name.as_str())
    }
}

/// Landing page.
#[derive(Debug, PartialEq)]
pub struct Home;

impl Component for Home {
    fn render(&self) -> impl IntoElement {
        let lib_state = use_state(|| LibState::Loading);
        let snap = use_state(|| Option::<LibraryIndex>::None);
        let ws_snap = use_state(|| Option::<WorkspaceSnapshot>::None);
        let nav = use_state(Vec::<FolderId>::new);
        let renaming = use_state(|| Option::<CardId>::None);
        let rename_buffer = use_state(String::new);
        let pad = *use_safe_area_insets().read();
        // Capture the `RouterContext` from Home's render scope (which
        // lives inside `Router`). Modal / FabMenu bodies render under
        // `ModalPortal`, a *sibling* of the Router in `AppShell` — any
        // `RouterContext::get()` call from those scopes panics. The
        // captured handle is `Copy`, so cloning it into every downstream
        // closure is free.
        let router = RouterContext::get();

        use_hook({
            let mut lib_state = lib_state;
            let mut snap = snap;
            let mut ws_snap = ws_snap;
            move || {
                spawn(async move {
                    match open_library().await {
                        Ok(handle) => {
                            let lib = handle.lock().await;
                            snap.set(Some(lib.index().clone()));
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

        let bg = Color::from_rgb(15, 15, 18);
        let state_val = lib_state.read().clone();
        let snapshot = snap.read().clone();
        let ws_snapshot = ws_snap.read().clone();
        let nav_stack = nav.read().clone();
        let current = nav_stack.last().copied().unwrap_or(ROOT_FOLDER);

        let ws_pill_target = match (&state_val, ws_snapshot.as_ref()) {
            (LibState::Ready(handle), Some(snap)) => Some((handle.clone(), snap.clone())),
            _ => None,
        };
        let mut column = rect()
            .vertical()
            .expanded()
            .padding((pad.top, pad.right, 0.0, pad.left))
            .child(top_bar(ws_pill_target, ws_snap));
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
                    .child(breadcrumb_bar(
                        &index,
                        &nav_stack,
                        nav,
                        handle.clone(),
                        snap,
                    ))
                    .child(grid(
                        &index,
                        current,
                        snap,
                        nav,
                        handle.clone(),
                        renaming,
                        rename_buffer,
                    ));
                fab_slot = Some(fab_stack(handle, snap, current, pad, router));
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

fn top_bar(
    workspace_pill: Option<(LibHandle, WorkspaceSnapshot)>,
    ws_snap: State<Option<WorkspaceSnapshot>>,
) -> impl IntoElement {
    let bg = Color::from_rgb(15, 15, 18);
    let search_value = use_state(String::default);
    let left = match workspace_pill {
        Some((handle, ws)) => workspace_pill_button(handle, ws, ws_snap).into_element(),
        None => icon_button("☰").into_element(),
    };
    rect()
        .horizontal()
        .width(Size::fill())
        .height(Size::px(56.0))
        .padding((10.0, 16.0))
        .background(bg)
        .content(Content::Flex)
        .cross_align(Alignment::Center)
        .child(left)
        .child(spacer_px(6.0))
        .child(flexible_spacer())
        .child(
            FormInput::new(search_value)
                .left_icon(search())
                .collapsible()
                .corner_radius(99.)
                .width(Size::px(400.))
                .min_width(Size::px(40.))
                .placeholder("Search"),
        )
        .child(spacer_px(6.0))
        .child(icon_button("⚙").on_press(|_| SettingsModal::open()))
}

/// Pill rendered where the hamburger lived: shows the active workspace
/// name and opens the switcher dropdown on press.
fn workspace_pill_button(
    handle: LibHandle,
    ws: WorkspaceSnapshot,
    ws_snap: State<Option<WorkspaceSnapshot>>,
) -> Rect {
    let name = ws.active_name().unwrap_or("Sin workspace").to_owned();
    let entries: Vec<WorkspaceSwitcherEntry> = ws
        .workspaces
        .iter()
        .map(|w| WorkspaceSwitcherEntry {
            id: w.id,
            label: w.name.clone().into(),
            swatch: w.color.map(|c| Color::from_rgb(c[0], c[1], c[2])),
            is_active: ws.active == Some(w.id),
        })
        .collect();
    let handle_for_pick = handle.clone();
    let handle_for_create = handle;
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .height(Size::px(32.0))
        .padding((0.0, 12.0))
        .with_corner_radius(16.0)
        .background(Color::from_rgb(28, 28, 32))
        .spacing(6.0)
        .on_press(move |_| {
            let entries = entries.clone();
            let pick_handle = handle_for_pick.clone();
            let create_handle = handle_for_create.clone();
            let ws_snap_pick = ws_snap;
            let ws_snap_create = ws_snap;
            WorkspaceSwitcher::new(
                entries,
                Callback::new(move |id: WorkspaceId| {
                    let handle = pick_handle.clone();
                    let mut ws_snap = ws_snap_pick;
                    spawn(async move {
                        let mut lib = handle.lock().await;
                        if let Err(err) = lib.set_active_workspace(Some(id)).await {
                            log::error!("set active workspace: {err}");
                            return;
                        }
                        ws_snap.set(Some(snapshot_workspaces(&lib)));
                    });
                }),
                NoArgCallback::new(move || {
                    let handle = create_handle.clone();
                    let mut ws_snap = ws_snap_create;
                    spawn(async move {
                        let mut lib = handle.lock().await;
                        let count = lib.workspaces().len();
                        let name = format!("Workspace {}", count + 1);
                        if let Err(err) = lib.create_workspace(&name, None).await {
                            log::error!("create workspace: {err}");
                            return;
                        }
                        ws_snap.set(Some(snapshot_workspaces(&lib)));
                    });
                }),
            )
            .open();
        })
        .child(
            label()
                .color(Color::from_rgb(230, 230, 240))
                .font_size(13.0)
                .text(name),
        )
        .child(
            label()
                .color(Color::from_rgb(170, 170, 180))
                .font_size(12.0)
                .text("▾"),
        )
}

/// Snapshot the library's workspace index into a UI-side struct. Keeps
/// the switcher independent of the async-locked [`Library`].
fn snapshot_workspaces(lib: &Library) -> WorkspaceSnapshot {
    WorkspaceSnapshot {
        active: lib.active_workspace(),
        workspaces: lib.workspaces().to_vec(),
    }
}

fn icon_button(glyph: &'static str) -> Rect {
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
        let press_start = use_state(|| Option::<CursorPoint>::None);

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
        // Capture at render time: on_press fires in-scope here, but
        // long-press opens a `ContextMenu` whose entries run under the
        // menu overlay — outside Router. Pre-captured handle sidesteps
        // the ambiguity.
        let router = RouterContext::get();

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
            crate::route::queue_canvas_background(bg_style, rgba_to_color(tint).into());
            let route = match kind {
                ItemKind::Canvas => Route::CanvasView { id: id.0 },
                ItemKind::PdfCanvas => Route::CanvasPdfView { id: id.0 },
            };
            let _ = router.push(route);
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
            focus,
            rename_buffer,
            renaming,
            on_commit_rename,
            open,
            menu_builder,
            press_token,
            long_fired,
            press_start,
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
        let press_start = use_state(|| Option::<CursorPoint>::None);

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
            build_card_menu(start_rename, || log::info!("edit folder: TODO"), ask_delete)
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
            focus,
            rename_buffer,
            renaming,
            on_commit_rename,
            open,
            menu_builder,
            press_token,
            long_fired,
            press_start,
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
fn card_body_view<Open, Commit>(
    title: String,
    subtitle: String,
    tint: Rgba,
    is_folder: bool,
    is_editing: bool,
    focus: AccessibilityId,
    rename_buffer: State<String>,
    renaming: State<Option<CardId>>,
    on_commit_rename: Commit,
    open: Open,
    menu_builder: Menu,
    press_token: State<u64>,
    long_fired: State<bool>,
    press_start: State<Option<CursorPoint>>,
) -> impl IntoElement
where
    Open: Fn() + Clone + 'static,
    Commit: Fn(String) + Clone + 'static,
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

    let menu_for_secondary = menu_builder.clone();
    let column = column.on_secondary_down(move |_: Event<PressEventData>| {
        ContextMenu::open(menu_for_secondary.clone());
    });

    let menu_for_long = menu_builder.clone();
    let column = column.on_pointer_down({
        let mut long_fired = long_fired;
        let mut press_token = press_token;
        let mut press_start = press_start;
        move |e: Event<PointerEventData>| {
            long_fired.set(false);
            press_start.set(Some(e.global_location()));
            let token = press_token.peek().wrapping_add(1);
            press_token.set(token);
            let menu = menu_for_long.clone();
            spawn(async move {
                async_io::Timer::after(LONG_PRESS).await;
                if *press_token.peek() == token {
                    long_fired.set(true);
                    ContextMenu::open(menu);
                }
            });
        }
    });

    let column = column.on_press({
        let open = open.clone();
        let mut press_token = press_token;
        let mut press_start = press_start;
        let long_fired = long_fired;
        let editing_now = is_editing;
        move |_e: Event<PressEventData>| {
            press_token.set(0);
            press_start.set(None);
            if editing_now || *long_fired.peek() {
                return;
            }
            open();
        }
    });

    let mut press_token_move = press_token;
    let mut press_start_move = press_start;
    let mut press_token_leave = press_token;
    let mut press_start_leave = press_start;
    column
        .on_pointer_move(move |e: Event<PointerEventData>| {
            let Some(start) = *press_start_move.peek() else {
                return;
            };
            let current = e.global_location();
            let dx = current.x - start.x;
            let dy = current.y - start.y;
            if (dx * dx + dy * dy).sqrt() >= LONG_PRESS_SLOP_PX {
                press_token_move.set(0);
                press_start_move.set(None);
            }
        })
        .on_pointer_leave(move |_| {
            press_token_leave.set(0);
            press_start_leave.set(None);
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

#[derive(Clone)]
struct FabCtx {
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
    current: FolderId,
    router: RouterContext,
}

fn fab_stack(
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
    current: FolderId,
    pad: EdgeInsets,
    router: RouterContext,
) -> Rect {
    let ctx = FabCtx {
        handle,
        snap,
        current,
        router,
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
        fab_button("+", plus_bg, move |_| {
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
            let icon = entry.entry.icon.clone();
            fab_button_owned(icon, bg, move |_| {
                trigger_entry(&entry, &ctx);
            })
        }
        None => {
            // Empty slot: subdued bubble that opens settings so the
            // user can pick what sits here.
            let bg = Color::from_rgb(38, 38, 44);
            fab_button("＋", bg, |_| {
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
            let icon = entry.entry.icon.clone();
            let label = menu_label(&entry);
            let mut row = FabMenuEntry::new(icon, label, move || {
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
    use crate::shell_builtins::{BUILTIN_PLUGIN_ID, NEW_FOLDER};
    match entry.source {
        EntrySource::ShellBuiltin if entry.plugin_id == BUILTIN_PLUGIN_ID => {
            match entry.entry.id.as_str() {
                NEW_FOLDER => flow_new_folder(ctx.clone()),
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
        EntrySource::Plugin(_) => {
            log::info!(
                "plugin creation entry {}/{} — execute_creation wiring lands with Phase 2.6+",
                entry.plugin_id,
                entry.entry.id
            );
        }
    }
}

fn flow_new_ephemeral(ctx: FabCtx) {
    crate::route::queue_canvas_background(BackgroundStyle::default(), crate::route::DEFAULT_PAPER);
    let _ = ctx.router.push(Route::CanvasEphemeral);
}

fn flow_quick_sketch(ctx: FabCtx) {
    let FabCtx {
        handle,
        snap,
        current,
        router,
    } = ctx;
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
                let _ = router.push(Route::CanvasView { id: id.0 });
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
        router,
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
                    crate::route::queue_canvas_background(bg_style, surface.into());
                    let _ = router.push(Route::CanvasView { id: id.0 });
                }
                Err(err) => log::error!("create_item: {err}"),
            }
        });
    })
    .kind(ItemKind::Canvas)
    .available_tags(available)
    .open();
}

fn flow_new_folder(ctx: FabCtx) {
    use crate::components::{CreateFolderRequest, FolderCreateSheet};
    let FabCtx {
        handle,
        snap,
        current,
        router: _,
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
        router,
    } = ctx;
    spawn_forever(pick_and_open_pdf(handle, snap, current, router));
}

fn fab_button<F: Fn(()) + 'static>(glyph: &'static str, bg: Color, handler: F) -> Rect {
    rect()
        .width(Size::px(52.0))
        .height(Size::px(52.0))
        .background(bg)
        .with_corner_radius(26.0)
        .center()
        .child(label().color(Color::WHITE).font_size(22.0).text(glyph))
        .on_press(move |_| handler(()))
}

fn fab_button_owned<F: Fn(()) + 'static>(glyph: String, bg: Color, handler: F) -> Rect {
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
/// and open the canvas-create sheet pre-filled with its name. `router`
/// must be captured in the caller's component scope — this async body
/// runs on the root task scope and cannot look it up itself.
async fn pick_and_open_pdf(
    handle: LibHandle,
    snap: State<Option<LibraryIndex>>,
    parent: FolderId,
    router: RouterContext,
) {
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
                    let _ = router.push(Route::CanvasPdfView { id: id.0 });
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
