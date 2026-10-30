//! What screens render from: a reactive snapshot of the library, kept
//! fresh by the library's change notifications, and the one way to
//! change it from the UI.

use std::sync::Arc;

use freya::prelude::*;
use notein_i18n::tr;
use notein_ui_kit::{Toast, ToastTone, Toasts};

use crate::library::{self, LibHandle, Library, LibraryData, LibraryError};

/// Loading state of the library.
#[derive(Clone, Debug, PartialEq)]
pub enum LibraryState {
    Loading,
    Ready(Arc<LibraryData>),
    Failed(String),
}

/// Reactive library snapshot, provided at the app root.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct LibraryView {
    state: State<LibraryState>,
}

impl std::fmt::Debug for LibraryView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibraryView").finish_non_exhaustive()
    }
}

impl LibraryView {
    /// The view installed by [`use_library_root`].
    #[must_use]
    pub fn get() -> Self {
        consume_root_context::<Self>()
    }

    /// The view, when a library root is mounted (not in bare previews).
    #[must_use]
    pub fn try_get() -> Option<Self> {
        try_consume_root_context::<Self>()
    }

    /// The loaded data, if ready, without subscribing the caller.
    #[must_use]
    pub fn peek_data(&self) -> Option<Arc<LibraryData>> {
        match &*self.state.peek() {
            LibraryState::Ready(data) => Some(Arc::clone(data)),
            LibraryState::Loading | LibraryState::Failed(_) => None,
        }
    }

    /// Current state; subscribes the caller.
    #[must_use]
    pub fn state(&self) -> LibraryState {
        self.state.read().clone()
    }

    /// The loaded data, if ready; subscribes the caller.
    #[must_use]
    pub fn data(&self) -> Option<Arc<LibraryData>> {
        match &*self.state.read() {
            LibraryState::Ready(data) => Some(Arc::clone(data)),
            LibraryState::Loading | LibraryState::Failed(_) => None,
        }
    }

    /// Ask [`use_library_root`] to try opening the library again after it
    /// failed.
    pub fn retry(&self) {
        library::shared::notify();
    }

    /// Open the library, showing each failure and waiting for
    /// [`Self::retry`] before trying again. A retry that fails again says
    /// so, since the screen alone would not change.
    #[allow(clippy::future_not_send)] // runs on the UI executor, never sent
    async fn open(mut self) -> LibHandle {
        let mut retried = false;
        loop {
            match library::shared().await {
                Ok(handle) => return handle,
                Err(err) => {
                    log::error!("library open: {err}");
                    if retried {
                        Toasts::get().show(
                            Toast::new("Todavía no se puede abrir la biblioteca")
                                .tone(ToastTone::Error),
                        );
                    }
                    let retry = library::changes();
                    self.state.set(LibraryState::Failed(err.to_string()));
                    if retry.recv_async().await.is_err() {
                        std::future::pending::<()>().await;
                    }
                    self.state.set(LibraryState::Loading);
                    retried = true;
                }
            }
        }
    }

    #[allow(clippy::future_not_send)] // runs on the UI executor, never sent
    async fn refresh(mut self, handle: &LibHandle) {
        let data = handle.lock().await.snapshot();
        self.state.set(LibraryState::Ready(Arc::new(data)));
    }
}

/// A view over fixed data, for screen previews.
#[cfg(all(test, not(feature = "hotreload")))]
pub(crate) fn use_fixed_library(data: Arc<LibraryData>) -> LibraryView {
    let state = use_state(move || LibraryState::Ready(data));
    use_hook(|| {
        let view = LibraryView { state };
        provide_context_for_scope_id(view, ScopeId::ROOT);
        view
    })
}

/// Open the shared library and keep a snapshot of it for every screen.
pub fn use_library_root() {
    let state = use_state(|| LibraryState::Loading);
    let view = use_hook(|| {
        let view = LibraryView { state };
        provide_context_for_scope_id(view, ScopeId::ROOT);
        view
    });
    use_hook(move || {
        spawn_in_window(async move {
            let handle = view.open().await;
            view.refresh(&handle).await;
            let changes = library::changes();
            while changes.recv_async().await.is_ok() {
                while changes.try_recv().is_ok() {}
                view.refresh(&handle).await;
            }
        });
    });
}

/// A background change to the library, named so its failure can be
/// told to the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryEdit {
    OpenDraft,
    DeleteDesk,
    DeleteForever,
    SwitchTab,
    Resize,
    ClosePane,
    CloseTab,
    CreateFolder,
    UndoImport,
    SplitPane,
    DuplicateDesk,
    FocusPane,
    PinDesk,
    Pin,
    SaveDraft,
    SaveFolder,
    SaveDesk,
    SaveChanges,
    MoveTab,
    Move,
    ReorderTabs,
    Unpin,
    ReorderPinned,
    Restore,
    EmptyTrash,
}

impl LibraryEdit {
    /// The toast shown when the change fails.
    #[must_use]
    pub fn failure(self) -> String {
        match self {
            Self::OpenDraft => tr!("state-failed-open-draft"),
            Self::DeleteDesk => tr!("state-failed-delete-desk"),
            Self::DeleteForever => tr!("state-failed-delete-forever"),
            Self::SwitchTab => tr!("state-failed-switch-tab"),
            Self::Resize => tr!("state-failed-resize"),
            Self::ClosePane => tr!("state-failed-close-pane"),
            Self::CloseTab => tr!("state-failed-close-tab"),
            Self::CreateFolder => tr!("state-failed-create-folder"),
            Self::UndoImport => tr!("state-failed-undo-import"),
            Self::SplitPane => tr!("state-failed-split-pane"),
            Self::DuplicateDesk => tr!("state-failed-duplicate-desk"),
            Self::FocusPane => tr!("state-failed-focus-pane"),
            Self::PinDesk => tr!("state-failed-pin-desk"),
            Self::Pin => tr!("state-failed-pin"),
            Self::SaveDraft => tr!("state-failed-save-draft"),
            Self::SaveFolder => tr!("state-failed-save-folder"),
            Self::SaveDesk => tr!("state-failed-save-desk"),
            Self::SaveChanges => tr!("state-failed-save-changes"),
            Self::MoveTab => tr!("state-failed-move-tab"),
            Self::Move => tr!("state-failed-move"),
            Self::ReorderTabs => tr!("state-failed-reorder-tabs"),
            Self::Unpin => tr!("state-failed-unpin"),
            Self::ReorderPinned => tr!("state-failed-reorder-pinned"),
            Self::Restore => tr!("state-failed-restore"),
            Self::EmptyTrash => tr!("state-failed-empty-trash"),
        }
    }
}

/// Run `change` on the library in the background. A failure becomes a
/// toast saying what could not be done.
pub fn edit<F>(what: LibraryEdit, change: F)
where
    F: AsyncFnOnce(&mut Library) -> Result<(), LibraryError> + 'static,
{
    spawn_in_window(async move {
        let result = match library::shared().await {
            Ok(handle) => {
                let mut lib = handle.lock().await;
                change(&mut lib).await
            }
            Err(err) => Err(err),
        };
        if let Err(err) = result {
            log::error!("{what:?}: {err}");
            Toasts::get().show(Toast::new(what.failure()).tone(ToastTone::Error));
        }
    });
}
