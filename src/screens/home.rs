//! Home: the library, the mesas and what to continue.

pub mod card;

use freya::prelude::*;
use freya::router::RouterContext;

use crate::library::{FolderId, ROOT_FOLDER};
use crate::route::Route;

/// The folder Home is browsing, shared by every surface that opens one.
#[derive(Clone, Copy, PartialEq)]
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
pub fn use_browse_root() -> Browse {
    let folder = use_state(|| ROOT_FOLDER);
    use_hook(|| {
        let browse = Browse { folder };
        provide_context_for_scope_id(browse, ScopeId::ROOT);
        browse
    })
}

/// Go Home and show `folder`.
pub fn open_folder(folder: FolderId) {
    Browse::get().enter(folder);
    let _ = RouterContext::get().replace(Route::Home);
}
