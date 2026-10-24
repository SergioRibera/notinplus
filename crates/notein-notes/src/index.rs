//! Titles and headings of every note, for the reference picker.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

use freya_markdown_editor::document::Document;
use crate::anchor::NoteAnchor;

/// What the index knows about one note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedNote {
    pub title: String,
    /// `(slug, text)` in document order.
    pub headings: Vec<(String, String)>,
    /// Plain text of the body, for full-text search.
    pub body: String,
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
            body: markdown.to_owned(),
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

    /// Read access for search.
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<[u8; 16], IndexedNote>> {
        self.0.read().unwrap_or_else(PoisonError::into_inner)
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
}
