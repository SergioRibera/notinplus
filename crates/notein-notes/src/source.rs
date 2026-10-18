//! References into notes.
//!
//! Notes are not tiled sources: this half only offers notes and their
//! headings to the reference picker and hands chip taps to the page.

use notein_sdk::{
    AnchorView, Completion, ConfigError, ConfigSchema, ConfigValue, CreationEntry, CreationError,
    ItemCreationResult, SearchHit, Selection, Source, SourceError, Tile, TileRequest,
};

use crate::anchor::NoteAnchor;
use crate::index::NotesIndex;
use crate::{KIND_ID, SOURCE_ID};

#[derive(Debug)]
pub struct NotesSource {
    index: NotesIndex,
}

impl NotesSource {
    #[must_use]
    pub const fn new(index: NotesIndex) -> Self {
        Self { index }
    }
}

impl Source for NotesSource {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn kind_id(&self) -> &str {
        SOURCE_ID
    }

    fn kind_label(&self) -> &'static str {
        "Notas"
    }

    fn file_extensions(&self) -> &[&'static str] {
        &[]
    }

    fn open(&self, _item_bytes: &[u8]) -> Result<u64, SourceError> {
        Err(SourceError::OpenFailed("notes open as pages".into()))
    }

    fn close(&self, _instance: u64) {}

    fn title(&self, _instance: u64) -> String {
        "Nota".into()
    }

    fn content_size(&self, _instance: u64) -> (f32, f32) {
        (0.0, 0.0)
    }

    fn render_tile(&self, _instance: u64, _req: TileRequest) -> Result<Tile, SourceError> {
        Err(SourceError::RenderFailed("notes are not tiled".into()))
    }

    fn hit_test(&self, _instance: u64, _x: f32, _y: f32) -> Option<Vec<u8>> {
        None
    }

    fn resolve_anchor(&self, _instance: u64, anchor: &[u8]) -> Option<AnchorView> {
        let anchor = NoteAnchor::decode(anchor)?;
        Some(AnchorView {
            title: "Nota".into(),
            snippet: anchor.heading.unwrap_or_default(),
        })
    }

    fn current_selection(&self, _instance: u64) -> Option<Selection> {
        None
    }

    fn search_begin(&self, _instance: u64, _query: &str) -> u64 {
        0
    }

    fn search_poll(&self, _stream_id: u64) -> Vec<SearchHit> {
        Vec::new()
    }

    fn search_end(&self, _stream_id: u64) {}

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
        Vec::new()
    }

    fn execute_creation(
        &self,
        entry_id: &str,
        _import_bytes: Option<&[u8]>,
        _name_hint: Option<&str>,
    ) -> Result<ItemCreationResult, CreationError> {
        Err(CreationError::UnknownEntry(entry_id.into()))
    }

    fn plugin_completions(&self, query: &str) -> Vec<Completion> {
        self.index.completions(query)
    }

    /// The page understands the same anchor.
    fn navigate_payload(&self, anchor: &[u8]) -> Option<Vec<u8>> {
        NoteAnchor::decode(anchor).map(|anchor| anchor.encode())
    }

    fn viewer_kind_id(&self) -> &str {
        KIND_ID
    }
}
