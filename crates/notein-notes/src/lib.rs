//! Text notes plugin.
//!
//! A note is a library item whose content is a markdown document,
//! edited with [`freya_markdown_editor`]. The crate exposes:
//!
//! - [`NotesCanvas`] — the page kind (`io.notein.notes.viewer`). Its
//!   outline is the heading tree of the note, which the shell shows in
//!   the sidebar "Contents" tab.
//! - [`NotesSource`] — lets any other note or bookmark reference a note
//!   or one of its headings through the `/` menu, and routes chip taps
//!   back to the page.
//! - [`ui`] — the page chrome (top bar).
//!
//! The note body is stored by the shell as the item snapshot: the raw
//! UTF-8 markdown. The shell keeps [`NotesIndex`] up to date with every
//! note's title and headings so references can be offered without
//! opening each note.

use std::sync::Arc;

use notein_sdk::{PluginRegistry, RegistryError};

pub mod anchor;
pub mod canvas;
pub mod index;
pub mod source;
pub mod ui;

pub use anchor::NoteAnchor;
pub use canvas::NotesCanvas;
pub use index::NotesIndex;
pub use source::NotesSource;

/// Kind id of the note page (`PluginCanvas` items and their snapshots).
pub const KIND_ID: &str = "io.notein.notes.viewer";

/// Id of the reference source (`SourceRef::Plugin::plugin_id`).
pub const SOURCE_ID: &str = "io.notein.notes";

/// Register both plugin halves. The returned index is shared with them:
/// the shell feeds it note titles and bodies.
pub fn install(registry: &mut PluginRegistry) -> Result<NotesIndex, RegistryError> {
    let index = NotesIndex::default();
    registry.register_canvas(Arc::new(NotesCanvas::default()))?;
    registry.register_source(Arc::new(NotesSource::new(index.clone())))?;
    Ok(index)
}

/// Markdown body stored in a note snapshot.
#[must_use]
pub fn body_from_snapshot(snapshot: &[u8]) -> String {
    String::from_utf8_lossy(snapshot).into_owned()
}
