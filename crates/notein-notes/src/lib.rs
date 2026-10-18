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

/// File extensions a markdown import accepts.
pub const IMPORT_EXTENSIONS: [&str; 3] = ["md", "markdown", "txt"];
/// MIME types a markdown import accepts.
pub const IMPORT_MIME_TYPES: [&str; 2] = ["text/markdown", "text/plain"];

/// Name of a note imported from `file_name`: the file stem, else the
/// first heading of `markdown`, else a generic name.
#[must_use]
pub fn import_name(file_name: Option<&str>, markdown: &str) -> String {
    let stem = file_name
        .map(|name| {
            let lower = name.to_ascii_lowercase();
            IMPORT_EXTENSIONS
                .iter()
                .find_map(|ext| {
                    lower
                        .strip_suffix(&format!(".{ext}"))
                        .map(|stem| &name[..stem.len()])
                })
                .unwrap_or(name)
                .trim()
        })
        .filter(|stem| !stem.is_empty());
    if let Some(stem) = stem {
        return stem.to_owned();
    }
    freya_markdown_editor::document::Document::parse(markdown)
        .outline()
        .into_iter()
        .map(|heading| heading.text)
        .find(|text| !text.trim().is_empty())
        .unwrap_or_else(|| "Nota importada".to_owned())
}

/// Fix the anchor of the heading `slug` names in `markdown`.
///
/// A reference to it then keeps working when the heading text changes.
/// Returns the updated markdown, or `None` when the heading is missing or its
/// anchor is already fixed.
#[must_use]
pub fn pin_heading(markdown: &str, slug: &str) -> Option<String> {
    use freya_markdown_editor::document::Document;
    use freya_markdown_editor::markdown::Dialect;

    let mut document = Document::parse(markdown);
    let heading = document
        .outline()
        .into_iter()
        .find(|heading| heading.slug == slug && !heading.pinned)?;
    document.set_heading_anchor(heading.block, Some(heading.slug));
    Some(document.to_markdown(Dialect::Rich))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_heading_keeps_its_anchor_after_a_rename() {
        let note = "# Diario\n\n## Ideas\n\ntexto\n";
        let pinned = pin_heading(note, "ideas").expect("heading found");
        assert!(pinned.contains("## Ideas {#ideas}"), "{pinned}");
        assert_eq!(pin_heading(&pinned, "ideas"), None);

        let index = NotesIndex::default();
        index.update(
            [7; 16],
            "Diario",
            &pinned.replace("## Ideas", "## Proyectos"),
        );
        assert_eq!(
            index
                .label([7; 16], &NoteAnchor::heading("ideas"))
                .as_deref(),
            Some("Diario › Proyectos")
        );
    }

    #[test]
    fn imported_notes_are_named_after_the_file_or_first_heading() {
        assert_eq!(import_name(Some("Plan.MD"), "# Otro"), "Plan");
        assert_eq!(import_name(Some("notas.txt"), ""), "notas");
        assert_eq!(import_name(None, "texto\n\n## Viaje\n"), "Viaje");
        assert_eq!(import_name(Some(" .md"), "sin títulos"), "Nota importada");
    }
}
