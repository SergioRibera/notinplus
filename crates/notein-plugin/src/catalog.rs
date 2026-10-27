//! The shell's catalog of installed plugins.

use std::path::PathBuf;
use std::sync::Arc;

use crate::create::CreationEntry;
use crate::erased::ErasedKind;
use crate::face::ErasedFace;
use crate::info::PluginInfo;
use crate::kind::{Hit, SearchQuery};
use crate::manifest::PluginManifest;
use crate::plugin::{Font, Plugin, PluginSettings, Registrar};
use crate::services::Host;

/// Where a plugin came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Ships with Notein.
    Builtin,
    /// Installed by the user from a file.
    ThirdParty {
        manifest: Box<PluginManifest>,
        library: PathBuf,
    },
}

/// Whether an installed plugin runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginState {
    Active,
    Disabled,
}

/// One installed plugin.
#[derive(Clone)]
pub struct Installed {
    pub info: PluginInfo,
    pub origin: Origin,
    pub state: PluginState,
    pub kinds: Vec<Arc<dyn ErasedKind>>,
    pub faces: Vec<Arc<dyn ErasedFace>>,
    pub settings: Option<Arc<dyn PluginSettings>>,
    plugin: Arc<dyn Plugin>,
}

impl std::fmt::Debug for Installed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Installed")
            .field("info", &self.info)
            .field("origin", &self.origin)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl Installed {
    /// Hand `host` to this plugin (installed while the app runs).
    pub fn install_host(&self, host: &Host) {
        self.plugin.install(host);
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state == PluginState::Active
    }
}

/// A plugin that could not be loaded, kept so Settings can say why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failed {
    pub name: String,
    pub reason: String,
}

/// A create-menu entry with its owners.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogEntry {
    pub plugin: String,
    pub kind: String,
    pub entry: CreationEntry,
}

/// Search hits of one kind.
#[derive(Clone, Debug, PartialEq)]
pub struct KindHits {
    pub kind: String,
    pub hits: Vec<Hit<Vec<u8>>>,
}

/// Two plugins claimed the same id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogError {
    DuplicatePlugin(String),
    DuplicateKind(String),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicatePlugin(id) => write!(f, "ya hay un plugin con id {id}"),
            Self::DuplicateKind(id) => write!(f, "ya hay un tipo de documento con id {id}"),
        }
    }
}

impl std::error::Error for CatalogError {}

/// Every plugin the shell knows.
#[derive(Default)]
pub struct PluginCatalog {
    plugins: Vec<Installed>,
    failed: Vec<Failed>,
    /// Shared libraries of third-party plugins. Declared last so they
    /// drop after every trait object created from them.
    libraries: Vec<libloading::Library>,
}

impl std::fmt::Debug for PluginCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginCatalog")
            .field("plugins", &self.plugins)
            .field("failed", &self.failed)
            .finish_non_exhaustive()
    }
}

impl PluginCatalog {
    /// Register `plugin`.
    ///
    /// # Errors
    /// [`CatalogError`] when its id or one of its kind ids is taken.
    pub fn add(&mut self, plugin: Arc<dyn Plugin>, origin: Origin) -> Result<(), CatalogError> {
        let info = plugin.info();
        if self.plugins.iter().any(|p| p.info.id == info.id) {
            return Err(CatalogError::DuplicatePlugin(info.id.into_owned()));
        }
        let mut registrar = Registrar::default();
        plugin.register(&mut registrar);
        for kind in &registrar.kinds {
            let id = kind.info().id;
            if self.kind(&id).is_some() {
                return Err(CatalogError::DuplicateKind(id.into_owned()));
            }
        }
        self.plugins.push(Installed {
            info,
            origin,
            state: PluginState::Active,
            kinds: registrar.kinds,
            faces: registrar.faces,
            settings: registrar.settings,
            plugin,
        });
        Ok(())
    }

    /// Keep a third-party library alive for as long as the catalog.
    pub fn keep_library(&mut self, library: libloading::Library) {
        self.libraries.push(library);
    }

