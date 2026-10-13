//! Phase 4 shell — mounts [`PaneTreeView`] against the active
//! workspace, wires its callbacks into the async [`Library`] helpers,
//! and installs the desktop keyboard shortcuts (Ctrl+T / Ctrl+W /
//! Ctrl+Tab) targeted at the focused pane.
//!
//! Loaded by [`crate::route::Route::WorkspaceView`]. The route value
//! carries the workspace id as a plain `u32` so the deeplink surface
//! stays serde-free; the view promotes it to [`WorkspaceId`] on open.

use std::collections::HashMap;

use freya::prelude::*;
use freya::router::*;

use crate::components::{PaneTreeCallbacks, PaneTreeView};
use crate::components::pane_tree_view::SplitEdge;
use crate::home::{LibHandle, open_library};
use crate::library::{
    BackgroundStyle, ItemId, ItemKind, Library, PaneId, PaneTab, SplitOrientation, SplitSide,
    Workspace, WorkspaceId,
};
use crate::route::Route;

/// Fallback title for a tab whose underlying library item is missing
/// (e.g. the item was deleted on another device and the tab snapshot
/// did not yet catch up).
const MISSING_TITLE: &str = "(elemento no disponible)";

#[derive(Clone, PartialEq, Debug)]
struct WorkspaceSnapshot {
    workspace: Workspace,
    titles: HashMap<ItemId, String>,
    kinds: HashMap<ItemId, ItemKind>,
}

#[derive(Clone)]
enum LoadState {
    Loading,
    Ready(LibHandle, WorkspaceSnapshot),
    Error(String),
}

/// Pane-tree shell for a persisted workspace.
#[derive(Debug, PartialEq)]
pub struct WorkspaceView {
    pub id: WorkspaceId,
}

impl Component for WorkspaceView {
    fn render(&self) -> impl IntoElement {
        let workspace_id = self.id;
        let load_state = use_state(|| LoadState::Loading);
        let router = RouterContext::get();

        use_hook({
            let mut load_state = load_state;
            move || {
                spawn(async move {
                    match open_library().await {
                        Ok(handle) => {
                            let snap = {
                                let lib = handle.lock().await;
                                snapshot_workspace(&lib, workspace_id)
                            };
                            match snap {
                                Some(snap) => load_state.set(LoadState::Ready(handle, snap)),
                                None => load_state.set(LoadState::Error(format!(
                                    "workspace {workspace_id} no existe"
                                ))),
                            }
                        }
                        Err(err) => load_state.set(LoadState::Error(err)),
                    }
                });
            }
        });

        let bg = Color::from_rgb(15, 15, 18);
        let state_val = load_state.read().clone();

        match state_val {
            LoadState::Loading => rect()
                .expanded()
                .background(bg)
                .center()
                .child(centered_text("Cargando workspace…"))
                .into_element(),
            LoadState::Error(msg) => rect()
                .expanded()
                .background(bg)
                .center()
                .child(centered_text(&msg))
                .into_element(),
            LoadState::Ready(handle, snap) => {
                workspace_shell(workspace_id, handle, snap, load_state, router).into_element()
            }
        }
    }
}

fn centered_text(text: &str) -> Rect {
    rect()
        .center()
        .child(
            label()
                .color(Color::from_rgb(200, 200, 210))
                .font_size(14.0)
                .text(text.to_owned()),
        )
}

fn snapshot_workspace(lib: &Library, id: WorkspaceId) -> Option<WorkspaceSnapshot> {
    let ws = lib.workspace(id)?.clone();
    let mut titles = HashMap::new();
    let mut kinds = HashMap::new();
    for pane in ws.layout.leaves() {
        for tab in &pane.tabs {
            if titles.contains_key(&tab.item) {
                continue;
            }
            match lib.index().item(tab.item) {
                Some(item) => {
                    titles.insert(tab.item, item.name.clone());
                    kinds.insert(tab.item, item.kind);
                }
                None => {
                    titles.insert(tab.item, MISSING_TITLE.to_owned());
                }
            }
        }
    }
    Some(WorkspaceSnapshot {
        workspace: ws,
        titles,
        kinds,
    })
}

