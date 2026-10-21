//! Phase 4 shell — mounts [`PaneTreeView`] against the active
//! workspace, wires its callbacks into the async [`Library`] helpers,
//! and installs the desktop keyboard shortcuts (Ctrl+T / Ctrl+W /
//! Ctrl+Tab) targeted at the focused pane.
//!
//! Loaded by [`crate::route::Route::Space`]. The route value
//! carries the workspace id as a plain `u32` so the deeplink surface
//! stays serde-free; the view promotes it to [`WorkspaceId`] on open.

use std::collections::HashMap;

use freya::prelude::*;
use istmo::plugins::EdgeInsets;

use crate::components::pane_tree_view::SplitEdge;
use crate::components::{ItemPicker, PaneTreeCallbacks, PaneTreeView, PickerChoice};
use crate::home::{LibHandle, open_library};
use crate::hooks::{provide_safe_area_override, use_safe_area_insets};
use crate::library::{
    DocKind, ItemId, Library, PaneId, PaneTab, SplitSide, TabTarget, ViewSlot, Workspace,
    WorkspaceId,
};
use crate::space::{self, OpenHint};

/// Fallback title for a tab whose underlying library item is missing
/// (e.g. the item was deleted on another device and the tab snapshot
/// did not yet catch up).
const MISSING_TITLE: &str = "(elemento no disponible)";
/// Tab title for a scratch canvas.
const SCRATCH_TITLE: &str = "Borrador";

#[derive(Clone, PartialEq, Debug)]
struct WorkspaceSnapshot {
    workspace: Workspace,
    titles: HashMap<ItemId, String>,
    kinds: HashMap<ItemId, DocKind>,
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
        let edge = use_safe_area_insets();
        let edge = *edge.read();
        // The shell carves out the physical safe area at its outer
        // padding; nested components (pane canvases, DocLinked shells,
        // sidebar modals) must not re-apply the same pad or the usable
        // area collapses — the chrome overlays would then anchor off
        // the visible viewport and the pane canvas would overflow past
        // the window on bottom/right.
        provide_safe_area_override(EdgeInsets::default());

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
        // Opens made elsewhere (Home, chips, the sidebar) mutate the
        // library behind this view's back; refresh on every tick.
        use_hook({
            let mut load_state = load_state;
            move || {
                let changes = space::changes();
                spawn(async move {
                    while changes.recv_async().await.is_ok() {
                        let Ok(handle) = open_library().await else {
                            continue;
                        };
                        let snap = {
                            let lib = handle.lock().await;
                            snapshot_workspace(&lib, workspace_id)
                        };
                        if let Some(snap) = snap {
                            load_state.set(LoadState::Ready(handle, snap));
                        }
                    }
                });
            }
        });

        let bg = Color::from_rgb(15, 15, 18);
        let state_val = load_state.read().clone();

        match state_val {
            LoadState::Loading => padded_safe_area(edge)
                .background(bg)
                .center()
                .child(centered_text("Cargando workspace…"))
                .into_element(),
            LoadState::Error(msg) => padded_safe_area(edge)
                .background(bg)
                .center()
                .child(centered_text(&msg))
                .into_element(),
            LoadState::Ready(handle, snap) => {
                workspace_shell(workspace_id, handle, snap, load_state, edge).into_element()
            }
        }
    }
}

fn centered_text(text: &str) -> Rect {
    rect().center().child(
        label()
            .color(Color::from_rgb(200, 200, 210))
            .font_size(14.0)
            .text(text.to_owned()),
    )
}

/// Full-window rect whose inner content area is inset by the platform
/// safe-area edges. Uses `padding` rather than absolute positioning so
/// the outer box stays sized to its parent — absolute `top/right/
/// bottom/left` only shift the origin, so combining them with
/// `.expanded()` would push the right/bottom edges past the window.
fn padded_safe_area(edge: EdgeInsets) -> Rect {
    rect().width(Size::fill()).height(Size::fill()).padding((
        edge.top,
        edge.right,
        edge.bottom,
        edge.left,
    ))
}