    pub fn record_failure(&mut self, failed: Failed) {
        self.failed.push(failed);
    }

    /// Hand `host` to every plugin.
    pub fn install_host(&self, host: &Host) {
        for plugin in &self.plugins {
            plugin.plugin.install(host);
        }
    }

    #[must_use]
    pub fn plugins(&self) -> &[Installed] {
        &self.plugins
    }

    #[must_use]
    pub fn failed(&self) -> &[Failed] {
        &self.failed
    }

    #[must_use]
    pub fn plugin_by_name(&self, name: &str) -> Option<&Installed> {
        self.plugins.iter().find(|p| p.info.name == name)
    }

    #[must_use]
    pub fn plugin(&self, id: &str) -> Option<&Installed> {
        self.plugins.iter().find(|p| p.info.id == id)
    }

    /// Turn a plugin on or off. Its documents stay; they open again
    /// when it comes back.
    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        if let Some(plugin) = self.plugins.iter_mut().find(|p| p.info.id == id) {
            plugin.state = if enabled {
                PluginState::Active
            } else {
                PluginState::Disabled
            };
        }
    }

    fn active(&self) -> impl Iterator<Item = &Installed> {
        self.plugins.iter().filter(|p| p.is_active())
    }

    /// The active kind with id `id`.
    #[must_use]
    pub fn kind(&self, id: &str) -> Option<Arc<dyn ErasedKind>> {
        self.active()
            .flat_map(|p| p.kinds.iter())
            .find(|k| k.info().id == id)
            .cloned()
    }

    /// Every active kind.
    #[must_use]
    pub fn kinds(&self) -> Vec<Arc<dyn ErasedKind>> {
        self.active()
            .flat_map(|p| p.kinds.iter().cloned())
            .collect()
    }

    /// The face for documents of kind `id`.
    #[must_use]
    pub fn face(&self, id: &str) -> Option<Arc<dyn ErasedFace>> {
        self.active()
            .flat_map(|p| p.faces.iter())
            .find(|f| f.kind_id() == id)
            .cloned()
    }

    /// The kind notebooks are made of ([`KindInfo::notebook`]).
    ///
    /// [`KindInfo::notebook`]: crate::kind::KindInfo::notebook
    #[must_use]
    pub fn notebook_kind(&self) -> Option<Arc<dyn ErasedKind>> {
        self.active()
            .flat_map(|p| p.kinds.iter())
            .find(|k| k.info().notebook)
            .cloned()
    }

    /// The typefaces of every plugin, on or off: text already laid out
    /// with them keeps its face.
    #[must_use]
    pub fn fonts(&self) -> Vec<Font> {
        self.plugins.iter().flat_map(|p| p.plugin.fonts()).collect()
    }

    /// Plugin that owns kind `id`.
    #[must_use]
    pub fn owner_of(&self, kind: &str) -> Option<&Installed> {
        self.plugins
            .iter()
            .find(|p| p.kinds.iter().any(|k| k.info().id == kind))
    }

    /// Every create-menu entry of every active kind.
    #[must_use]
    pub fn entries(&self) -> Vec<CatalogEntry> {
        self.active()
            .flat_map(|p| {
                p.kinds.iter().flat_map(move |kind| {
                    let kind_id = kind.info().id.into_owned();
                    kind.creation_entries()
                        .into_iter()
                        .map(move |entry| CatalogEntry {
                            plugin: p.info.id.to_string(),
                            kind: kind_id.clone(),
                            entry,
                        })
                })
            })
            .collect()
    }

    /// Search every active kind.
    #[must_use]
    pub fn search(&self, query: &SearchQuery) -> Vec<KindHits> {
        self.kinds()
            .into_iter()
            .map(|kind| KindHits {
                kind: kind.info().id.into_owned(),
                hits: kind.search(query),
            })
            .filter(|k| !k.hits.is_empty())
            .collect()
    }
}