fn workspace_shell(
    workspace_id: WorkspaceId,
    handle: LibHandle,
    snap: WorkspaceSnapshot,
    load_state: State<LoadState>,
    router: RouterContext,
) -> Rect {
    let focused = snap.workspace.focused_pane;
    let chrome_state = crate::app::use_chrome_state();
    // Expose the shared chrome signals to every pane body so children
    // like `DocLinkedView` / `SourcePdfView` opt into the workspace
    // toolbar instead of mounting their own.
    provide_context(chrome_state);
    let insets = crate::hooks::use_safe_area_insets();
    let pad = *insets.read();
    let header = header_bar(workspace_id, handle.clone(), focused, load_state, router, &snap);
    let body = pane_tree(workspace_id, handle, load_state, &snap, chrome_state);
    let keyboard = keyboard_shortcuts(workspace_id, handle_clone_for_keys(&snap, &load_state));
    rect()
        .expanded()
        .background(Color::from_rgb(15, 15, 18))
        .vertical()
        .on_global_key_down(keyboard)
        .child(header)
        .child(body)
        .child(crate::app::canvas_chrome(chrome_state, pad))
}

fn handle_clone_for_keys(
    snap: &WorkspaceSnapshot,
    load_state: &State<LoadState>,
) -> KeyboardCtx {
    KeyboardCtx {
        focused: snap.workspace.focused_pane,
        tab_count: snap
            .workspace
            .focused_pane
            .and_then(|id| snap.workspace.layout.find_leaf(id))
            .map(|p| p.tabs.len())
            .unwrap_or(0),
        active_tab: snap
            .workspace
            .focused_pane
            .and_then(|id| snap.workspace.layout.find_leaf(id))
            .map(|p| p.active_tab as usize)
            .unwrap_or(0),
        load_state: *load_state,
    }
}

#[derive(Clone, Copy)]
struct KeyboardCtx {
    focused: Option<PaneId>,
    tab_count: usize,
    active_tab: usize,
    load_state: State<LoadState>,
}

fn keyboard_shortcuts(
    workspace_id: WorkspaceId,
    ctx: KeyboardCtx,
) -> impl FnMut(Event<KeyboardEventData>) + 'static {
    move |e: Event<KeyboardEventData>| {
        let modded = e.modifiers.contains(Modifiers::CONTROL)
            || e.modifiers.contains(Modifiers::META);
        if !modded {
            return;
        }
        let Some(pane) = ctx.focused else {
            return;
        };
        let mut load_state = ctx.load_state;
        match &e.key {
            Key::Character(ch) if ch == "t" || ch == "T" => {
                e.stop_propagation();
                e.prevent_default();
                spawn(async move {
                    if let Ok(handle) = open_library().await {
                        let mut lib = handle.lock().await;
                        if let Ok(item) = lib
                            .create_item(
                                crate::library::ROOT_FOLDER,
                                ItemKind::Canvas,
                                "Borrador",
                                None,
                                Vec::new(),
                                BackgroundStyle::default(),
                            )
                            .await
                            && lib
                                .add_pane_tab(workspace_id, pane, item)
                                .await
                                .is_ok()
                        {
                            if let Some(snap) = snapshot_workspace(&lib, workspace_id) {
                                load_state.set(LoadState::Ready(handle.clone(), snap));
                            }
                        }
                    }
                });
            }
            Key::Character(ch) if ch == "w" || ch == "W" => {
                if ctx.tab_count == 0 {
                    return;
                }
                e.stop_propagation();
                e.prevent_default();
                let idx = ctx.active_tab.min(ctx.tab_count.saturating_sub(1));
                spawn(async move {
                    if let Ok(handle) = open_library().await {
                        let mut lib = handle.lock().await;
                        let _ = lib.close_pane_tab(workspace_id, pane, idx).await;
                        if let Some(snap) = snapshot_workspace(&lib, workspace_id) {
                            load_state.set(LoadState::Ready(handle.clone(), snap));
                        }
                    }
                });
            }
            Key::Named(NamedKey::Tab) => {
                if ctx.tab_count <= 1 {
                    return;
                }
                e.stop_propagation();
                e.prevent_default();
                let next = (ctx.active_tab + 1) % ctx.tab_count;
                spawn(async move {
                    if let Ok(handle) = open_library().await {
                        let mut lib = handle.lock().await;
                        let _ = lib.focus_pane_tab(workspace_id, pane, next).await;
                        if let Some(snap) = snapshot_workspace(&lib, workspace_id) {
                            load_state.set(LoadState::Ready(handle.clone(), snap));
                        }
                    }
                });
            }
            _ => {}
        }
    }
}

