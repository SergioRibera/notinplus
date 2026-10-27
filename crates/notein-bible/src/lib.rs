//! Bible plugin: a `SQLite` + FTS5 store of installed translations.
//!
//! [`BibleKind`] is the document kind: it reads chapters, resolves
//! verse anchors and searches the installed text. [`Db`] is the shared
//! store; [`installer`] downloads translations into it in the
//! background.

pub mod anchor;
pub mod canon;
pub mod db;
pub mod fonts;
pub mod import;
#[cfg(feature = "importer")]
pub mod install_background;
#[cfg(feature = "importer")]
pub mod installer;
pub mod kind;
#[cfg(feature = "ui")]
pub mod plugin;
pub mod plugin_config;
pub mod reference;
pub mod resolver;
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
pub use db::Db;
pub use kind::{BibleContent, BibleKind};
#[cfg(feature = "ui")]
pub use plugin::{BibleFace, BiblePlugin};
pub use plugin_config::{SharedConfig, new_shared};
