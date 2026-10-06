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
use std::path::PathBuf;
use std::sync::Arc;

use crate::canvas::CanvasKind;
use crate::config::ConfigSchema;
use crate::creation::CreationEntry;
use crate::dynamic::{LoadedPlugin, Registrations};
use crate::manifest::PluginManifest;
use crate::source::Source;

/// Which trait a plugin implements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PluginRole {
    Source,
    Canvas,
}

/// Error returned by registration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryError {
    /// A plugin with the same `kind_id` is already registered under
    /// the same role.
    DuplicateKindId { role: PluginRole, kind_id: String },
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

/// Where a registered plugin came from. Missing from the origin map =
/// built-in (default).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginOrigin {
    /// Compiled into the host binary.
    Builtin,
    /// Loaded at runtime from a shared library on disk. Boxed to
    /// keep the enum size close to [`PluginOrigin::Builtin`]'s.
    Dynamic(Box<DynamicOrigin>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicOrigin {
    pub manifest_path: PathBuf,
    pub library_path: PathBuf,
    pub manifest: PluginManifest,
}

/// One row of [`PluginRegistry::installed_plugins`] — the Phase 3
/// settings panel renders one of these per plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledPlugin {
    pub plugin_role: PluginRole,
    pub plugin_id: String,
    pub plugin_label: String,
    pub origin: PluginOrigin,
    pub enabled: bool,
}

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
///
/// Each plugin carries a per-process enabled flag. Absent from the map
/// means enabled; the shell persists only the user-toggled disabled
/// entries so a brand-new install starts with everything on.
#[derive(Default)]
pub struct PluginRegistry {
    sources: HashMap<String, Arc<dyn Source>>,
    canvases: HashMap<String, Arc<dyn CanvasKind>>,
    disabled: HashMap<String, ()>,
    /// Non-default origins (built-ins stay absent from this map).
    origins: HashMap<String, PluginOrigin>,
    /// Dynamic plugin containers kept alive for Drop ordering: each
    /// entry owns its `libloading::Library`, which must outlive every
    /// `Arc<dyn Trait>` registered from it.
    dynamic: Vec<LoadedPlugin>,
}

