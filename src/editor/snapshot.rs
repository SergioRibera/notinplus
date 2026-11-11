//! "Insert as image": a place in another document, pictured as it looks
//! now and kept in this one.
//!
//! The pictures come from the source's kind ([`notin_plugin::kind::DocumentKind::snapshot`])
//! and are stored like any image added to the item, so they travel with
//! it. They are copies: the source may change, the pictures stay.

use std::sync::{Arc, Mutex};

use freya::prelude::*;
use freya_markdown_editor::prelude::EditorController;
use notin_i18n::tr;
use notin_plugin::DocId;
use notin_plugin::bookmark::SourceRef;
use notin_plugin::kind::Snapshot;
use notin_ui_kit::{Toast, ToastTone, Toasts};

use super::images::{ItemImages, report_add_failure};
use crate::canvas::Board;
use crate::library::{ItemId, bodies};
use crate::plugins::content;
use crate::references;
use crate::state::LibraryView;

/// Where an editor keeps pictures of other documents.
#[derive(Clone)]
pub enum SnapshotSink {
    /// Nowhere: the editor belongs to no item (an annotation, a sheet) and
    /// does not offer "Insert as image".
    Nowhere,
    /// The body of `item`: pictures go in its `assets/` folder and are
    /// linked where the caret is.
    Note(ItemId),
    /// A text box of `board`, the canvas of `item`: pictures land on the
    /// canvas beside the box.
    Board {
        item: ItemId,
        board: Arc<Mutex<Board>>,
    },
}

impl std::fmt::Debug for SnapshotSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nowhere => f.write_str("Nowhere"),
            Self::Note(item) => f.debug_tuple("Note").field(item).finish(),
            Self::Board { item, .. } => f
                .debug_struct("Board")
                .field("item", item)
                .finish_non_exhaustive(),
        }
    }
}

impl SnapshotSink {
    /// Whether "Insert as image" is offered.
    #[must_use]
    pub const fn takes_pictures(&self) -> bool {
        !matches!(self, Self::Nowhere)
    }

    const fn item(&self) -> Option<ItemId> {
        match self {
            Self::Nowhere => None,
            Self::Note(item) | Self::Board { item, .. } => Some(*item),
        }
    }
}

/// Picture `target` and put the pictures where `sink` says. Rendering and
/// storing happen off the UI thread; the editor behind `controller` gets
/// the result when it is ready.
#[sismo::function("editor::insert_snapshot")]
pub(super) fn insert(target: SourceRef, sink: &SnapshotSink, controller: EditorController) {
    let SourceRef::Plugin {
        plugin_id,
        item: source_item,
        anchor,
        ..
    } = target.clone()
    else {
        return;
    };
    let Some(destination) = sink.item() else {
        return;
    };
    let sink = sink.clone();
    spawn_in_window(traced!("task::editor::snapshot", async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let source = ItemId(source_item);
        let Some(found) = handle.lock().await.item(source).cloned() else {
            Toasts::get().show(Toast::new(tr!("editor-snapshot-gone")).tone(ToastTone::Error));
            return;
        };
        let Some(kind) = crate::plugins::kind(&plugin_id).filter(|kind| kind.snapshots()) else {
            return;
        };
        kind.cited(DocId(source_item), &anchor);
        let bytes = content::read(source, &plugin_id).unwrap_or_default();
        let attachment = kind
            .info()
            .attachment
            .and_then(|_| bodies::read_pdf(source).ok());
        let pictures = off_thread(move || {
            kind.snapshot(
                content::meta(&found),
                &bytes,
                attachment.as_deref(),
                &anchor,
            )
        })
        .await;
        if pictures.is_empty() {
            Toasts::get().show(Toast::new(tr!("editor-snapshot-empty")));
            return;
        }
        sismo::counter!("editor.snapshot_pictures", pictures.len());
        let images = ItemImages::new(destination);
        let mut stored = Vec::with_capacity(pictures.len());
        for Snapshot { png, source, .. } in pictures {
            match images.store(png).await {
                Ok(image) => stored.push((image, source)),
                Err(err) => {
                    report_add_failure(&err);
                    return;
                }
            }
        }
        place(&sink, &target, stored, controller);
    }));
}

/// Put the stored pictures in the document.
#[sismo::function("editor::place_snapshot")]
fn place(
    sink: &SnapshotSink,
    target: &SourceRef,
    stored: Vec<(super::images::StoredImage, String)>,
    controller: EditorController,
) {
    match sink {
        SnapshotSink::Nowhere => {}
        // Rendering takes a while; the note may have closed meanwhile.
        SnapshotSink::Note(_) if !controller.is_mounted() => {}
        SnapshotSink::Note(_) => {
            let data = LibraryView::get().data();
            let label = references::display(target, data.as_deref()).label;
            let mut blocks: Vec<String> = stored
                .iter()
                .map(|(image, source)| {
                    format!("![{}]({})", source.replace(['[', ']'], ""), image.url)
                })
                .collect();
            blocks.push(format!("— {}", references::chip_markdown(&label, target)));
            controller.insert_markdown(&blocks.join("\n\n"));
        }
        SnapshotSink::Board { board, .. } => {
            let assets: Vec<(String, (u32, u32))> = stored
                .into_iter()
                .map(|(image, _)| (image.name, image.size))
                .collect();
            let added = crate::canvas::lock(board)
                .add_snapshots_at_view_center(&assets, Some(&target.to_url()));
            if added {
                Toasts::get()
                    .show(Toast::new(tr!("editor-snapshot-on-canvas")).tone(ToastTone::Done));
            }
        }
    }
}

/// Run `work` on its own thread and wait for it without blocking the UI.
#[allow(clippy::future_not_send, reason = "runs on the UI executor")]
async fn off_thread<T: Send + Default + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, result) = flume::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("notein-snapshot".to_owned())
        .spawn(move || {
            sismo::scope!("editor::snapshot_worker");
            let _ = done.send(work());
        });
    if spawned.is_err() {
        return T::default();
    }
    result.recv_async().await.unwrap_or_default()
}
