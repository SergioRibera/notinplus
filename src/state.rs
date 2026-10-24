//! What screens render from: a reactive snapshot of the library, kept
//! fresh by the library's change notifications, and the one way to
//! change it from the UI.

use std::sync::Arc;

use freya::prelude::*;
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

    #[allow(clippy::future_not_send)] // runs on the UI executor, never sent
    async fn refresh(mut self, handle: &LibHandle) {
        let data = handle.lock().await.snapshot();
        self.state.set(LibraryState::Ready(Arc::new(data)));
    }
}

/// A view over fixed data, for screen previews.
#[cfg(test)]
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
        spawn_forever(async move {
            let handle = match library::shared().await {
                Ok(handle) => handle,
                Err(err) => {
                    log::error!("library open: {err}");
                    let mut state = state;
                    state.set(LibraryState::Failed(err.to_string()));
                    return;
                }
            };
            view.refresh(&handle).await;
            let changes = library::changes();
            while changes.recv_async().await.is_ok() {
                while changes.try_recv().is_ok() {}
                view.refresh(&handle).await;
            }
        });
    });
}

/// Run `change` on the library in the background. A failure becomes a
/// toast saying what could not be done ("No se pudo renombrar").
pub fn edit<F>(what: &'static str, change: F)
where
    F: AsyncFnOnce(&mut Library) -> Result<(), LibraryError> + 'static,
{
    spawn_forever(async move {
        let result = match library::shared().await {
            Ok(handle) => {
                let mut lib = handle.lock().await;
                change(&mut lib).await
            }
            Err(err) => Err(err),
        };
        if let Err(err) = result {
            log::error!("{what}: {err}");
            Toasts::get().show(Toast::new(format!("No se pudo {what}")).tone(ToastTone::Error));
        }
    });
}