fn snapshot_workspace(lib: &Library, id: WorkspaceId) -> Option<WorkspaceSnapshot> {
    let ws = lib.workspace(id)?.clone();
    let mut titles = HashMap::new();
    let mut kinds = HashMap::new();
    for pane in ws.layout.leaves() {
        for id in pane.tabs.iter().filter_map(PaneTab::item) {
            if titles.contains_key(&id) {
                continue;
            }
            match lib.index().item(id) {
                Some(item) => {
                    titles.insert(id, item.name.clone());
                    kinds.insert(id, item.kind.clone());
                }
                None => {
                    titles.insert(id, MISSING_TITLE.to_owned());
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
    edge: EdgeInsets,
) -> Rect {
    let body = pane_tree(workspace_id, handle, load_state, &snap);
    let keyboard = keyboard_shortcuts(workspace_id, handle_clone_for_keys(&snap, &load_state));

    padded_safe_area(edge)
        .background(Color::from_rgb(15, 15, 18))
        .vertical()
        .content(Content::Flex)
        .on_global_key_down(keyboard)
        .child(body)
}

fn handle_clone_for_keys(snap: &WorkspaceSnapshot, load_state: &State<LoadState>) -> KeyboardCtx {
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
        let modded =
            e.modifiers.contains(Modifiers::CONTROL) || e.modifiers.contains(Modifiers::META);
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
                open_tab_picker(workspace_id, pane, load_state);
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

fn pane_close_cb(
    workspace_id: WorkspaceId,
    handle: LibHandle,
    load_state: State<LoadState>,
) -> Callback<PaneId, ()> {
    Callback::new(move |target: PaneId| {
        schedule_close_pane(workspace_id, target, handle.clone(), load_state);
    })
}

fn pane_tree(
    workspace_id: WorkspaceId,
    handle: LibHandle,
    load_state: State<LoadState>,
    snap: &WorkspaceSnapshot,
) -> Rect {
    let layout = snap.workspace.layout.clone();
    // One pane with exactly one tab reads as a plain document: no
    // tab strip until a second tab or pane appears. An empty pane keeps
    // the strip so its `+` stays reachable.
    let show_tabs = {
        let leaves = layout.leaves();
        !(leaves.len() == 1 && leaves[0].tabs.len() == 1)
    };
    let titles = snap.titles.clone();
    let kinds = snap.kinds.clone();
    let callbacks = PaneTreeCallbacks {
        on_tab_select: tab_select_cb(workspace_id, handle.clone(), load_state),
        on_tab_close: tab_close_cb(workspace_id, handle.clone(), load_state),
        on_tab_new: tab_new_cb(workspace_id, load_state),
        on_split_resize: split_resize_cb(workspace_id, handle.clone(), load_state),
        on_tab_move: tab_move_cb(workspace_id, handle.clone(), load_state),
        on_tab_split: tab_split_cb(workspace_id, handle.clone(), load_state),
        on_pane_close: pane_close_cb(workspace_id, handle, load_state),
    };
    let label_cb = Callback::<PaneTab, String>::new(move |tab: PaneTab| match tab.target {
        TabTarget::Item(id) => titles
            .get(&id)
            .cloned()
            .unwrap_or_else(|| MISSING_TITLE.to_owned()),
        TabTarget::Scratch(_) => SCRATCH_TITLE.to_owned(),
    });
    let body_kinds = kinds;
    let body_cb =
        Callback::<(PaneId, PaneTab), Element>::new(move |(pane, tab): (PaneId, PaneTab)| {
            let slot = ViewSlot {
                workspace: workspace_id,
                pane,
            };
            tab_body_element(&tab, &body_kinds, slot)
        });
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .content(Content::Flex)
        .child(PaneTreeView::new(layout, callbacks, label_cb, body_cb).show_tabs(show_tabs))
}

/// Dispatch a per-tab body [`Element`] based on the item's kind. Each
/// variant mounts a fresh subcomponent so hooks fire in the right scope
/// (new per-pane `Board`, autosave worker, PDF viewer state, etc.).
/// Unknown kinds fall through to the plain canvas — matches the fallback
/// the old `tab_open_cb` picked for deleted-on-another-device items.
///
/// Canvas-flavoured tabs mount a self-contained [`CanvasPane`] that
/// owns its own chrome (toolbar, overlays, bookmark sidebar). Each
/// pane's chrome is scoped to its own body area — tab bars stay
/// untouched. DocLinked and SourcePdf ship their own self-contained
/// shells with the same per-pane scope.
fn tab_body_element(tab: &PaneTab, kinds: &HashMap<ItemId, DocKind>, slot: ViewSlot) -> Element {
    use crate::app::{CanvasMount, CanvasPane};
    let body = match tab.target {
        TabTarget::Item(id) => match kinds.get(&id) {
            Some(kind) => crate::faces::render(id, kind, Some(slot)),
            None => CanvasPane {
                mount: CanvasMount::Item {
                    id,
                    slot: Some(slot),
                },
            }
            .into_element(),
        },
        TabTarget::Scratch(id) => CanvasPane {
            mount: CanvasMount::Ephemeral(id),
        }
        .into_element(),
    };
    // Keyed by target so switching tabs in a pane mounts a fresh body:
    // components diff by position, and a reused `CanvasPane` would keep
    // the previous document's board and workers alive under the new tab.
    rect()
        .key(tab.target)
        .width(Size::fill())
        .height(Size::fill())
        .content(Content::Flex)
        .child(body)
        .into_element()
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

fn tab_new_cb(workspace_id: WorkspaceId, load_state: State<LoadState>) -> Callback<PaneId, ()> {
    Callback::new(move |pane: PaneId| open_tab_picker(workspace_id, pane, load_state))
}

/// Ask what the new tab in `pane` should show, then open it there.
fn open_tab_picker(workspace_id: WorkspaceId, pane: PaneId, mut load_state: State<LoadState>) {
    ItemPicker::new(move |choice: PickerChoice| {
        spawn_forever(async move {
            let Ok(handle) = open_library().await else {
                return;
            };
            let mut lib = handle.lock().await;
            let Some(target) = choice.resolve(&mut lib).await else {
                return;
            };
            if let Err(err) = space::open_in(&mut lib, target, OpenHint::InPane(pane)).await {
                log::error!("open in pane: {err}");
                return;
            }
            if let Some(snap) = snapshot_workspace(&lib, workspace_id) {
                load_state.set(LoadState::Ready(handle.clone(), snap));
            }
        });
    })
    .title("Nueva pestaña")
    .open();
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

// Drop handlers run in the scope of the drop zone that caught the
// release, and the zone clears the drag right after — the edge overlay
// unmounts with it, taking any task spawned there. The library work
// below is therefore spawned at the root so it outlives the zone, and
// the view refresh tolerates the shell having gone away meanwhile.

fn tab_move_cb(
    workspace_id: WorkspaceId,
    handle: LibHandle,
    load_state: State<LoadState>,
) -> Callback<(PaneId, usize, PaneId), ()> {
    Callback::new(move |(src, idx, dst): (PaneId, usize, PaneId)| {
        let handle = handle.clone();
        spawn_forever(async move {
            let mut lib = handle.lock().await;
            if let Err(err) = lib.move_workspace_tab(workspace_id, src, idx, dst).await {
                log::error!("move_workspace_tab: {err}");
                return;
            }
            publish_snapshot(&lib, workspace_id, &handle, load_state);
        });
    })
}

fn tab_split_cb(
    workspace_id: WorkspaceId,
    handle: LibHandle,
    load_state: State<LoadState>,
) -> Callback<(PaneId, usize, PaneId, SplitEdge), ()> {
    Callback::new(
        move |(src, idx, dst, edge): (PaneId, usize, PaneId, SplitEdge)| {
            let handle = handle.clone();
            spawn_forever(async move {
                let mut lib = handle.lock().await;
                if let Err(err) = lib
                    .move_workspace_tab_to_split(
                        workspace_id,
                        src,
                        idx,
                        dst,
                        edge.orientation(),
                        edge.new_side(),
                    )
                    .await
                {
                    log::error!("move_workspace_tab_to_split: {err}");
                    return;
                }
                publish_snapshot(&lib, workspace_id, &handle, load_state);
            });
        },
    )
}

/// Push a fresh workspace snapshot into the shell, if it is still
/// mounted.
fn publish_snapshot(
    lib: &Library,
    workspace_id: WorkspaceId,
    handle: &LibHandle,
    mut load_state: State<LoadState>,
) {
    let Some(snap) = snapshot_workspace(lib, workspace_id) else {
        return;
    };
    if let Some(mut state) = load_state.try_write() {
        *state = LoadState::Ready(handle.clone(), snap);
    }
}
