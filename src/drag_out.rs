//! Documents dragged out of the app to other apps.
//!
//! A card pressed with a mouse (a trackpad, a pen hovering) and moved
//! leaves as the file its kind exports by default: a note as markdown, a
//! canvas as PDF, an imported PDF as itself. The file is written under
//! `istmo-dnd`'s drag folder with the document's name, and the system
//! takes the drag from there.

use istmo_dnd::{DragAndDropClient, DragFile, DragItem, DragRequest, drag_out_dir};

use crate::library::{ItemId, bodies};
use crate::plugins::content;

/// Drag `item` out as its exported file, from the press in progress.
#[sismo::function("drag_out::document")]
pub(crate) fn document(item: ItemId) {
    freya::prelude::spawn_in_window(traced!("task::drag_out::document", async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let Some(found) = handle.lock().await.item(item).cloned() else {
            return;
        };
        let Some(kind) = crate::plugins::kind(&found.kind.plugin_id) else {
            return;
        };
        if !kind.exports() {
            return;
        }
        // Exporting and the drag itself block: off the UI thread.
        let spawned = std::thread::Builder::new()
            .name("drag-out".into())
            .spawn(move || {
                sismo::scope!("drag_out::run");
                let content = content::read(item, &found.kind.plugin_id).unwrap_or_default();
                let attachment = kind
                    .info()
                    .attachment
                    .and_then(|_| bodies::read_pdf(item).ok());
                let Some(file) =
                    kind.export(content::meta(&found), &content, attachment.as_deref())
                else {
                    return;
                };
                let path = match drag_out_dir().and_then(|dir| {
                    let path = dir.join(format!("{}.{}", file_name(&found.name), file.extension));
                    std::fs::write(&path, &file.bytes)?;
                    Ok(path)
                }) {
                    Ok(path) => path,
                    Err(err) => {
                        tracing::warn!("drag out: write: {err}");
                        return;
                    }
                };
                let request = DragRequest::new([DragItem::File(DragFile {
                    path: path.display().to_string(),
                    mime_type: Some(file.mime.into_owned()),
                })]);
                let outcome = DragAndDropClient::acquire()
                    .map(|client| pollster::block_on(client.start_drag(request)));
                match outcome {
                    Ok(Ok(result)) => tracing::debug!("drag out: {result:?}"),
                    Ok(Err(err)) => tracing::debug!("drag out: {err}"),
                    Err(err) => tracing::warn!("drag out: {err}"),
                }
            });
        if let Err(err) = spawned {
            tracing::warn!("drag out thread: {err}");
        }
    }));
}

/// `name` fit for a file name on every system: no separators or reserved
/// characters, never empty.
fn file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').to_owned();
    if cleaned.is_empty() {
        notin_i18n::tr!("common-untitled")
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_fit_any_file_system() {
        assert_eq!(super::file_name("a/b: c?"), "a_b_ c_");
        assert_eq!(super::file_name("Notas de clase"), "Notas de clase");
    }
}
