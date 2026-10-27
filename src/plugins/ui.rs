//! The pieces of the shell's interface plugins get to use.

use std::rc::Rc;

use freya_markdown_editor::prelude::{EditorTheme, MarkdownEditor};
use notein_plugin::doc::DocId;
use notein_plugin::ui::{HostUi, SelectionMenu, Ui};

use crate::library::model::ItemId;

/// The shell's [`HostUi`].
#[derive(Debug, Default)]
pub struct ShellUi;

impl HostUi for ShellUi {
    fn editor(&self, editor: MarkdownEditor) -> MarkdownEditor {
        crate::editor::configure(editor)
    }

    fn editor_for(&self, editor: MarkdownEditor, doc: DocId) -> MarkdownEditor {
        crate::editor::configure_for(editor, &crate::library::bodies::item_root(ItemId(doc.0)))
    }

    fn editor_theme(&self) -> EditorTheme {
        crate::editor::theme(&notein_ui_kit::palette())
    }

    fn selection(&self, menu: SelectionMenu) {
        crate::selection::show(menu);
    }

    fn close_selection(&self) {
        crate::selection::close_sticky();
    }
}

/// The handle faces receive.
#[must_use]
pub fn handle() -> Ui {
    Rc::new(ShellUi)
}
