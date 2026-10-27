//! Share menu of a text note: send it through the system share sheet
//! (as text or as a `.md` file) or copy its markdown.

use freya::icons::lucide;
use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use istmo_share::{ShareClient, ShareFile, ShareRequest};
use notein_ui_kit::{Action, ActionEntry, ActionList};

use crate::actions::run;

/// Open the share menu for the note in `controller`, below the press at
/// `at` in `window` coordinates, or under the top-right corner without one.
pub(crate) fn open_menu(
    controller: EditorController,
    title: String,
    at: Option<(f32, f32)>,
    window: Option<Area>,
) {
    let (x, y) = match (at, window) {
        (Some(at), _) => at,
        (None, Some(window)) => (window.max_x() - 16.0, 72.0),
        (None, None) => (16.0, 72.0),
    };
    let anchor = Area::new((x, y).into(), (1.0, 1.0).into());
    spawn(async move {
        let can_share = match ShareClient::acquire() {
            Ok(client) => client
                .capabilities()
                .await
                .is_ok_and(|capabilities| capabilities.send),
            Err(_) => false,
        };
        let mut entries = Vec::new();
        if can_share {
            let text_title = title.clone();
            entries.push(ActionEntry::Item(
                Action::new(
                    "Compartir como texto",
                    run(move || share_text(controller, text_title.clone())),
                )
                .icon(lucide::share_2()),
            ));
            entries.push(ActionEntry::Item(
                Action::new(
                    "Compartir archivo .md",
                    run(move || share_file(controller, title.clone())),
                )
                .icon(lucide::file_down()),
            ));
            entries.push(ActionEntry::Divider);
        }
        entries.push(ActionEntry::Item(
            Action::new(
                "Copiar markdown",
                run(move || copy_markdown(controller, Dialect::Portable)),
            )
            .icon(lucide::copy()),
        ));
        entries.push(ActionEntry::Item(
            Action::new(
                "Copiar markdown enriquecido",
                run(move || copy_markdown(controller, Dialect::Rich)),
            )
            .icon(lucide::clipboard_copy()),
        ));
        ActionList::new(entries).show_at(anchor);
    });
}

fn share_text(controller: EditorController, title: String) {
    let request = ShareRequest::text(controller.markdown(Dialect::Portable)).with_subject(title);
    send(request);
}

fn share_file(controller: EditorController, title: String) {
    let file = ShareFile::bytes(
        file_name(&title),
        controller.markdown(Dialect::Portable).into_bytes(),
    )
    .with_mime_type("text/markdown");
    send(ShareRequest::files([file]).with_subject(title));
}

fn send(request: ShareRequest) {
    // Menu entries close the menu before running, which unmounts the scope
    // `spawn` would tie this task to. Dropping the task drops the call, and
    // istmo cancels a dropped call: Android never got to show the sheet.
    spawn_in_window(async move {
        let result = match ShareClient::acquire() {
            Ok(client) => client
                .share(request)
                .await
                .map_err(istmo_share::ShareError::from),
            Err(err) => Err(istmo_share::ShareError::from(err)),
        };
        if let Err(err) = result {
            log::warn!("notes: share failed: {err}");
            notify(true, "No se pudo compartir la nota");
        }
    });
}

fn copy_markdown(controller: EditorController, dialect: Dialect) {
    match freya::clipboard::Clipboard::set(controller.markdown(dialect)) {
        Ok(()) => notify(false, "Markdown copiado"),
        Err(err) => {
            log::warn!("notes: copy failed: {err:?}");
            notify(true, "No se pudo copiar");
        }
    }
}

fn notify(error: bool, message: &str) {
    let toast = notein_ui_kit::Toast::new(message.to_owned());
    notein_ui_kit::Toasts::get().show(if error { toast.tone(notein_ui_kit::ToastTone::Error) } else { toast });
}

/// `title` as a file name: path separators and reserved characters out,
/// `.md` appended.
fn file_name(title: &str) -> String {
    let stem: String = title
        .trim()
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let stem = stem.trim_matches(['.', ' ']);
    if stem.is_empty() {
        "nota.md".to_owned()
    } else {
        format!("{stem}.md")
    }
}

#[cfg(test)]
mod tests {
    use super::file_name;

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("Ideas: 2026/10"), "Ideas- 2026-10.md");
        assert_eq!(file_name("  "), "nota.md");
        assert_eq!(file_name("..x.."), "x.md");
    }
}
