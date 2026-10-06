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
//! Bookmark payloads cross the trait boundary as opaque bincode blobs
//! (`Vec<u8>` arguments / returns) — only identifiers and primitives
//! appear in the ABI surface. This keeps the layer thin and avoids
//! pinning the bookmark model as a stable wire type.


pub mod config;
pub mod creation;
pub mod error;
pub mod ids;
pub mod source;
pub mod canvas;

pub use canvas::{CanvasKind, FrameCtxHandle, InputEvent, PointerKind};
pub use config::{ConfigEnumOption, ConfigField, ConfigFieldKind, ConfigSchema, ConfigValue};
pub use creation::{CreationCategory, CreationEntry, CreationKind, ItemCreationResult};
pub use error::{CanvasError, ConfigError, CreationError, SourceError};
pub use ids::{BookmarkId, WorldPoint};
pub use source::{AnchorView, SearchHit, Selection, Source, Tile, TileRequest};
