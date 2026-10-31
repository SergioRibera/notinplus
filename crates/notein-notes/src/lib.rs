//! Text notes plugin.
//!
//! A note is a library item whose content is a markdown document,
//! edited with [`freya_markdown_editor`]. [`NotesKind`] is the document
//! kind: its outline is the heading tree of the note and its anchors
//! point at headings. [`NotesIndex`] keeps every note's title and
//! headings so references can be offered without opening each note.

use notein_sdk::tr;

pub mod anchor;
mod face;
pub mod index;
pub mod kind;
pub mod plugin;
mod share;

pub use anchor::NoteAnchor;
pub use index::NotesIndex;
pub use kind::{NoteContent, NotesKind};
pub use plugin::{NoteFace, NotesPlugin};

notein_sdk::i18n::catalog!();

/// Kind id of the note page (`PluginCanvas` items and their snapshots).
pub const KIND_ID: &str = kind::KIND;

/// File extensions a markdown import accepts.
pub const IMPORT_EXTENSIONS: [&str; 3] = ["md", "markdown", "txt"];
/// MIME types a markdown import accepts.
pub const IMPORT_MIME_TYPES: [&str; 2] = ["text/markdown", "text/plain"];

/// Name of a note imported from `file_name`: the file stem, else the
/// first heading of `markdown`, else a generic name.
#[must_use]
#[cfg_attr(feature = "sismo", sismo::function("notes::import_name"))]
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
        .unwrap_or_else(|| tr!("notes-imported-name"))
}

/// Fix the anchor of the heading `slug` names in `markdown`.
///
/// A reference to it then keeps working when the heading text changes.
/// Returns the updated markdown, or `None` when the heading is missing or its
/// anchor is already fixed.
#[must_use]
#[cfg_attr(feature = "sismo", sismo::function("notes::pin_heading"))]
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

/// What `markdown` holds under the heading `slug` names, up to the next
/// heading of the same or a higher level; the whole note without a slug.
/// `None` when the heading is missing or nothing sits under it.
#[must_use]
#[cfg_attr(feature = "sismo", sismo::function("notes::section"))]
pub fn section(markdown: &str, slug: Option<&str>) -> Option<String> {
    use freya_markdown_editor::document::{BlockKind, Document};
    use freya_markdown_editor::markdown::Dialect;

    let document = Document::parse(markdown);
    let part = match slug {
        None => document,
        Some(slug) => {
            let at = document.index_of(document.heading_by_slug(slug)?)?;
            let blocks = document.blocks();
            let BlockKind::Heading { level, .. } = blocks[at].kind else {
                return None;
            };
            Document::from(
                blocks[at + 1..]
                    .iter()
                    .take_while(|block| {
                        !matches!(block.kind, BlockKind::Heading { level: next, .. } if next <= level)
                    })
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        }
    };
    let text = part.to_markdown(Dialect::Rich).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_section_runs_to_the_next_heading_of_its_level() {
        let note =
            "# Diario\n\nintro\n\n## Ideas\n\nuna\n\n### Detalle\n\n- dos\n\n## Otra\n\nfuera\n";
        let ideas = section(note, Some("ideas")).expect("section");
        assert!(ideas.starts_with("una"), "{ideas}");
        assert!(ideas.contains("### Detalle"), "{ideas}");
        assert!(ideas.contains("- dos"), "{ideas}");
        assert!(!ideas.contains("fuera"), "{ideas}");
        assert!(!ideas.contains("## Ideas"), "{ideas}");
    }

    #[test]
    fn an_empty_or_missing_section_quotes_nothing() {
        let note = "# Diario\n\n## Vacia\n\n## Llena\n\ntexto\n";
        assert_eq!(section(note, Some("vacia")), None);
        assert_eq!(section(note, Some("nada")), None);
        assert!(section(note, None).is_some_and(|all| all.contains("texto")));
    }

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
        notein_sdk::i18n::Lang::Es.make_current();
        assert_eq!(import_name(Some("Plan.MD"), "# Otro"), "Plan");
        assert_eq!(import_name(Some("notas.txt"), ""), "notas");
        assert_eq!(import_name(None, "texto\n\n## Viaje\n"), "Viaje");
        assert_eq!(import_name(Some(" .md"), "sin títulos"), "Nota importada");
    }
}