fn header_bar(
    workspace_id: WorkspaceId,
    handle: LibHandle,
    focused: Option<PaneId>,
    load_state: State<LoadState>,
    router: RouterContext,
    snap: &WorkspaceSnapshot,
) -> Rect {
    let name = snap.workspace.name.clone();
    let bg = Color::from_rgb(18, 18, 22);
    let back_handle = handle.clone();
    let back_router = router;
    let back = text_button("← Home").on_press(move |_| {
        let _ = back_router.push(Route::Home);
        drop(back_handle.clone());
    });
    let split_h_handle = handle.clone();
    let split_h = text_button("Dividir ↔").on_press(move |_| {
        if let Some(target) = focused {
            schedule_split(
                workspace_id,
                target,
                SplitOrientation::Horizontal,
                split_h_handle.clone(),
                load_state,
            );
        }
    });
    let split_v_handle = handle.clone();
    let split_v = text_button("Dividir ↕").on_press(move |_| {
        if let Some(target) = focused {
            schedule_split(
                workspace_id,
                target,
                SplitOrientation::Vertical,
                split_v_handle.clone(),
                load_state,
            );
        }
    });
    let close_handle = handle.clone();
    let close_pane = text_button("Cerrar pane").on_press(move |_| {
        if let Some(target) = focused {
            schedule_close_pane(workspace_id, target, close_handle.clone(), load_state);
        }
    });
    let export_handle = handle.clone();
    let export_name = snap.workspace.name.clone();
    let export_btn = text_button("Exportar").on_press(move |_| {
        let handle = export_handle.clone();
        let stem = export_name.clone();
        spawn_forever(async move {
            crate::home::export_selection_via_picker(
                handle,
                crate::bundle::ExportSelection {
                    workspaces: vec![workspace_id],
                    ..Default::default()
                },
                stem,
            )
            .await;
        });
    });

    rect()
        .horizontal()
        .width(Size::fill())
        .height(Size::px(48.0))
        .padding((0.0, 12.0))
        .background(bg)
        .cross_align(Alignment::Center)
        .spacing(8.0)
        .child(back)
        .content(Content::Flex)
        .child(
            label()
                .color(Color::from_rgb(230, 230, 240))
                .font_size(14.0)
                .text(name),
        )
        .child(rect().width(Size::flex(1.0)).height(Size::px(1.0)))
        .child(split_h)
        .child(split_v)
        .child(close_pane)
        .child(export_btn)
}

fn text_button(text: &str) -> Rect {
    rect()
        .height(Size::px(32.0))
        .padding((0.0, 12.0))
        .center()
        .with_corner_radius(8.0)
        .background(Color::from_rgb(28, 28, 32))
        .child(
            label()
                .color(Color::from_rgb(220, 220, 230))
                .font_size(13.0)
                .text(text.to_owned()),
        )
}

fn schedule_split(
    workspace_id: WorkspaceId,
    target: PaneId,
    orientation: SplitOrientation,
    handle: LibHandle,
    mut load_state: State<LoadState>,
) {
    spawn(async move {
        let mut lib = handle.lock().await;
        if let Err(err) = lib
            .split_workspace_pane(workspace_id, target, orientation, SplitSide::Second)
            .await
        {
            log::error!("split_workspace_pane: {err}");
            return;
        }
        if let Some(snap) = snapshot_workspace(&lib, workspace_id) {
            load_state.set(LoadState::Ready(handle.clone(), snap));
        }
    });
}

