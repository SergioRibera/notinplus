//! Opening things on the active mesa.
//!
//! Every path that shows a document — a card on Home, a chip, the
//! sidebar, the tab picker, a notebook button — comes through here so
//! the rules of where a tab lands live in one place:
//!
//! * [`OpenHint::Focused`]: a new tab on the focused pane, or focus
//!   the tab that already shows the target.
//! * [`OpenHint::Beside`]: never replace what the user is reading —
//!   a tab on another pane, splitting the focused one when the mesa
//!   has a single pane.
//! * [`OpenHint::InPane`]: a tab on a specific pane (the `+` button).
//! * [`OpenHint::NewSpace`]: a fresh mesa holding only the target.
//!
//! A target already open anywhere on the mesa is focused instead of
//! opened twice.
//!
//! Views that render a mesa subscribe to [`changes`] so an open made
//! from outside their own callbacks (Home, a chip, a plugin) refreshes
//! them.

use std::sync::{Mutex, OnceLock};

use crate::library::shared as open_library;
use crate::library::{
    ItemId, Library, LibraryError, NotFoundKind, PaneId, PaneLayout, Result, SplitOrientation,
    SplitSide, TabTarget, WorkspaceId,
};
use crate::route::Route;

/// Where an opened tab should land.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpenHint {
    Focused,
    Beside,
    InPane(PaneId),
    NewSpace,
}

/// Where a target ended up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Opened {
    pub workspace: WorkspaceId,
    pub pane: PaneId,
}

/// Open `target` on the active mesa and navigate to it. Fire and
/// forget: failures are logged.
pub fn open(target: TabTarget, hint: OpenHint) {
    open_with(target, hint, true);
}

fn open_with(target: TabTarget, hint: OpenHint, dedupe: bool) {
    freya::prelude::spawn_forever(async move {
        let Ok(handle) = open_library().await else {
            log::warn!("space::open: library unavailable");
            return;
        };
        let opened = {
            let mut lib = handle.lock().await;
            open_in_with(&mut lib, target, hint, dedupe).await
        };
        match opened {
            Ok(opened) => {
                notify_changed();
                let router = crate::app::global_router();
                if router.current::<Route>()
                    != (Route::Space {
                        id: opened.workspace,
                    })
                {
                    let _ = router.push(Route::Space {
                        id: opened.workspace,
                    });
                }
            }
            Err(err) => log::error!("space::open: {err}"),
        }
    });
}

/// Make `workspace` the active mesa and navigate to it.
pub fn open_space(workspace: WorkspaceId) {
    freya::prelude::spawn_forever(async move {
        let Ok(handle) = open_library().await else {
            return;
        };
        let switched = {
            let mut lib = handle.lock().await;
            lib.set_active_workspace(Some(workspace)).await
        };
        match switched {
            Ok(()) => {
                notify_changed();
                let _ = crate::app::global_router().push(Route::Space { id: workspace });
            }
            Err(err) => log::error!("space::open_space: {err}"),
        }
    });
}

/// Apply the opening rules on `lib`. Callers that already hold the
/// library lock (view callbacks) use this directly and refresh
/// themselves; everything else goes through [`open`].
///
/// # Errors
/// Library persistence failures, or a hint naming a pane that is gone.
pub async fn open_in(lib: &mut Library, target: TabTarget, hint: OpenHint) -> Result<Opened> {
    open_in_with(lib, target, hint, true).await
}

/// Open a second view of `item` beside the current one even though it
/// is already on the mesa. Both views share the document; each keeps
/// its own viewport.
pub fn open_copy(item: ItemId) {
    open_with(TabTarget::Item(item), OpenHint::Beside, false);
}

/// Open `target` on a mesa of its own.
async fn open_in_new_space(lib: &mut Library, target: TabTarget) -> Result<Opened> {
    let workspace = lib.create_workspace("", None).await?;
    lib.set_active_workspace(Some(workspace)).await?;
    let pane = root_pane(lib, workspace)?;
    lib.add_tab(workspace, pane, target).await?;
    Ok(Opened { workspace, pane })
}

