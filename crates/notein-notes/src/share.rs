//! Share menu of a text note: send it through the system share sheet
//! (as text or as a `.md` file) or copy its markdown.

use freya::icons::lucide;
use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use notein_sdk::services::{Host, HostToast, Share, ToastTone};
use notein_sdk::tr;
use notein_ui_kit::{Action, ActionEntry, ActionList};

/// Open the share menu for the note in `controller`, below the press at
/// `at` in `window` coordinates, or under the top-right corner without one.
pub(crate) fn open_menu(
    host: &Host,
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
    let mut entries = Vec::new();
    if host.can_share() {
        let (text_host, text_title) = (host.clone(), title.clone());
        entries.push(ActionEntry::Item(
            Action::closing(tr!("notes-share-as-text"), move || {
                text_host.share(Share::Text {
                    text: controller.markdown(Dialect::Portable),
                    subject: text_title.clone(),
                });
            })
            .icon(lucide::share_2()),
        ));
        let file_host = host.clone();
        entries.push(ActionEntry::Item(
            Action::closing(tr!("notes-share-as-file"), move || {
                file_host.share(Share::File {
                    name: file_name(&title),
                    mime: "text/markdown".to_owned(),
                    bytes: controller.markdown(Dialect::Portable).into_bytes(),
                    subject: title.clone(),
                });
            })
            .icon(lucide::file_down()),
        ));
    }
    let portable = host.clone();
    entries.push(ActionEntry::Item(
        Action::closing(tr!("notes-share-copy-markdown"), move || {
            copy_markdown(&portable, controller, Dialect::Portable);
        })
        .icon(lucide::copy()),
    ));
    let rich = host.clone();
    entries.push(ActionEntry::Item(
        Action::closing(tr!("notes-share-copy-rich"), move || {
            copy_markdown(&rich, controller, Dialect::Rich);
        })
        .icon(lucide::clipboard_copy()),
    ));
    ActionList::new(entries).show_at(anchor);
}

fn copy_markdown(host: &Host, controller: EditorController, dialect: Dialect) {
    let toast = match freya::clipboard::Clipboard::set(controller.markdown(dialect)) {
        Ok(()) => HostToast::new(tr!("notes-toast-markdown-copied"), ToastTone::Info),
        Err(err) => {
            log::warn!("notes: copy failed: {err:?}");
            HostToast::new(tr!("notes-toast-copy-failed"), ToastTone::Error)
        }
    };
    host.toast(toast);
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