fn schedule_close_pane(
    workspace_id: WorkspaceId,
    target: PaneId,
    handle: LibHandle,
    mut load_state: State<LoadState>,
) {
    spawn(async move {
        let mut lib = handle.lock().await;
        if let Err(err) = lib.close_workspace_pane(workspace_id, target).await {
            log::error!("close_workspace_pane: {err}");
            return;
        }
        if let Some(snap) = snapshot_workspace(&lib, workspace_id) {
            load_state.set(LoadState::Ready(handle.clone(), snap));
        }
    });
}

fn pane_tree(
    workspace_id: WorkspaceId,
    handle: LibHandle,
    load_state: State<LoadState>,
    snap: &WorkspaceSnapshot,
    chrome_state: crate::app::ChromeState,
) -> Rect {
    let layout = snap.workspace.layout.clone();
    let titles = snap.titles.clone();
    let kinds = snap.kinds.clone();
    let callbacks = PaneTreeCallbacks {
        on_tab_select: tab_select_cb(workspace_id, handle.clone(), load_state),
        on_tab_close: tab_close_cb(workspace_id, handle.clone(), load_state),
        on_tab_new: tab_new_cb(workspace_id, handle.clone(), load_state),
        on_split_resize: split_resize_cb(workspace_id, handle.clone(), load_state),
        on_tab_move: tab_move_cb(workspace_id, handle.clone(), load_state),
        on_tab_split: tab_split_cb(workspace_id, handle, load_state),
    };
    let label_cb = Callback::<PaneTab, String>::new(move |tab: PaneTab| {
        titles
            .get(&tab.item)
            .cloned()
            .unwrap_or_else(|| MISSING_TITLE.to_owned())
    });
    let body_kinds = kinds;
    let body_cb =
        Callback::<(PaneId, PaneTab), Element>::new(move |(_, tab): (PaneId, PaneTab)| {
            tab_body_element(
                tab.item,
                body_kinds.get(&tab.item).copied(),
                chrome_state,
            )
        });
    rect()
        .width(Size::fill())
        .height(Size::flex(1.0))
        .content(Content::Flex)
        .child(PaneTreeView::new(layout, callbacks, label_cb, body_cb))
}

/// Dispatch a per-tab body [`Element`] based on the item's kind. Each
/// variant mounts a fresh subcomponent so hooks fire in the right scope
/// (new per-pane `Board`, autosave worker, PDF viewer state, etc.).
/// Unknown kinds fall through to the plain canvas — matches the fallback
/// the old `tab_open_cb` picked for deleted-on-another-device items.
///
/// Canvas-flavoured tabs mount a bare [`canvas_body`](crate::app::canvas_body)
/// so the workspace-level chrome (toolbar, overlays, bookmark sidebar)
/// drives whichever pane last became active. DocLinked and SourcePdf
/// still ship their own self-contained shells pending their own
/// chrome-decoupling pass.
fn tab_body_element(
    id: ItemId,
    kind: Option<ItemKind>,
    chrome_state: crate::app::ChromeState,
) -> Element {
    use crate::app::{CanvasMount, canvas_body};
    use crate::doc_linked::{DocLinkedView, SourcePdfView};
    match kind {
        Some(ItemKind::PdfCanvas) => {
            canvas_body(CanvasMount::PdfBacked(id), chrome_state).into_element()
        }
        Some(ItemKind::SourcePdf) => SourcePdfView { id }.into_element(),
        Some(ItemKind::DocLinked) => DocLinkedView { id }.into_element(),
        Some(ItemKind::Canvas) | None => {
            canvas_body(CanvasMount::Item(id), chrome_state).into_element()
        }
    }
}

fn tab_select_cb(
    workspace_id: WorkspaceId,
    handle: LibHandle,
    mut load_state: State<LoadState>,
) -> Callback<(PaneId, usize), ()> {
    Callback::new(move |(pane, index): (PaneId, usize)| {
        let handle = handle.clone();
        spawn(async move {
            let mut lib = handle.lock().await;
            if let Err(err) = lib.focus_pane_tab(workspace_id, pane, index).await {
                log::error!("focus_pane_tab: {err}");
                return;
            }
            if let Some(snap) = snapshot_workspace(&lib, workspace_id) {
                load_state.set(LoadState::Ready(handle.clone(), snap));
            }
        });
    })
}

