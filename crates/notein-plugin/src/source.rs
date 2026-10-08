//! Source plugin trait — read-mostly content viewers.
//!
//! Each instance returned from [`Source::open`] is identified by an
//! opaque `u64` local to the plugin. The host hands anchor blobs back
//! and forth as `Vec<u8>` payloads whose shape is private to the
//! plugin that produced them — only the originating plugin ever
//! decodes them.
//!
//! Search is modelled as a poll-driven stream: [`Source::search_begin`]
//! returns a stream id, [`Source::search_poll`] drains the next batch
//! of hits, and [`Source::search_end`] releases the stream.

use rkyv::{Archive, Deserialize, Serialize};

use crate::config::{ConfigSchema, ConfigValue};
use crate::creation::{CreationEntry, ItemCreationResult};
use crate::error::{ConfigError, CreationError, SourceError};

/// Request for one rendered tile from a source.
#[derive(Clone, Copy, Debug, PartialEq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct TileRequest {
    /// Zero-based page index (sources without pagination use `0`).
    pub page_index: u32,
    /// Tile origin inside the source's content coordinate system.
    pub origin_x: f32,
    pub origin_y: f32,
    /// Tile size in pixels.
    pub width_px: u32,
    pub height_px: u32,
    /// Multiplier from content units to pixels (`1.0` = 100%).
    pub scale: f32,
}

/// A rendered tile, packed as 8-bit per channel RGBA pre-multiplied.
#[derive(Clone, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct Tile {
    pub rgba: Vec<u8>,
    pub width_px: u32,
    pub height_px: u32,
}

/// Human-readable snapshot of an anchor — used to render chips.
#[derive(Clone, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct AnchorView {
    /// Document / section title (e.g. `"Chapter 3"`).
    pub title: String,
    /// Short snippet around the anchored content.
    pub snippet: String,
}

/// The user's current selection inside the source viewer, if any.
#[derive(Clone, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct Selection {
    /// Opaque anchor the plugin encoded for this selection.
    pub anchor: Vec<u8>,
    pub snippet: String,
}

/// One full-text search hit returned by `search_poll`.
#[derive(Clone, Debug, PartialEq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct SearchHit {
    pub anchor: Vec<u8>,
    pub snippet: String,
    /// Relevance score; higher = better. Semantics are plugin-defined.
    pub score: f32,
}

/// Content viewer plugin.
///
/// Implementations are expected to be `Send + Sync` so the shell can
/// register them behind an `Arc<dyn Source>`.
pub trait Source: Send + Sync + std::fmt::Debug {
    /// Stable id for this plugin (e.g. `"io.notein.pdf"`).
    fn kind_id(&self) -> &str;
    /// Human label (e.g. `"PDF"`).
    fn kind_label(&self) -> &str;
    /// Extensions (lowercase, no leading dot) this source handles.
    fn file_extensions(&self) -> &[&'static str];

    /// Open an item from its stored bytes. Returns an instance handle
    /// opaque to the host.
    fn open(&self, item_bytes: &[u8]) -> Result<u64, SourceError>;
    /// Drop the instance; any later call with this handle must return
    /// [`SourceError::UnknownInstance`].
    fn close(&self, instance: u64);

    fn title(&self, instance: u64) -> String;
    /// Content size in the source's own coordinate system (not pixels).
    fn content_size(&self, instance: u64) -> (f32, f32);

    fn render_tile(&self, instance: u64, req: TileRequest) -> Result<Tile, SourceError>;

    /// Convert a content-space point to an anchor the plugin can later
    /// resolve. `None` = no content at that location.
    fn hit_test(&self, instance: u64, x: f32, y: f32) -> Option<Vec<u8>>;
    fn resolve_anchor(&self, instance: u64, anchor: &[u8]) -> Option<AnchorView>;
    fn current_selection(&self, instance: u64) -> Option<Selection>;

    /// Begin a search stream. Returns an id used in subsequent poll /
    /// end calls.
    fn search_begin(&self, instance: u64, query: &str) -> u64;
    /// Drain the next batch of hits (possibly empty).
    fn search_poll(&self, stream_id: u64) -> Vec<SearchHit>;
    fn search_end(&self, stream_id: u64);

    fn config_schema(&self) -> ConfigSchema;
    fn get_config(&self, key: &str) -> Option<ConfigValue>;
    fn set_config(&self, key: &str, value: ConfigValue) -> Result<(), ConfigError>;

    fn creation_entries(&self) -> Vec<CreationEntry>;
    /// Produce a new item.
    ///
    /// `import_bytes` is `Some` only when the entry is of
    /// [`crate::CreationKind::Import`] shape. `name_hint` carries the
    /// user-provided name for a `Blank { prompt_name: true }` flow.
    fn execute_creation(
        &self,
        entry_id: &str,
        import_bytes: Option<&[u8]>,
        name_hint: Option<&str>,
    ) -> Result<ItemCreationResult, CreationError>;
}
