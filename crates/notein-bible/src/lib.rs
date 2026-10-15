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
pub mod import;
pub mod plugin_config;
pub mod reference;
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
    let canvas = Arc::new(BibleCanvas::with_selection(
        db.clone(),
        config.clone(),
        selection.clone(),
    ));
    let source_plugin = Arc::new(BibleSource::with_selection(db, config, selection));
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
