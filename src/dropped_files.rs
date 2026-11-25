//! Files dragged onto the window from the system.
//!
//! The window takes the drop as a whole: the system says neither where
//! over the window the file is while it is dragged nor where it lands,
//! so the destination comes from where the person is:
//!
//! - in a mesa whose focused document takes them (a note, a canvas),
//!   the files go into that document ([`crate::flows`]
//!   asks how);
//! - elsewhere they are imported into the library folder on screen, a
//!   folder as a folder tree.
//!
//! Either way they pass through the import queue, one sheet at a time.
//!
//! While something is dragged over the window an overlay says what a
//! drop will do.

use std::path::PathBuf;

use freya::prelude::*;
use notin_i18n::tr;
use notin_ui_kit::{Toast, ToastTone, Toasts, palette};

use crate::flows::create::Context;
use crate::flows::import_queue::{self, Pending, Source};
use crate::library::{ItemId, WorkspaceId};
use crate::route::Route;

/// The focused document of mesa `id`, when it is on screen and takes
/// dropped files (a note, a canvas).
fn focused_body(id: WorkspaceId) -> Option<ItemId> {
    let data = crate::state::LibraryView::try_get()?.data()?;
    let item = crate::screens::mesa::Focus::of(data.workspace(id)?)?.item()?;
    crate::flows::add_to_document::Destination::of(item).map(|_| item)
}

/// What a drop does where the person is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// Imported into the library folder on screen.
    Library,
    /// Into the focused document, through its editor.
    Document(ItemId),
    /// Nowhere (the trash).
    None,
}

impl Target {
    fn here() -> Self {
        match crate::app::global_router().current::<Route>() {
            Route::Home => Self::Library,
            Route::Space { id } => focused_body(id).map_or(Self::Library, Self::Document),
            Route::Trash => Self::None,
        }
    }
}

/// Bring in what was dropped.
#[sismo::function("dropped_files::dropped")]
pub(crate) fn dropped(paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    match Target::here() {
        Target::None => {
            Toasts::get().show(Toast::new(tr!("flows-import-drop-nowhere")).tone(ToastTone::Error));
        }
        Target::Library => {
            let context = Context {
                folder: crate::screens::home::Browse::get().folder_now(),
                ..Context::default()
            };
            let items = paths
                .into_iter()
                .map(|path| {
                    // Not followed: a dropped link to a folder imports
                    // nothing it points to.
                    let folder = std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir());
                    if folder {
                        Pending::Tree(path)
                    } else {
                        Pending::Path(path)
                    }
                })
                .collect();
            import_queue::add(items, &context, Source::Dropped, Vec::new());
        }
        Target::Document(document) => {
            let mut refused = Vec::new();
            let items = paths
                .into_iter()
                .filter_map(|path| {
                    if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()) {
                        refused.push(
                            path.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                        );
                        None
                    } else {
                        Some(Pending::IntoDocument { path, document })
                    }
                })
                .collect();
            import_queue::add(items, &Context::default(), Source::Dropped, refused);
        }
    }
}

/// Covers the window while files are dragged over it.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct DropOverlay {
    pub(crate) dragging: State<bool>,
}

impl Component for DropOverlay {
    #[sismo::function("dropped_files::overlay")]
    fn render(&self) -> impl IntoElement {
        if !*self.dragging.read() {
            return rect().into_element();
        }
        let p = palette();
        let text = match Target::here() {
            Target::Library => tr!("flows-import-drop-here"),
            Target::Document(document) => tr!(
                "flows-add-drop",
                "document" => crate::state::LibraryView::try_get()
                    .and_then(|view| view.data())
                    .and_then(|data| data.item(document).map(|item| item.name.clone()))
                    .unwrap_or_default()
            ),
            Target::None => tr!("flows-import-drop-nowhere"),
        };
        // Percentages, not `fill`: an absolute child gets what the stacked
        // siblings left of the parent.
        rect()
            .position(Position::new_absolute().top(0.0).left(0.0))
            .width(Size::percent(100.))
            .height(Size::percent(100.))
            .layer(Layer::OverlayLevel(notin_ui_kit::tokens::layer::MODAL_MAX))
            .background(p.bg.with_a(200))
            .padding(24.0)
            .center()
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::fill())
                    .corner_radius(16.0)
                    .border(Border::new().fill(p.accent).width(2.0))
                    .center()
                    .spacing(12.0)
                    .child(
                        SvgViewer::new(freya::icons::lucide::download())
                            .color(p.accent)
                            .width(Size::px(40.0))
                            .height(Size::px(40.0)),
                    )
                    .child(label().font_size(16.0).color(p.t1).text(text)),
            )
            .into_element()
    }
}

/// Drops `istmo-drop` receives itself: on Wayland, where winit delivers
/// none, from the compositor. They take the same way as Freya's
/// `on_file_drop`, and `dragging` shows the same overlay while files
/// hover. Elsewhere on desktop nothing arrives here.
#[cfg(not(any(
    target_os = "android",
    target_os = "ios",
    target_os = "tvos",
    target_os = "watchos",
    target_os = "visionos",
)))]
#[sismo::function("dropped_files::listen_system")]
pub(crate) fn listen_system(mut dragging: State<bool>) {
    use istmo_dnd::{DragAndDropClient, DragState, DropInbox, DropOptions};

    spawn_in_window(async move {
        let listening = match DragAndDropClient::acquire() {
            Ok(client) => client.listen(DropOptions::any().with_folders()).await,
            Err(err) => Err(err),
        };
        if let Err(err) = listening {
            tracing::warn!("system drops unavailable: {err}");
            return;
        }
        let inbox = match DropInbox::acquire() {
            Ok(inbox) => inbox,
            Err(err) => {
                tracing::warn!("system drops unavailable: {err}");
                return;
            }
        };
        let changes = inbox.drag_changes();
        spawn_in_window(async move {
            while let Ok(state) = changes.recv_async().await {
                dragging.set(matches!(state, DragState::Over(_)));
            }
        });
        let drops = inbox.stream();
        while let Ok(drop) = drops.recv_async().await {
            for skipped in &drop.skipped {
                tracing::debug!(name = %skipped.display_name, reason = ?skipped.reason, "drop skipped");
            }
            dropped(
                drop.files
                    .into_iter()
                    .map(|file| PathBuf::from(file.path))
                    .collect(),
            );
        }
    });
}

/// Whether the app is dragging one of its documents out (or just did):
/// a drop arriving now is that drag let go on the app's own window.
pub(crate) fn dragging_out() -> bool {
    #[cfg(not(any(
        target_os = "android",
        target_os = "ios",
        target_os = "tvos",
        target_os = "watchos",
        target_os = "visionos",
    )))]
    {
        crate::desktop::DROP_PUBLISHER
            .get()
            .is_some_and(|publisher| publisher.dragging_out())
    }
    #[cfg(any(
        target_os = "android",
        target_os = "ios",
        target_os = "tvos",
        target_os = "watchos",
        target_os = "visionos",
    ))]
    {
        false
    }
}