impl std::fmt::Debug for PluginRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginRegistry")
            .field("sources", &self.sources.keys().collect::<Vec<_>>())
            .field("canvases", &self.canvases.keys().collect::<Vec<_>>())
            .field("disabled", &self.disabled.keys().collect::<Vec<_>>())
            .field("dynamic", &self.dynamic.len())
            .finish_non_exhaustive()
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

    /// Whether `plugin_id` is currently enabled. Unknown ids answer
    /// `true` — the shell may ask about a plugin before it registers.
    #[must_use]
    pub fn is_enabled(&self, plugin_id: &str) -> bool {
        !self.disabled.contains_key(plugin_id)
    }

    /// Flip the enabled bit for `plugin_id`. The plugin stays
    /// registered either way; disabled plugins are hidden from
    /// `find_source_for_extension` and the home FAB's creation menu.
    pub fn set_enabled(&mut self, plugin_id: &str, enabled: bool) {
        if enabled {
            self.disabled.remove(plugin_id);
        } else {
            self.disabled.insert(plugin_id.to_owned(), ());
        }
    }

    /// Plugin ids the user has explicitly disabled. Used by the
    /// settings store to persist only the deviations from the default.
    pub fn disabled_ids(&self) -> impl Iterator<Item = &str> {
        self.disabled.keys().map(String::as_str)
    }

    /// First enabled source plugin whose `file_extensions` include
    /// `ext` (case-insensitive, leading dot stripped). `None` = no
    /// enabled plugin claims the extension.
    #[must_use]
    pub fn find_source_for_extension(&self, ext: &str) -> Option<&Arc<dyn Source>> {
        let needle = ext.trim_start_matches('.').to_ascii_lowercase();
        self.sources.values().find(|p| {
            self.is_enabled(p.kind_id())
                && p.file_extensions()
                    .iter()
                    .any(|e| e.eq_ignore_ascii_case(&needle))
        })
    }

    /// Creation entries from every **enabled** plugin, each wrapped
    /// with its owning plugin's id + label + role so the home FAB can
    /// route the pick without a second lookup. Disabled plugins are
    /// filtered — a user who turned a plugin off should not see its
    /// `+` menu entries.
    #[must_use]
    pub fn list_creation_entries(&self) -> Vec<RegistryCreationEntry> {
        let mut out = Vec::new();
        for plugin in self.sources.values() {
            let plugin_id = plugin.kind_id().to_owned();
            if !self.is_enabled(&plugin_id) {
                continue;
            }
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
            if !self.is_enabled(&plugin_id) {
                continue;
            }
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

    /// Register every trait object from a [`LoadedPlugin`]. All
    /// registrations share the same [`PluginOrigin::Dynamic`] origin
    /// so the settings panel can tag them with the manifest they came
    /// from. The `LoadedPlugin` (and the `libloading::Library` it
    /// owns) is retained for the lifetime of the registry — see
    /// [`crate::dynamic::LoadedPlugin`] for the Drop-order
    /// invariant.
    ///
    /// On a duplicate `kind_id`, the whole load is rolled back: any
    /// trait objects already inserted from this bundle are removed
    /// and the library is dropped. The caller sees a single
    /// [`RegistryError::DuplicateKindId`] without side effects.
    pub fn register_dynamic(&mut self, loaded: LoadedPlugin) -> Result<(), RegistryError> {
        let origin = PluginOrigin::Dynamic(Box::new(DynamicOrigin {
            manifest_path: loaded.manifest_path.clone(),
            library_path: loaded.library_path.clone(),
            manifest: loaded.manifest.clone(),
        }));
        self.register_with_origin(&loaded.registrations, &origin)?;
        self.dynamic.push(loaded);
        Ok(())
    }

    /// Insert every trait object in `regs` with `origin` tagged on
    /// each. Rolls back on the first duplicate so partial state never
    /// leaks. Shared between [`register_dynamic`] and the test seam
    /// below.
    fn register_with_origin(
        &mut self,
        regs: &Registrations,
        origin: &PluginOrigin,
    ) -> Result<(), RegistryError> {
        let mut inserted_sources = Vec::new();
        let mut inserted_canvases = Vec::new();
        for plugin in &regs.canvases {
            let kind_id = plugin.kind_id().to_owned();
            if self.canvases.contains_key(&kind_id) {
                self.rollback(&inserted_sources, &inserted_canvases);
                return Err(RegistryError::DuplicateKindId {
                    role: PluginRole::Canvas,
                    kind_id,
                });
            }
            self.canvases.insert(kind_id.clone(), Arc::clone(plugin));
            self.origins.insert(kind_id.clone(), origin.clone());
            inserted_canvases.push(kind_id);
        }
        for plugin in &regs.sources {
            let kind_id = plugin.kind_id().to_owned();
            if self.sources.contains_key(&kind_id) {
                self.rollback(&inserted_sources, &inserted_canvases);
                return Err(RegistryError::DuplicateKindId {
                    role: PluginRole::Source,
                    kind_id,
                });
            }
            self.sources.insert(kind_id.clone(), Arc::clone(plugin));
            self.origins.insert(kind_id.clone(), origin.clone());
            inserted_sources.push(kind_id);
        }
        Ok(())
    }

    /// Test-only seam: register trait objects as if loaded from a
    /// `.so` without actually opening a library. The caller supplies
    /// the manifest + paths so [`PluginOrigin::Dynamic`] carries real
    /// data for the settings panel assertions.
    #[cfg(test)]
    pub(crate) fn register_dynamic_for_test(
        &mut self,
        regs: &Registrations,
        manifest: PluginManifest,
        manifest_path: PathBuf,
        library_path: PathBuf,
    ) -> Result<(), RegistryError> {
        self.register_with_origin(
            regs,
            &PluginOrigin::Dynamic(Box::new(DynamicOrigin {
                manifest_path,
                library_path,
                manifest,
            })),
        )
    }

    fn rollback(&mut self, sources: &[String], canvases: &[String]) {
        for id in sources {
            self.sources.remove(id);
            self.origins.remove(id);
        }
        for id in canvases {
            self.canvases.remove(id);
            self.origins.remove(id);
        }
    }

    /// Origin of `plugin_id`. Returns `PluginOrigin::Builtin` for any
    /// id not explicitly tagged — the default for built-in
    /// registrations.
    #[must_use]
    pub fn origin(&self, plugin_id: &str) -> PluginOrigin {
        self.origins
            .get(plugin_id)
            .cloned()
            .unwrap_or(PluginOrigin::Builtin)
    }

    /// Every registered plugin, with its role, label, origin and
    /// enabled flag. Powers the settings panel's "installed plugins"
    /// section. Sorted by `(role, plugin_id)` for stable UI order.
    #[must_use]
    pub fn installed_plugins(&self) -> Vec<InstalledPlugin> {
        let mut out = Vec::with_capacity(self.sources.len() + self.canvases.len());
        for plugin in self.sources.values() {
            let plugin_id = plugin.kind_id().to_owned();
            let enabled = self.is_enabled(&plugin_id);
            out.push(InstalledPlugin {
                plugin_role: PluginRole::Source,
                plugin_label: plugin.kind_label().to_owned(),
                origin: self.origin(&plugin_id),
                plugin_id,
                enabled,
            });
        }
        for plugin in self.canvases.values() {
            let plugin_id = plugin.kind_id().to_owned();
            let enabled = self.is_enabled(&plugin_id);
            out.push(InstalledPlugin {
                plugin_role: PluginRole::Canvas,
                plugin_label: plugin.kind_label().to_owned(),
                origin: self.origin(&plugin_id),
                plugin_id,
                enabled,
            });
        }
        out.sort_by(|a, b| {
            a.plugin_role
                .cmp(&b.plugin_role)
                .then_with(|| a.plugin_id.cmp(&b.plugin_id))
        });
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
        fn new(kind_id: &'static str, label: &'static str, extensions: &[&'static str]) -> Self {
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
        fn set_config(&self, _: &str, _: crate::config::ConfigValue) -> Result<(), ConfigError> {
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
        fn set_config(&self, _: &str, _: crate::config::ConfigValue) -> Result<(), ConfigError> {
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
        reg.register_canvas(Arc::new(StubCanvas::new(
            "io.notein.canvas.stroke",
            "Stroke",
        )))
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
        reg.register_canvas(Arc::new(StubCanvas::new(
            "io.notein.canvas.stroke",
            "Stroke",
        )))
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
    fn disabled_plugin_is_filtered_from_extension_lookup_and_creation_entries() {
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

        assert!(reg.is_enabled("io.notein.pdf"));
        reg.set_enabled("io.notein.pdf", false);
        assert!(!reg.is_enabled("io.notein.pdf"));

        assert!(reg.find_source_for_extension("pdf").is_none());
        assert!(reg.find_source_for_extension("epub").is_some());

        let entries = reg.list_creation_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry.label, "Import EPUB");

        // `all_schemas` still shows the disabled plugin so the user can
        // find it in the settings panel and flip it back on.
        assert_eq!(reg.all_schemas().len(), 2);

        // Disabled set exposes just the user-flipped ids.
        let dis: Vec<_> = reg.disabled_ids().collect();
        assert_eq!(dis, vec!["io.notein.pdf"]);

        reg.set_enabled("io.notein.pdf", true);
        assert!(reg.is_enabled("io.notein.pdf"));
        assert!(reg.find_source_for_extension("pdf").is_some());
    }

    #[test]
    fn all_schemas_covers_every_registered_plugin() {
        let mut reg = PluginRegistry::new();
        reg.register_source(Arc::new(StubSource::new("io.notein.pdf", "PDF", &["pdf"])))
            .unwrap();
        reg.register_canvas(Arc::new(StubCanvas::new(
            "io.notein.canvas.stroke",
            "Stroke",
        )))
        .unwrap();

        let schemas = reg.all_schemas();
        assert_eq!(schemas.len(), 2);
        let ids: Vec<_> = schemas.iter().map(|s| s.plugin_id.as_str()).collect();
        assert!(ids.contains(&"io.notein.pdf"));
        assert!(ids.contains(&"io.notein.canvas.stroke"));
    }

    fn dummy_manifest(id: &str) -> PluginManifest {
        PluginManifest {
            id: id.to_owned(),
            name: id.to_owned(),
            version: "1.0.0".into(),
            author: None,
            library: Some(format!("{id}.so")),
            provides: Vec::new(),
            requires: Vec::new(),
        }
    }

    #[test]
    fn installed_plugins_tags_builtins_and_dynamic() {
        let mut reg = PluginRegistry::new();
        reg.register_source(Arc::new(StubSource::new("io.notein.pdf", "PDF", &["pdf"])))
            .unwrap();

        let regs = Registrations {
            canvases: vec![Arc::new(StubCanvas::new("org.user.mindmap", "Mind Map"))],
            sources: Vec::new(),
        };
        reg.register_dynamic_for_test(
            &regs,
            dummy_manifest("org.user.mindmap"),
            PathBuf::from("/opt/plugins/mindmap/plugin.ntp.kdl"),
            PathBuf::from("/opt/plugins/mindmap/libmindmap.so"),
        )
        .unwrap();

        let installed = reg.installed_plugins();
        assert_eq!(installed.len(), 2);

        let pdf = installed
            .iter()
            .find(|p| p.plugin_id == "io.notein.pdf")
            .unwrap();
        assert_eq!(pdf.origin, PluginOrigin::Builtin);
        assert!(pdf.enabled);

        let mindmap = installed
            .iter()
            .find(|p| p.plugin_id == "org.user.mindmap")
            .unwrap();
        match &mindmap.origin {
            PluginOrigin::Dynamic(d) => {
                assert_eq!(
                    d.library_path,
                    PathBuf::from("/opt/plugins/mindmap/libmindmap.so")
                );
                assert_eq!(d.manifest.id, "org.user.mindmap");
            }
            PluginOrigin::Builtin => panic!("expected Dynamic origin"),
        }
    }

    #[test]
    fn dynamic_duplicate_rolls_back_whole_bundle() {
        let mut reg = PluginRegistry::new();
        reg.register_source(Arc::new(StubSource::new("io.notein.pdf", "PDF", &["pdf"])))
            .unwrap();

        // Bundle registers one brand-new canvas, then collides on a
        // source already present. The whole bundle must roll back —
        // the brand-new canvas must not stay registered.
        let regs = Registrations {
            canvases: vec![Arc::new(StubCanvas::new("org.user.good", "Good"))],
            sources: vec![Arc::new(StubSource::new("io.notein.pdf", "PDF Dupe", &["pdf"]))],
        };
        let err = reg
            .register_dynamic_for_test(
                &regs,
                dummy_manifest("org.user.bundle"),
                PathBuf::from("/opt/plugins/bundle/plugin.ntp.kdl"),
                PathBuf::from("/opt/plugins/bundle/libbundle.so"),
            )
            .unwrap_err();
        assert!(matches!(
            err,
            RegistryError::DuplicateKindId {
                role: PluginRole::Source,
                ..
            }
        ));
        assert!(reg.get_canvas("org.user.good").is_none());
        assert_eq!(reg.origin("org.user.good"), PluginOrigin::Builtin);
    }
}
