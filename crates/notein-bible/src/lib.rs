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
//! Built-in registration path:
//!
//! ```ignore
//! use notein_bible::install;
//! let db = notein_bible::db::Db::open("~/.local/share/notinplus/bible.db")?;
//! install(&mut registry, db)?;
//! ```
//!
//! Dynamic registration via `notein_plugin::PluginBundle` is also
//! exposed below; the host decides which path to use.

use std::sync::Arc;

use notein_sdk::{PluginRegistry, Registrations, RegistryError};

pub mod anchor;
pub mod canvas;
pub mod db;
pub mod fonts;
pub mod import;
#[cfg(feature = "importer")]
pub mod install_background;
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
    /// # Panics
    /// If the in-memory SQLite instance cannot be created — a condition
    /// that signals a fundamentally broken rusqlite link, not a
    /// recoverable error.
    #[must_use]
    pub fn new() -> Self {
        let db = Db::in_memory().expect("in-memory sqlite should always open");
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
/// The dynamic plugin instance always starts with an in-memory DB —
/// no persisted translations — because the dynamic loader does not
/// hand the plugin a filesystem path. Install flow (background
/// downloader) still works through `set_config` once the UI
/// surfaces it, writing fetched Bibles into that in-memory DB until
/// the host grows a persist-dir handshake.
#[unsafe(no_mangle)]
pub fn notein_plugin_entry() -> Box<dyn notein_sdk::PluginBundle> {
    Box::new(BibleBundle::new())
}
