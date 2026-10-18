//! Titles and headings of every note, for the reference picker.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use freya_markdown_editor::document::Document;
use notein_sdk::{Completion, SourceRef};

use crate::SOURCE_ID;
use crate::anchor::NoteAnchor;

/// Most completions offered for one query.
const COMPLETION_LIMIT: usize = 60;

#[derive(Clone, Debug, PartialEq, Eq)]
struct IndexedNote {
    title: String,
    /// `(slug, text)` in document order.
    headings: Vec<(String, String)>,
}

/// Shared, thread safe map of library item id to note title + headings.
#[derive(Clone, Debug, Default)]
pub struct NotesIndex(Arc<RwLock<HashMap<[u8; 16], IndexedNote>>>);

impl NotesIndex {
    /// Store (or refresh) the note `item`.
    pub fn update(&self, item: [u8; 16], title: &str, markdown: &str) {
        let headings = Document::parse(markdown)
            .outline()
            .into_iter()
            .filter(|heading| !heading.text.trim().is_empty())
            .map(|heading| (heading.slug, heading.text))
            .collect();
        let note = IndexedNote {
            title: title.to_owned(),
            headings,
        };
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(item, note);
    }

    /// Refresh the headings of a known note, keeping its title.
    pub fn update_body(&self, item: [u8; 16], markdown: &str) {
        let title = self
            .0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&item)
            .map(|note| note.title.clone());
        if let Some(title) = title {
            self.update(item, &title, markdown);
        }
    }

    pub fn remove(&self, item: [u8; 16]) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&item);
    }

    /// Drop every note, before re-populating from the library.
    pub fn clear(&self) {
        self.0
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// Current label of a reference to `item`: the note title, plus the
    /// heading text when the anchor names one. `None` when the note is
    /// gone; a heading that no longer exists falls back to the title.
    #[must_use]
    pub fn label(&self, item: [u8; 16], anchor: &NoteAnchor) -> Option<String> {
        let note = self
            .0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&item)?
            .clone();
        let heading = anchor.heading.as_deref().and_then(|slug| {
            note.headings
                .iter()
                .find(|(candidate, _)| candidate == slug)
                .map(|(_, text)| text)
        });
        Some(heading.map_or_else(
            || note.title.clone(),
            |text| format!("{} › {text}", note.title),
        ))
    }

    /// Notes and headings whose label contains `query` (case
    /// insensitive), notes first, sorted by title.
    #[must_use]
    pub fn completions(&self, query: &str) -> Vec<Completion> {
        let needle = query.trim().to_lowercase();
        let matches = |label: &str| needle.is_empty() || label.to_lowercase().contains(&needle);
        let mut sorted: Vec<([u8; 16], IndexedNote)> = self
            .0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(item, note)| (*item, note.clone()))
            .collect();
        sorted.sort_by_key(|(_, note)| note.title.to_lowercase());

        let reference = |item: [u8; 16], anchor: NoteAnchor| SourceRef::Plugin {
            plugin_id: SOURCE_ID.to_owned(),
            item,
            anchor: anchor.encode(),
            selection: None,
        };
        let mut out = Vec::new();
        for (item, note) in &sorted {
            if matches(&note.title) {
                out.push(Completion {
                    label: note.title.clone(),
                    subtitle: Some("Nota".to_owned()),
                    source: reference(*item, NoteAnchor::default()),
                });
            }
        }
        for (item, note) in &sorted {
            for (slug, text) in &note.headings {
                let label = format!("{} › {text}", note.title);
                if matches(&label) {
                    out.push(Completion {
                        label,
                        subtitle: Some("Sección de nota".to_owned()),
                        source: reference(*item, NoteAnchor::heading(slug)),
                    });
                }
            }
        }
        out.truncate(COMPLETION_LIMIT);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completions_cover_notes_and_headings() {
        let index = NotesIndex::default();
        index.update([1; 16], "Diario", "# Lunes\n\ntexto\n\n## Ideas");
        index.update([2; 16], "Recetas", "# Pan");

        let labels: Vec<String> = index
            .completions("")
            .into_iter()
            .map(|completion| completion.label)
            .collect();
        assert_eq!(
            labels,
            [
                "Diario",
                "Recetas",
                "Diario › Lunes",
                "Diario › Ideas",
                "Recetas › Pan"
            ]
        );

        let ideas = index.completions("idea");
        assert_eq!(ideas.len(), 1);
        let SourceRef::Plugin { item, anchor, .. } = &ideas[0].source else {
            panic!("plugin ref");
        };
        assert_eq!(*item, [1; 16]);
        assert_eq!(
            NoteAnchor::decode(anchor),
            Some(NoteAnchor::heading("ideas"))
        );
    }
}