async fn open_in_with(
    lib: &mut Library,
    target: TabTarget,
    hint: OpenHint,
    dedupe: bool,
) -> Result<Opened> {
    if hint == OpenHint::NewSpace {
        return open_in_new_space(lib, target).await;
    }

    let workspace = if let Some(id) = lib.active_workspace() {
        id
    } else {
        let id = lib.create_workspace("", None).await?;
        lib.set_active_workspace(Some(id)).await?;
        id
    };
    let ws = lib
        .workspace(workspace)
        .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;

    // Already shown somewhere on this mesa: bring it forward.
    if dedupe && let Some((pane, index)) = ws.find_tab(target) {
        lib.focus_workspace_pane(workspace, Some(pane)).await?;
        lib.focus_pane_tab(workspace, pane, index).await?;
        return Ok(Opened { workspace, pane });
    }

    let focused = ws
        .focused_pane
        .filter(|id| ws.layout.find_leaf(*id).is_some())
        .or_else(|| ws.layout.leaves().first().map(|p| p.id))
        .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
    let leaves: Vec<PaneId> = ws.layout.leaves().iter().map(|p| p.id).collect();

    let pane = match hint {
        OpenHint::Focused => focused,
        OpenHint::InPane(pane) => {
            if ws.layout.find_leaf(pane).is_none() {
                return Err(LibraryError::NotFound(NotFoundKind::Workspace(workspace)));
            }
            pane
        }
        OpenHint::Beside => match leaves.iter().find(|id| **id != focused) {
            Some(other) => *other,
            None => {
                lib.split_workspace_pane(
                    workspace,
                    focused,
                    SplitOrientation::Horizontal,
                    SplitSide::Second,
                )
                .await?
            }
        },
        OpenHint::NewSpace => return open_in_new_space(lib, target).await,
    };

    lib.add_tab(workspace, pane, target).await?;
    lib.focus_workspace_pane(workspace, Some(pane)).await?;
    Ok(Opened { workspace, pane })
}

fn root_pane(lib: &Library, workspace: WorkspaceId) -> Result<PaneId> {
    let ws = lib
        .workspace(workspace)
        .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
    match &ws.layout {
        PaneLayout::Leaf(pane) => Ok(pane.id),
        PaneLayout::Split { .. } => ws
            .layout
            .leaves()
            .first()
            .map(|p| p.id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace))),
    }
}

type Listeners = Mutex<Vec<flume::Sender<()>>>;

fn listeners() -> &'static Listeners {
    static L: OnceLock<Listeners> = OnceLock::new();
    L.get_or_init(|| Mutex::new(Vec::new()))
}

/// Receiver that gets a tick whenever a mesa changed outside the view
/// that renders it. Drop it to unsubscribe.
#[must_use]
pub fn changes() -> flume::Receiver<()> {
    let (tx, rx) = flume::unbounded();
    if let Ok(mut list) = listeners().lock() {
        list.push(tx);
    }
    rx
}

/// Wake every live [`changes`] subscriber.
pub fn notify_changed() {
    if let Ok(mut list) = listeners().lock() {
        list.retain(|tx| tx.send(()).is_ok());
    }
}

/// Close the tab showing `target` on the active mesa, if any.
pub fn close_tab(target: TabTarget) {
    freya::prelude::spawn_forever(async move {
        let Ok(handle) = open_library().await else {
            return;
        };
        let mut lib = handle.lock().await;
        let Some(workspace) = lib.active_workspace() else {
            return;
        };
        let found = lib.workspace(workspace).and_then(|w| w.find_tab(target));
        if let Some((pane, index)) = found
            && let Err(err) = lib.close_pane_tab(workspace, pane, index).await
        {
            log::error!("close_tab: {err}");
        }
    });
}
