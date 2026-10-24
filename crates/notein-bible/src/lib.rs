//! Bible plugin — SQLite + FTS5 backed source + canvas kind.
//!
//! The crate exposes two trait objects to the host:
//!
//! - [`canvas::BibleCanvas`] — reading surface. Takes the full pane,
//!   renders a flowing paragraph with inline verse numbers + section
//!   headers + highlight bookmarks. Declares the `"bookmarks"`
//!   capability only; no stroke / layer tools.
//! - [`source::BibleSource`] — anchor resolver. Lets other canvases
//!   create `SourceRef::Plugin { plugin_id = "io.notein.bible" }`
//!   references, renders chips for them, and exposes FTS5 search.
//!
//! Both share the same `Db` handle so an edit done through either side
//! shows up on the other without a sync hop.
//!
//! Built-in registration path (uses the shared notinplus data dir):
//!
//! ```ignore
//! notein_bible::install_default(&mut registry)?;
//! ```
//!
//! …or supply your own path:
//!
//! ```ignore
//! let db = notein_bible::db::Db::open("/custom/path/bible.db")?;
//! notein_bible::install(&mut registry, db)?;
//! ```
//!
//! Dynamic registration via `notein_plugin::PluginBundle` is also
//! exposed below; the host decides which path to use.

use std::sync::Arc;

use notein_sdk::{PluginRegistry, Registrations, RegistryError};

pub mod anchor;
pub mod canon;
pub mod canvas;
pub mod db;
pub mod fonts;
pub mod import;
pub mod kind;
#[cfg(feature = "importer")]
pub mod install_background;
#[cfg(feature = "importer")]
pub mod installer;
pub mod plugin_config;
pub mod reference;
pub mod resolver;
pub mod source;
pub mod state;
#[cfg(feature = "ui")]
pub mod ui;
#[cfg(not(feature = "ui"))]
pub mod ui {
    //! Chapter planning is kept available regardless of the `ui`
    //! feature flag so headless callers (importers, tests) can still
    //! build a `ChapterPlan` without pulling in Freya.
    pub mod chapter;
}

pub use anchor::BibleAnchor;
pub use canvas::BibleCanvas;
pub use kind::{BibleContent, BibleKind};
pub use db::Db;
pub use plugin_config::{SharedConfig, new_shared};
pub use source::BibleSource;

/// Register both plugin trait objects with the shell's registry.
/// Called by the host at startup for the built-in path. A fresh shared
/// config is created unless one is handed in — call [`install_with`]
/// to reuse an already-populated config map.
pub fn install(registry: &mut PluginRegistry, db: Db) -> Result<(), RegistryError> {
    install_with(registry, db, new_shared())
}

/// Convenience wrapper that opens the DB at the shared notinplus data
/// location ([`Db::default_path`]) and calls [`install`]. Hosts that
/// want to control the DB path should call [`install`] directly.
pub fn install_default(registry: &mut PluginRegistry) -> Result<(), InstallError> {
    install_default_with_handle(registry).map(|_| ())
}

/// Same as [`install_default`] but returns the concrete [`BibleCanvas`]
/// handle so the host can call plugin-specific methods (fetching the DB
/// for a custom UI render path, polling install progress, etc.) that
/// the generic `CanvasKind` trait object does not expose.
pub fn install_default_with_handle(
    registry: &mut PluginRegistry,
) -> Result<Arc<BibleCanvas>, InstallError> {
    let db = Db::open_default().map_err(InstallError::Db)?;
    let config = new_shared();
    let selection = source::new_shared_selection();
    #[cfg(feature = "importer")]
    let progress = install_background::new_shared_progress();

    let canvas = BibleCanvas::with_selection(db.clone(), config.clone(), selection.clone());
    #[cfg(feature = "importer")]
    let canvas = canvas.with_install_progress(progress.clone());
    let canvas = Arc::new(canvas);

    let source_plugin = BibleSource::with_selection(db, config, selection);
    #[cfg(feature = "importer")]
    let source_plugin = source_plugin.with_install_progress(progress);
    let source_plugin = Arc::new(source_plugin);

    registry
        .register_canvas(canvas.clone())
        .map_err(InstallError::Registry)?;
    registry
        .register_source(source_plugin)
        .map_err(InstallError::Registry)?;
    Ok(canvas)
}

