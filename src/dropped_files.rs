//! Files dragged onto the window from the system.
//!
//! The window takes the drop as a whole: the system says neither where
//! over the window the file is while it is dragged nor where it lands,
//! so the destination comes from where the person is. In the library the
//! files are imported into the folder on screen, through the import
//! queue; a folder comes in as a folder tree.
//!
//! While something is dragged over the window an overlay says what a
//! drop will do.

use std::path::PathBuf;

use freya::prelude::*;
use notin_i18n::tr;
use notin_ui_kit::{Toast, ToastTone, Toasts, palette};

use crate::flows::create::Context;
use crate::flows::import_queue::{self, Pending, Source};
use crate::route::Route;

/// What a drop does where the person is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target {
    /// Imported into the library folder on screen.
    Library,
    /// Nowhere (the trash).
    None,
}

impl Target {
    fn here() -> Self {
        match crate::app::global_router().current::<Route>() {
            Route::Home | Route::Space { .. } => Self::Library,
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
