//! Pieces of the shell's own interface a face can use, so a plugin's
//! text reads and edits like every other text in Notein.
//!
//! They live on the UI thread: [`HostUi`] is handed to faces in their
//! [`FaceContext`](crate::face::FaceContext) and is not `Send`.

use std::rc::Rc;

use bytes::Bytes;
use freya_markdown_editor::prelude::{EditorTheme, MarkdownEditor};

use crate::bookmark::SourceRef;
use crate::doc::DocId;

/// Shared handle to the shell's interface. Cheap to clone.
pub type Ui = Rc<dyn HostUi>;

/// A button of the selection bar only one kind has: label, icon and
/// what it does ("Marcador" in PDFs).
pub type SelectionExtra = (&'static str, Bytes, Rc<dyn Fn()>);

/// The bar over a text selection: five highlighters, a note, copy as
/// reference and send to the notebook.
#[derive(Clone)]
pub struct SelectionMenu {
    /// Window point the selection ends at; the bar floats above it.
    pub at: (f32, f32),
    /// Index of the highlighter already on the selection, among the
    /// palette's `highlighters`.
    pub active: Option<usize>,
    /// Highlight with one of the inks (`None` removes it).
    pub on_color: Rc<dyn Fn(Option<[u8; 3]>)>,
    /// Open the annotation for the selection; `None` hides Nota.
    pub on_note: Option<Rc<dyn Fn()>>,
    /// The place as a reference, for Al cuaderno.
    pub reference: Option<SourceRef>,
    /// The document the selection is in, for Al cuaderno.
    pub document: Option<DocId>,
    pub extra: Vec<SelectionExtra>,
    /// What the bar says it works on ("3 versículos").
    pub caption: Option<String>,
    /// The selection grows and shrinks while the bar shows: the bar docks
    /// at the bottom, taps outside it reach the document, and only its
    /// close button dismisses it. Showing the bar again replaces it.
    pub sticky: bool,
    /// The bar went away without an action.
    pub on_dismiss: Rc<dyn Fn()>,
}

impl PartialEq for SelectionMenu {
    fn eq(&self, other: &Self) -> bool {
        self.at == other.at && self.active == other.active && self.caption == other.caption
    }
}

impl std::fmt::Debug for SelectionMenu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectionMenu")
            .field("at", &self.at)
            .finish_non_exhaustive()
    }
}

/// The shell's interface, as faces use it.
pub trait HostUi {
    /// Set `editor` up as every editor in Notein: the theme, reference
    /// chips for `notein://` links, the `/ref` suggestions, links that
    /// open through the shell and the user's markdown-marks setting.
    fn editor(&self, editor: MarkdownEditor) -> MarkdownEditor;

    /// The editor theme of the active palette, for toolbars.
    fn editor_theme(&self) -> EditorTheme;

    /// Show the selection bar.
    fn selection(&self, menu: SelectionMenu);

    /// Take a sticky selection bar away, as when its selection emptied.
    fn close_selection(&self) {}
}
