//! Text notes plugin.
//!
//! A note is a library item whose content is a markdown document,
//! edited with [`freya_markdown_editor`]. [`NotesKind`] is the document
//! kind: its outline is the heading tree of the note and its anchors
//! point at headings. [`NotesIndex`] keeps every note's title and
//! headings so references can be offered without opening each note.

pub mod anchor;
pub mod kind;
pub mod index;

pub use anchor::NoteAnchor;
pub use kind::{NoteContent, NotesKind};
pub use index::NotesIndex;

/// Kind id of the note page (`PluginCanvas` items and their snapshots).
pub const KIND_ID: &str = kind::KIND;

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
