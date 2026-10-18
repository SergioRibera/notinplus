//! The note page as a canvas kind.
//!
//! Notes render through a Freya page in the shell, so the instance
//! lifecycle, input and snapshot hooks are inert: the shell stores the
//! markdown body itself. What this kind contributes is the creation entry
//! and the heading outline.

use std::sync::atomic::{AtomicU64, Ordering};

use freya_markdown_editor::document::{Document, HeadingRef};
use notein_sdk::{
    Bookmark, BookmarkId, CanvasError, CanvasKind, ConfigError, ConfigSchema, ConfigValue,
    CreationCategory, CreationEntry, CreationError, CreationKind, FrameCtxHandle, InputEvent,
    ItemCreationResult, Outline, OutlineNode, OutlineNodeId, WorldPoint, build::custom_entry,
};

use crate::anchor::NoteAnchor;
use crate::{IMPORT_EXTENSIONS, IMPORT_MIME_TYPES, KIND_ID, body_from_snapshot, import_name};

/// Creation entry id of a blank note.
pub const NEW_NOTE: &str = "new-note";
/// Creation entry id of a note imported from a markdown file.
pub const IMPORT_MARKDOWN: &str = "import-markdown";

#[derive(Debug, Default)]
pub struct NotesCanvas {
    next_instance: AtomicU64,
}

impl CanvasKind for NotesCanvas {
    fn kind_id(&self) -> &str {
        KIND_ID
    }

    fn kind_label(&self) -> &'static str {
        "Nota"
    }

    fn capabilities(&self) -> &[&'static str] {
        &[]
    }

    fn open(&self, _item_bytes: &[u8]) -> Result<u64, CanvasError> {
        Ok(self.next_instance.fetch_add(1, Ordering::Relaxed))
    }

    fn close(&self, _instance: u64) {}

    fn render(&self, _instance: u64, _ctx: FrameCtxHandle) {}

    fn on_input(&self, _instance: u64, _event: InputEvent) {}

    fn create_bookmark(&self, _instance: u64, _at: WorldPoint) -> Option<BookmarkId> {
        None
    }

    fn list_bookmarks(&self, _instance: u64) -> Vec<Bookmark> {
        Vec::new()
    }

    fn update_bookmark(&self, _instance: u64, _bookmark: Bookmark) {}

    fn delete_bookmark(&self, _instance: u64, _id: BookmarkId) {}

    fn navigate_to_bookmark(&self, _instance: u64, _id: BookmarkId) {}

    fn serialize_snapshot(&self, _instance: u64) -> Vec<u8> {
        Vec::new()
    }

    fn apply_remote_op(&self, _instance: u64, _op: &[u8]) {}

    /// Headings nested by level.
    fn outline(&self, snapshot: &[u8]) -> Outline {
        let headings = Document::parse(&body_from_snapshot(snapshot)).outline();
        Outline {
            nodes: nest(&headings),
            revision: 0,
        }
    }

    fn outline_anchor(&self, snapshot: &[u8], node: OutlineNodeId) -> Option<Vec<u8>> {
        let index = usize::try_from(node.0).ok()?;
        let headings = Document::parse(&body_from_snapshot(snapshot)).outline();
        let heading = headings.into_iter().nth(index)?;
        Some(NoteAnchor::heading(heading.slug).encode())
    }

    fn config_schema(&self) -> ConfigSchema {
        ConfigSchema::empty()
    }

    fn get_config(&self, _key: &str) -> Option<ConfigValue> {
        None
    }

    fn set_config(&self, key: &str, _value: ConfigValue) -> Result<(), ConfigError> {
        Err(ConfigError::UnknownKey(key.to_owned()))
    }

    fn creation_entries(&self) -> Vec<CreationEntry> {
        vec![
            custom_entry(
                NEW_NOTE,
                "Nota de texto",
                freya_icons::lucide::notebook_pen(),
                CreationCategory::Canvas,
            ),
            CreationEntry {
                id: IMPORT_MARKDOWN.to_owned(),
                label: "Importar markdown".to_owned(),
                icon: freya_icons::lucide::file_input(),
                category: CreationCategory::Source,
                kind: CreationKind::Import {
                    dialog_title: "Elegí un archivo markdown".to_owned(),
                    extensions: IMPORT_EXTENSIONS.map(str::to_owned).to_vec(),
                    mime_types: IMPORT_MIME_TYPES.map(str::to_owned).to_vec(),
                },
            },
        ]
    }

    fn execute_creation(
        &self,
        entry_id: &str,
        import_bytes: Option<&[u8]>,
        name_hint: Option<&str>,
    ) -> Result<ItemCreationResult, CreationError> {
        match entry_id {
            NEW_NOTE => Ok(ItemCreationResult {
                item_bytes: Vec::new(),
                suggested_name: "Nota".into(),
            }),
            IMPORT_MARKDOWN => {
                let bytes = import_bytes.ok_or(CreationError::MissingImportBytes)?;
                Ok(ItemCreationResult {
                    item_bytes: Vec::new(),
                    suggested_name: import_name(name_hint, &body_from_snapshot(bytes)),
                })
            }
            other => Err(CreationError::UnknownEntry(other.into())),
        }
    }
}

/// Turn the flat heading list into a tree: a heading owns the deeper
/// headings that follow it. Node ids are positions in `headings`.
fn nest(headings: &[HeadingRef]) -> Vec<OutlineNode> {
    fn attach(stack: &mut [(u8, OutlineNode)], roots: &mut Vec<OutlineNode>, node: OutlineNode) {
        match stack.last_mut() {
            Some((_, parent)) => parent.children.push(node),
            None => roots.push(node),
        }
    }
    let mut roots = Vec::new();
    let mut stack: Vec<(u8, OutlineNode)> = Vec::new();
    for (index, heading) in headings.iter().enumerate() {
        let level = heading.level.get();
        while let Some((_, done)) = stack.pop_if(|(open, _)| *open >= level) {
            attach(&mut stack, &mut roots, done);
        }
        let label = if heading.text.trim().is_empty() {
            "(sin título)".to_owned()
        } else {
            heading.text.clone()
        };
        stack.push((level, OutlineNode::new(OutlineNodeId(index as u64), label)));
    }
    while let Some((_, done)) = stack.pop() {
        attach(&mut stack, &mut roots, done);
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outline_nests_headings_by_level() {
        let note = b"# A\n\n## A.1\n\n### A.1.a\n\n## A.2\n\n# B";
        let outline = NotesCanvas::default().outline(note);
        let labels = |nodes: &[OutlineNode]| -> Vec<String> {
            nodes.iter().map(|node| node.label.clone()).collect()
        };
        assert_eq!(labels(&outline.nodes), ["A", "B"]);
        assert_eq!(labels(&outline.nodes[0].children), ["A.1", "A.2"]);
        assert_eq!(labels(&outline.nodes[0].children[0].children), ["A.1.a"]);
    }

    #[test]
    fn outline_anchor_points_at_the_heading_slug() {
        let note = b"# Intro\n\n## Intro";
        let canvas = NotesCanvas::default();
        let anchor = canvas
            .outline_anchor(note, OutlineNodeId(1))
            .and_then(|bytes| NoteAnchor::decode(&bytes));
        assert_eq!(anchor, Some(NoteAnchor::heading("intro-1")));
    }
}
