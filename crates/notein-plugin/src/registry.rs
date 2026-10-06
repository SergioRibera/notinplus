//! Host-side registry of built-in plugins.
//!
//! Phase 2.3 — in-process only. Plugins are compiled into the host
//! binary and handed to the registry via [`PluginRegistry::register_source`]
//! or [`PluginRegistry::register_canvas`] at startup. Phase 3 adds
//! dynamic `.so` / `.dll` loading on top of the same registry type —
//! the trait objects go in through the same door, just produced by a
//! `dlopen` instead of a `::new()`.
//!
//! The home FAB `+` is driven by [`PluginRegistry::list_creation_entries`]
//! and the settings panel reads [`PluginRegistry::all_schemas`]; both
//! methods aggregate across every registered plugin so adding a plugin
//! automatically adds its creation entries and its settings section
//! without the shell needing to know anything about the plugin's
//! domain.

use std::collections::HashMap;
use std::sync::Arc;

use crate::canvas::CanvasKind;
use crate::config::ConfigSchema;
use crate::creation::CreationEntry;
use crate::source::Source;

/// Which trait a plugin implements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PluginRole {
    Source,
    Canvas,
}

/// Error returned by registration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryError {
    /// A plugin with the same `kind_id` is already registered under
    /// the same role.
    DuplicateKindId {
        role: PluginRole,
        kind_id: String,
    },
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateKindId { role, kind_id } => {
                write!(f, "duplicate {role:?} plugin kind_id: {kind_id}")
            }
        }
    }
}

impl std::error::Error for RegistryError {}

/// One creation entry with the plugin that owns it attached, as
/// returned by [`PluginRegistry::list_creation_entries`]. The host
/// routes the user's pick back to this plugin's `execute_creation`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryCreationEntry {
    pub plugin_role: PluginRole,
    pub plugin_id: String,
    pub plugin_label: String,
    pub entry: CreationEntry,
}

/// One plugin's settings schema with the metadata needed to render a
/// section header, as returned by [`PluginRegistry::all_schemas`].
#[derive(Clone, Debug, PartialEq)]
pub struct RegistrySchemaEntry {
    pub plugin_role: PluginRole,
    pub plugin_id: String,
    pub plugin_label: String,
    pub schema: ConfigSchema,
}

/// Registry of all plugins known to the running shell.
///
/// Not internally synchronised — the caller wraps the whole registry
/// in whatever locking they want (startup-only mutation on a single
/// thread needs none; hot-reload wants an `RwLock`).
#[derive(Default)]
pub struct PluginRegistry {
    sources: HashMap<String, Arc<dyn Source>>,
    canvases: HashMap<String, Arc<dyn CanvasKind>>,
}

