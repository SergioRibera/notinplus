//! Share menu of a text note: send it through the system share sheet
//! (as text or as a `.md` file) or copy its markdown.

use freya::icons::lucide;
use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use istmo_share::{ShareClient, ShareFile, ShareRequest};
use notein_plugin::NotificationKind;

use crate::components::{FabMenu, FabMenuEntry};

/// Width of the menu card.
const MENU_WIDTH: f32 = 280.0;
/// Gap between the share button and the menu.
const MENU_GAP: f32 = 22.0;

/// Open the share menu for the note in `controller`, below the press at
/// `at` in `window` coordinates, or under the top-right corner without one.
pub(crate) fn open_menu(
    controller: EditorController,
    title: String,
    at: Option<(f32, f32)>,
    window: Option<Area>,
) {
    let (right, top) = match (at, window) {
        (Some((x, y)), Some(window)) => ((window.max_x() - x - MENU_GAP).max(8.0), y + MENU_GAP),
        _ => (16.0, 72.0),
    };
    spawn(async move {
        let can_share = match ShareClient::acquire() {
            Ok(client) => client
                .capabilities()
                .await
                .is_ok_and(|capabilities| capabilities.send),
            Err(_) => false,
        };
        let mut menu = FabMenu::new().width(MENU_WIDTH).anchor_top(right, top);
        if can_share {
            let text_title = title.clone();
            menu = menu
                .entry(FabMenuEntry::new(
                    lucide::share_2(),
                    "Compartir como texto",
                    move || share_text(controller, text_title.clone()),
                ))
                .entry(FabMenuEntry::new(
                    lucide::file_down(),
                    "Compartir archivo .md",
                    move || share_file(controller, title.clone()),
                ));
        }
        let copy = FabMenuEntry::new(lucide::copy(), "Copiar markdown", move || {
            copy_markdown(controller, Dialect::Portable);
        });
        menu.entry(if can_share {
            copy.with_divider_above()
        } else {
            copy
        })
        .entry(FabMenuEntry::new(
            lucide::clipboard_copy(),
            "Copiar markdown enriquecido",
            move || copy_markdown(controller, Dialect::Rich),
        ))
        .open();
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
    spawn(async move {
        let result = match ShareClient::acquire() {
            Ok(client) => client
                .share(request)
                .await
                .map_err(istmo_share::ShareError::from),
            Err(err) => Err(istmo_share::ShareError::from(err)),
        };
        if let Err(err) = result {
            log::warn!("notes: share failed: {err}");
            notify(NotificationKind::Error, "No se pudo compartir la nota");
        }
    });
}

fn copy_markdown(controller: EditorController, dialect: Dialect) {
    match freya::clipboard::Clipboard::set(controller.markdown(dialect)) {
        Ok(()) => notify(NotificationKind::Success, "Markdown copiado"),
        Err(err) => {
            log::warn!("notes: copy failed: {err:?}");
            notify(NotificationKind::Error, "No se pudo copiar");
        }
    }
}

fn notify(kind: NotificationKind, message: &str) {
    crate::host_api::host_handle().notify(notein_notes::KIND_ID, kind, message.to_owned());
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