fn tab_close_cb(
    workspace_id: WorkspaceId,
    handle: LibHandle,
    mut load_state: State<LoadState>,
) -> Callback<(PaneId, usize), ()> {
    Callback::new(move |(pane, index): (PaneId, usize)| {
        let handle = handle.clone();
        spawn(async move {
            let mut lib = handle.lock().await;
            if let Err(err) = lib.close_pane_tab(workspace_id, pane, index).await {
                log::error!("close_pane_tab: {err}");
                return;
            }
            if let Some(snap) = snapshot_workspace(&lib, workspace_id) {
                load_state.set(LoadState::Ready(handle.clone(), snap));
            }
        });
    })
}

fn tab_new_cb(
    workspace_id: WorkspaceId,
    handle: LibHandle,
    mut load_state: State<LoadState>,
) -> Callback<PaneId, ()> {
    Callback::new(move |pane: PaneId| {
        let handle = handle.clone();
        spawn(async move {
            let mut lib = handle.lock().await;
            let item = match lib
                .create_item(
                    crate::library::ROOT_FOLDER,
                    ItemKind::Canvas,
                    "Borrador",
                    None,
                    Vec::new(),
                    BackgroundStyle::default(),
                )
                .await
            {
                Ok(id) => id,
                Err(err) => {
                    log::error!("create_item: {err}");
                    return;
                }
            };
            if let Err(err) = lib.add_pane_tab(workspace_id, pane, item).await {
                log::error!("add_pane_tab: {err}");
                return;
            }
            if let Some(snap) = snapshot_workspace(&lib, workspace_id) {
                load_state.set(LoadState::Ready(handle.clone(), snap));
            }
        });
    })
}

fn split_resize_cb(
    workspace_id: WorkspaceId,
    handle: LibHandle,
    mut load_state: State<LoadState>,
) -> Callback<(Vec<SplitSide>, f32), ()> {
    Callback::new(move |(path, ratio): (Vec<SplitSide>, f32)| {
        let handle = handle.clone();
        spawn(async move {
            let mut lib = handle.lock().await;
            if let Err(err) = lib
                .set_workspace_split_ratio(workspace_id, &path, ratio)
                .await
            {
                log::error!("set_workspace_split_ratio: {err}");
                return;
            }
            if let Some(snap) = snapshot_workspace(&lib, workspace_id) {
                load_state.set(LoadState::Ready(handle.clone(), snap));
            }
        });
    })
}

fn tab_move_cb(
    workspace_id: WorkspaceId,
    handle: LibHandle,
    mut load_state: State<LoadState>,
) -> Callback<(PaneId, usize, PaneId), ()> {
    Callback::new(move |(src, idx, dst): (PaneId, usize, PaneId)| {
        let handle = handle.clone();
        spawn(async move {
            let mut lib = handle.lock().await;
            if let Err(err) = lib
                .move_workspace_tab(workspace_id, src, idx, dst)
                .await
            {
                log::error!("move_workspace_tab: {err}");
                return;
            }
            if let Some(snap) = snapshot_workspace(&lib, workspace_id) {
                load_state.set(LoadState::Ready(handle.clone(), snap));
            }
        });
    })
}

fn tab_split_cb(
    workspace_id: WorkspaceId,
    handle: LibHandle,
    mut load_state: State<LoadState>,
) -> Callback<(PaneId, usize, PaneId, SplitEdge), ()> {
    Callback::new(
        move |(src, idx, dst, edge): (PaneId, usize, PaneId, SplitEdge)| {
            let handle = handle.clone();
            spawn(async move {
                let mut lib = handle.lock().await;
                let new_pane = match lib
                    .split_workspace_pane(workspace_id, dst, edge.orientation(), edge.new_side())
                    .await
                {
                    Ok(id) => id,
                    Err(err) => {
                        log::error!("split_workspace_pane (edge drop): {err}");
                        return;
                    }
                };
                if let Err(err) = lib
                    .move_workspace_tab(workspace_id, src, idx, new_pane)
                    .await
                {
                    log::error!("move_workspace_tab (edge drop): {err}");
                    return;
                }
                if let Some(snap) = snapshot_workspace(&lib, workspace_id) {
                    load_state.set(LoadState::Ready(handle.clone(), snap));
                }
            });
        },
    )
}