impl std::fmt::Debug for PluginRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginRegistry")
            .field("sources", &self.sources.keys().collect::<Vec<_>>())
            .field("canvases", &self.canvases.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl PluginRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a built-in source plugin. Fails if another source
    /// with the same `kind_id` is already present.
    pub fn register_source(&mut self, plugin: Arc<dyn Source>) -> Result<(), RegistryError> {
        let kind_id = plugin.kind_id().to_owned();
        if self.sources.contains_key(&kind_id) {
            return Err(RegistryError::DuplicateKindId {
                role: PluginRole::Source,
                kind_id,
            });
        }
        self.sources.insert(kind_id, plugin);
        Ok(())
    }

    /// Register a built-in canvas plugin. Fails if another canvas
    /// with the same `kind_id` is already present.
    pub fn register_canvas(&mut self, plugin: Arc<dyn CanvasKind>) -> Result<(), RegistryError> {
        let kind_id = plugin.kind_id().to_owned();
        if self.canvases.contains_key(&kind_id) {
            return Err(RegistryError::DuplicateKindId {
                role: PluginRole::Canvas,
                kind_id,
            });
        }
        self.canvases.insert(kind_id, plugin);
        Ok(())
    }

    #[must_use]
    pub fn get_source(&self, kind_id: &str) -> Option<&Arc<dyn Source>> {
        self.sources.get(kind_id)
    }

    #[must_use]
    pub fn get_canvas(&self, kind_id: &str) -> Option<&Arc<dyn CanvasKind>> {
        self.canvases.get(kind_id)
    }

    pub fn sources(&self) -> impl Iterator<Item = &Arc<dyn Source>> {
        self.sources.values()
    }

    pub fn canvases(&self) -> impl Iterator<Item = &Arc<dyn CanvasKind>> {
        self.canvases.values()
    }

    #[must_use]
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    #[must_use]
    pub fn canvas_count(&self) -> usize {
        self.canvases.len()
    }

    /// First source plugin whose `file_extensions` include `ext`
    /// (case-insensitive, leading dot stripped). `None` = no plugin
    /// claims the extension.
    #[must_use]
    pub fn find_source_for_extension(&self, ext: &str) -> Option<&Arc<dyn Source>> {
        let needle = ext.trim_start_matches('.').to_ascii_lowercase();
        self.sources.values().find(|p| {
            p.file_extensions()
                .iter()
                .any(|e| e.eq_ignore_ascii_case(&needle))
        })
    }

    /// All creation entries from every registered plugin, each wrapped
    /// with its owning plugin's id + label + role so the home FAB can
    /// route the pick without a second lookup.
    #[must_use]
    pub fn list_creation_entries(&self) -> Vec<RegistryCreationEntry> {
        let mut out = Vec::new();
        for plugin in self.sources.values() {
            let plugin_id = plugin.kind_id().to_owned();
            let plugin_label = plugin.kind_label().to_owned();
            for entry in plugin.creation_entries() {
                out.push(RegistryCreationEntry {
                    plugin_role: PluginRole::Source,
                    plugin_id: plugin_id.clone(),
                    plugin_label: plugin_label.clone(),
                    entry,
                });
            }
        }
        for plugin in self.canvases.values() {
            let plugin_id = plugin.kind_id().to_owned();
            let plugin_label = plugin.kind_label().to_owned();
            for entry in plugin.creation_entries() {
                out.push(RegistryCreationEntry {
                    plugin_role: PluginRole::Canvas,
                    plugin_id: plugin_id.clone(),
                    plugin_label: plugin_label.clone(),
                    entry,
                });
            }
        }
        out
    }

    /// Every plugin's settings schema. The settings panel renders one
    /// section per entry.
    #[must_use]
    pub fn all_schemas(&self) -> Vec<RegistrySchemaEntry> {
        let mut out = Vec::new();
        for plugin in self.sources.values() {
            out.push(RegistrySchemaEntry {
                plugin_role: PluginRole::Source,
                plugin_id: plugin.kind_id().to_owned(),
                plugin_label: plugin.kind_label().to_owned(),
                schema: plugin.config_schema(),
            });
        }
        for plugin in self.canvases.values() {
            out.push(RegistrySchemaEntry {
                plugin_role: PluginRole::Canvas,
                plugin_id: plugin.kind_id().to_owned(),
                plugin_label: plugin.kind_label().to_owned(),
                schema: plugin.config_schema(),
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bookmark::{Bookmark, WorldPoint};
    use crate::canvas::{CanvasKind, FrameCtxHandle, InputEvent};
    use crate::creation::{CreationCategory, CreationEntry, CreationKind, ItemCreationResult};
    use crate::error::{CanvasError, ConfigError, CreationError, SourceError};
    use crate::ids::BookmarkId;
    use crate::source::{AnchorView, SearchHit, Selection, Source, Tile, TileRequest};

    #[derive(Debug)]
    struct StubSource {
        kind_id: &'static str,
        label: &'static str,
        extensions: Vec<&'static str>,
        entries: Vec<CreationEntry>,
    }

    impl StubSource {
        fn new(
            kind_id: &'static str,
            label: &'static str,
            extensions: &[&'static str],
        ) -> Self {
            Self {
                kind_id,
                label,
                extensions: extensions.to_vec(),
                entries: Vec::new(),
            }
        }

        fn with_entry(mut self, entry: CreationEntry) -> Self {
            self.entries.push(entry);
            self
        }
    }

    impl Source for StubSource {
        fn kind_id(&self) -> &str {
            self.kind_id
        }
        fn kind_label(&self) -> &str {
            self.label
        }
        fn file_extensions(&self) -> &[&'static str] {
            &self.extensions
        }
        fn open(&self, _: &[u8]) -> Result<u64, SourceError> {
            Ok(0)
        }
        fn close(&self, _: u64) {}
        fn title(&self, _: u64) -> String {
            String::new()
        }
        fn content_size(&self, _: u64) -> (f32, f32) {
            (0.0, 0.0)
        }
        fn render_tile(&self, _: u64, _: TileRequest) -> Result<Tile, SourceError> {
            Err(SourceError::UnknownInstance)
        }
        fn hit_test(&self, _: u64, _: f32, _: f32) -> Option<Vec<u8>> {
            None
        }
        fn resolve_anchor(&self, _: u64, _: &[u8]) -> Option<AnchorView> {
            None
        }
        fn current_selection(&self, _: u64) -> Option<Selection> {
            None
        }
        fn search_begin(&self, _: u64, _: &str) -> u64 {
            0
        }
        fn search_poll(&self, _: u64) -> Vec<SearchHit> {
            Vec::new()
        }
        fn search_end(&self, _: u64) {}
        fn config_schema(&self) -> ConfigSchema {
            ConfigSchema::empty()
        }
        fn get_config(&self, _: &str) -> Option<crate::config::ConfigValue> {
            None
        }
        fn set_config(
            &self,
            _: &str,
            _: crate::config::ConfigValue,
        ) -> Result<(), ConfigError> {
            Ok(())
        }
        fn creation_entries(&self) -> Vec<CreationEntry> {
            self.entries.clone()
        }
        fn execute_creation(
            &self,
            entry_id: &str,
            _: Option<&[u8]>,
            _: Option<&str>,
        ) -> Result<ItemCreationResult, CreationError> {
            Err(CreationError::UnknownEntry(entry_id.into()))
        }
    }

    #[derive(Debug)]
    struct StubCanvas {
        kind_id: &'static str,
        label: &'static str,
        caps: Vec<&'static str>,
    }

    impl StubCanvas {
        const fn new(kind_id: &'static str, label: &'static str) -> Self {
            Self {
                kind_id,
                label,
                caps: Vec::new(),
            }
        }
    }

    impl CanvasKind for StubCanvas {
        fn kind_id(&self) -> &str {
            self.kind_id
        }
        fn kind_label(&self) -> &str {
            self.label
        }
        fn capabilities(&self) -> &[&'static str] {
            &self.caps
        }
        fn open(&self, _: &[u8]) -> Result<u64, CanvasError> {
            Ok(0)
        }
        fn close(&self, _: u64) {}
        fn render(&self, _: u64, _: FrameCtxHandle) {}
        fn on_input(&self, _: u64, _: InputEvent) {}
        fn create_bookmark(&self, _: u64, _: WorldPoint) -> Option<BookmarkId> {
            None
        }
        fn list_bookmarks(&self, _: u64) -> Vec<Bookmark> {
            Vec::new()
        }
        fn update_bookmark(&self, _: u64, _: Bookmark) {}
        fn delete_bookmark(&self, _: u64, _: BookmarkId) {}
        fn navigate_to_bookmark(&self, _: u64, _: BookmarkId) {}
        fn serialize_snapshot(&self, _: u64) -> Vec<u8> {
            Vec::new()
        }
        fn apply_remote_op(&self, _: u64, _: &[u8]) {}
        fn config_schema(&self) -> ConfigSchema {
            ConfigSchema::empty()
        }
        fn get_config(&self, _: &str) -> Option<crate::config::ConfigValue> {
            None
        }
        fn set_config(
            &self,
            _: &str,
            _: crate::config::ConfigValue,
        ) -> Result<(), ConfigError> {
            Ok(())
        }
        fn creation_entries(&self) -> Vec<CreationEntry> {
            Vec::new()
        }
        fn execute_creation(
            &self,
            entry_id: &str,
            _: Option<&[u8]>,
            _: Option<&str>,
        ) -> Result<ItemCreationResult, CreationError> {
            Err(CreationError::UnknownEntry(entry_id.into()))
        }
    }

    fn import_entry(id: &str, label: &str) -> CreationEntry {
        CreationEntry {
            id: id.into(),
            label: label.into(),
            icon: "📄".into(),
            category: CreationCategory::Source,
            kind: CreationKind::Import {
                dialog_title: "Choose".into(),
                extensions: vec!["pdf".into()],
                mime_types: Vec::new(),
            },
        }
    }

    #[test]
    fn registers_source_and_canvas_by_kind_id() {
        let mut reg = PluginRegistry::new();
        reg.register_source(Arc::new(StubSource::new("io.notein.pdf", "PDF", &["pdf"])))
            .unwrap();
        reg.register_canvas(Arc::new(StubCanvas::new("io.notein.canvas.stroke", "Stroke")))
            .unwrap();
        assert!(reg.get_source("io.notein.pdf").is_some());
        assert!(reg.get_canvas("io.notein.canvas.stroke").is_some());
        assert_eq!(reg.source_count(), 1);
        assert_eq!(reg.canvas_count(), 1);
    }

    #[test]
    fn duplicate_source_kind_id_rejected() {
        let mut reg = PluginRegistry::new();
        reg.register_source(Arc::new(StubSource::new("io.notein.pdf", "PDF", &["pdf"])))
            .unwrap();
        let err = reg
            .register_source(Arc::new(StubSource::new("io.notein.pdf", "PDF2", &["pdf"])))
            .unwrap_err();
        assert_eq!(
            err,
            RegistryError::DuplicateKindId {
                role: PluginRole::Source,
                kind_id: "io.notein.pdf".into()
            }
        );
    }

    #[test]
    fn duplicate_canvas_kind_id_rejected() {
        let mut reg = PluginRegistry::new();
        reg.register_canvas(Arc::new(StubCanvas::new("io.notein.canvas.stroke", "Stroke")))
            .unwrap();
        assert!(
            reg.register_canvas(Arc::new(StubCanvas::new(
                "io.notein.canvas.stroke",
                "Stroke2"
            )))
            .is_err()
        );
    }

    #[test]
    fn source_and_canvas_share_namespace_safely() {
        let mut reg = PluginRegistry::new();
        reg.register_source(Arc::new(StubSource::new("shared.id", "S", &["bin"])))
            .unwrap();
        // Same kind_id under the canvas role is a separate map — allowed.
        reg.register_canvas(Arc::new(StubCanvas::new("shared.id", "C")))
            .unwrap();
        assert!(reg.get_source("shared.id").is_some());
        assert!(reg.get_canvas("shared.id").is_some());
    }

    #[test]
    fn find_source_for_extension_is_case_insensitive_and_dot_tolerant() {
        let mut reg = PluginRegistry::new();
        reg.register_source(Arc::new(StubSource::new("io.notein.pdf", "PDF", &["pdf"])))
            .unwrap();
        assert!(reg.find_source_for_extension("pdf").is_some());
        assert!(reg.find_source_for_extension("PDF").is_some());
        assert!(reg.find_source_for_extension(".pdf").is_some());
        assert!(reg.find_source_for_extension("epub").is_none());
    }

    #[test]
    fn list_creation_entries_aggregates_across_plugins() {
        let mut reg = PluginRegistry::new();
        reg.register_source(Arc::new(
            StubSource::new("io.notein.pdf", "PDF", &["pdf"])
                .with_entry(import_entry("import-pdf", "Import PDF")),
        ))
        .unwrap();
        reg.register_source(Arc::new(
            StubSource::new("io.notein.epub", "EPUB", &["epub"])
                .with_entry(import_entry("import-epub", "Import EPUB")),
        ))
        .unwrap();

        let entries = reg.list_creation_entries();
        assert_eq!(entries.len(), 2);
        let labels: Vec<_> = entries.iter().map(|e| e.entry.label.as_str()).collect();
        assert!(labels.contains(&"Import PDF"));
        assert!(labels.contains(&"Import EPUB"));
        assert!(entries.iter().all(|e| e.plugin_role == PluginRole::Source));
    }

    #[test]
    fn all_schemas_covers_every_registered_plugin() {
        let mut reg = PluginRegistry::new();
        reg.register_source(Arc::new(StubSource::new("io.notein.pdf", "PDF", &["pdf"])))
            .unwrap();
        reg.register_canvas(Arc::new(StubCanvas::new("io.notein.canvas.stroke", "Stroke")))
            .unwrap();

        let schemas = reg.all_schemas();
        assert_eq!(schemas.len(), 2);
        let ids: Vec<_> = schemas.iter().map(|s| s.plugin_id.as_str()).collect();
        assert!(ids.contains(&"io.notein.pdf"));
        assert!(ids.contains(&"io.notein.canvas.stroke"));
    }
}