#[derive(Debug)]
pub enum InstallError {
    Db(db::DbError),
    Registry(RegistryError),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(e) => write!(f, "bible install: db: {e}"),
            Self::Registry(e) => write!(f, "bible install: registry: {e:?}"),
        }
    }
}

impl std::error::Error for InstallError {}

pub fn install_with(
    registry: &mut PluginRegistry,
    db: Db,
    config: SharedConfig,
) -> Result<(), RegistryError> {
    let selection = source::new_shared_selection();
    #[cfg(feature = "importer")]
    let progress = install_background::new_shared_progress();

    let canvas = BibleCanvas::with_selection(db.clone(), config.clone(), selection.clone());
    #[cfg(feature = "importer")]
    let canvas = canvas.with_install_progress(progress.clone());
    let canvas = Arc::new(canvas);

    let source_plugin = BibleSource::with_selection(db, config, selection);
    #[cfg(feature = "importer")]
    let source_plugin = source_plugin.with_install_progress(progress);
    let source_plugin = Arc::new(source_plugin);

    registry.register_canvas(canvas)?;
    registry.register_source(source_plugin)?;
    Ok(())
}

/// `PluginBundle` entry used by the dynamic loader. Returns both trait
/// objects populated with an **in-memory** DB — the dynamic path is
/// smoke-test only until the host negotiates a shared DB path via a
/// future config channel.
#[derive(Debug)]
pub struct BibleBundle {
    canvas: Arc<BibleCanvas>,
    source: Arc<BibleSource>,
}

impl BibleBundle {
    /// Open the shared data-dir DB when possible, falling back to an
    /// in-memory instance on IO failure so the dynamic plugin entry can
    /// never panic the host. First-run `open_default` creates the
    /// directory tree, so the fallback almost always indicates a
    /// permission problem worth logging.
    #[must_use]
    pub fn new() -> Self {
        let db = Db::open_default().unwrap_or_else(|e| {
            log::warn!(
                "bible: open_default failed ({e}); falling back to in-memory DB — \
                 any downloaded translations will not persist across restarts"
            );
            Db::in_memory().expect("in-memory sqlite should always open")
        });
        let config = new_shared();
        let selection = source::new_shared_selection();
        Self {
            canvas: Arc::new(BibleCanvas::with_selection(db.clone(), config.clone(), selection.clone())),
            source: Arc::new(BibleSource::with_selection(db, config, selection)),
        }
    }
}

impl Default for BibleBundle {
    fn default() -> Self {
        Self::new()
    }
}

impl notein_sdk::PluginBundle for BibleBundle {
    fn sdk_abi_version(&self) -> u32 {
        notein_sdk::SDK_ABI_VERSION
    }

    fn take_registrations(self: Box<Self>) -> Registrations {
        Registrations {
            canvases: vec![self.canvas as Arc<dyn notein_sdk::CanvasKind>],
            sources: vec![self.source as Arc<dyn notein_sdk::Source>],
        }
    }
}

/// Dynamic plugin entry point. The loader looks for this exact symbol
/// (`notein_plugin_entry`) via `libloading` after `dlopen`ing the
/// shared library produced by the `cdylib` crate-type above. The
/// returned bundle lives on the heap and is drained once by the host.
///
/// Compatibility matrix:
///
/// - Host and plugin **must** be built with the same rustc toolchain
///   and the same `notein-plugin` version — the signature uses the
///   Rust ABI, not `extern "C"`.
/// - [`notein_sdk::PluginBundle::sdk_abi_version`] guards against
///   trait-shape drift at load time; the host rejects the load before
///   touching any trait objects when the version does not match.
///
/// The dynamic plugin instance opens the DB under
/// `istmo::path::data_dir()/notinplus/bible/bible.db` so persisted
/// translations survive across restarts without any extra handshake
/// — the plugin and the host agree on the data root through
/// `istmo-core`. Open failures silently fall back to an in-memory DB
/// (logged at WARN) so the host never crashes on a locked disk.
#[unsafe(no_mangle)]
pub fn notein_plugin_entry() -> Box<dyn notein_sdk::PluginBundle> {
    Box::new(BibleBundle::new())
}
