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
//! (`Vec<u8>` arguments / returns) encoded from the shared
//! [`Bookmark`] / [`bookmark::SourceRef`] types defined here — only
//! identifiers and primitives appear in the ABI surface. Plugin
//! authors should pull `notein-sdk` and use its codec helpers rather
//! than hand-rolling the bincode calls.

pub mod bookmark;
pub mod canvas;
pub mod config;
pub mod creation;
pub mod error;
pub mod ids;
pub mod source;

pub use bookmark::{Bookmark, Rgba, SourceRef, StrokeAnchor, TimestampMs, WorldPoint};
pub use canvas::{CanvasKind, FrameCtxHandle, InputEvent, PointerKind};
pub use config::{ConfigEnumOption, ConfigField, ConfigFieldKind, ConfigSchema, ConfigValue};
pub use creation::{CreationCategory, CreationEntry, CreationKind, ItemCreationResult};
pub use error::{CanvasError, ConfigError, CreationError, SourceError};
pub use ids::{BookmarkId, StrokeId};
pub use source::{AnchorView, SearchHit, Selection, Source, Tile, TileRequest};
