//! Plugin surface for notinplus.
//!
//! Defines the two trait kinds the host dispatches to:
//!
//! - [`Source`] — read-mostly content viewers (PDF, EPUB, Bible, custom).
//!   The host hands the plugin bytes to open, asks for render tiles, and
//!   consults it for anchor resolution + full-text search.
//! - [`CanvasKind`] — owns the full pane. Renders, consumes input, holds
//!   bookmarks, and emits CRDT ops. Stroke canvas, mind map, code editor,
//!   whatever third parties build.
//!
//! The crate also defines the data-driven surfaces plugins declare to
//! the shell: [`ConfigSchema`] for the generic settings panel and
//! [`CreationEntry`] for the home FAB `+` menu.
//!
//! Shared types ([`Bookmark`], [`BookmarkId`], [`InputEvent`],
//! `ConfigSchema`, …) cross the trait boundary as typed Rust values —
//! no bincode encode / decode on the hot path. Only payloads whose
//! shape is private to the plugin (CRDT op bytes, snapshots, stored
//! item bytes, source anchors) stay opaque `Vec<u8>` / `&[u8]`; the
//! shell is pure transport for those.
//!
//! Phase 3 adds a stabby wrapper around this trait for dynamic
//! plugins loaded via dlopen — the wrapper serializes the typed
//! values at the FFI boundary only. In-process built-in plugins
//! (Phase 2.3–2.7) consume the trait directly through `Arc<dyn
//! CanvasKind>` with zero codec overhead.

pub mod bookmark;
pub mod canvas;
pub mod config;
pub mod creation;
pub mod dynamic;
pub mod error;
pub mod ids;
pub mod manifest;
pub mod registry;
pub mod source;

pub use bookmark::{Bookmark, Rgba, SourceRef, StrokeAnchor, TimestampMs, WorldPoint};
pub use canvas::{CanvasKind, FrameCtxHandle, InputEvent, PointerKind};
pub use config::{ConfigEnumOption, ConfigField, ConfigFieldKind, ConfigSchema, ConfigValue};
pub use creation::{CreationCategory, CreationEntry, CreationKind, ItemCreationResult};
pub use dynamic::{
    ENTRY_SYMBOL, LoadedPlugin, LoaderError, PluginBundle, PluginEntryFn, Registrations,
    SDK_ABI_VERSION, load_from_manifest,
};
pub use error::{CanvasError, ConfigError, CreationError, SourceError};
pub use ids::{BookmarkId, StrokeId};
pub use manifest::{ManifestError, ManifestRequirement, PluginManifest};
pub use registry::{
    InstalledPlugin, PluginOrigin, PluginRegistry, PluginRole, RegistryCreationEntry,
    RegistryError, RegistrySchemaEntry,
};
pub use source::{AnchorView, SearchHit, Selection, Source, Tile, TileRequest};
