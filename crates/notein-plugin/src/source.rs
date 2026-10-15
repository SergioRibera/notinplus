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

use crate::bookmark::{Rgba, SourceRef};
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

/// Granularity of a `pointer_begin_select` gesture. Shell detects the
/// gesture (long-press, double-click, …) and tells the plugin what the
/// user intends; the plugin decides what "word" / "point" mean for its
/// own content.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SelectMode {
    /// Collapsed-caret start — subsequent `pointer_extend_select` calls
    /// drag the end from here. Driven by long-press on tablet or
    /// click-and-drag on desktop.
    Point,
    /// Snap the selection to the "word" containing the point. Driven by
    /// double-click / double-tap. Plugins without a word notion can
    /// fall back to [`SelectMode::Point`].
    Word,
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
pub trait Source: Send + Sync + std::fmt::Debug + std::any::Any {
    /// Downcast hook. Lets the shell reach plugin-specific methods on a
    /// known built-in (e.g. [`crate::Source`] held as `Arc<dyn Source>`
    /// but wanting to call PDF-only text selection APIs). Each impl just
    /// returns `self`.
    fn as_any(&self) -> &dyn std::any::Any;

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

    /// Begin a selection at the world-space point `(x, y)` with the
    /// granularity `mode`. Shell detects the triggering gesture
    /// (long-press, double-click) and routes it here; the plugin owns
    /// what the point / word means in its own content model.
    ///
    /// Returns `true` when the point actually hit selectable content.
    /// Default = no-op (`false`): source plugins that don't yet
    /// implement selection stay inert.
    fn pointer_begin_select(&self, instance: u64, x: f32, y: f32, mode: SelectMode) -> bool {
        let _ = (instance, x, y, mode);
        false
    }
    /// Extend an in-progress selection to the world-space point
    /// `(x, y)`. No-op when there is no active selection.
    fn pointer_extend_select(&self, instance: u64, x: f32, y: f32) {
        let _ = (instance, x, y);
    }
    /// Finalise the in-progress selection. The selection stays visible
    /// until [`Self::clear_selection`] is called.
    fn pointer_end_select(&self, instance: u64) {
        let _ = instance;
    }
    /// Clear any active or finalised selection. Default = no-op.
    fn clear_selection(&self, instance: u64) {
        let _ = instance;
    }

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

    /// Return candidate entries for the body-editor `[[` autocomplete,
    /// matching `query` (case-insensitive substring is the usual
    /// convention; plugins free to score however they like). Called at
    /// the plugin level — no instance context — because completions
    /// usually span the whole catalogue of refs the plugin can
    /// produce, not any one open document.
    ///
    /// Default = empty vec. Plugins that want to surface entries opt
    /// in by overriding.
    fn plugin_completions(&self, query: &str) -> Vec<Completion> {
        let _ = query;
        Vec::new()
    }

    /// Produce the visual for a chip pointing at `anchor`. The anchor
    /// blob is self-describing by convention (page / section / char
    /// range all baked in); no instance context is passed because
    /// chip rendering must stay cheap enough to run on every editor
    /// render frame without touching live document state.
    ///
    /// Default = `None` so the shell falls back to a generic label.
    /// Plugins that want branded chips override.
    fn render_chip(&self, anchor: &[u8]) -> Option<ChipView> {
        let _ = anchor;
        None
    }
}

/// Autocomplete entry surfaced by [`Source::plugin_completions`] and
/// the shell's built-in canvas / url providers. Picking an entry
/// inserts [`source`] as a chip in the editing body.
#[derive(Clone, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct Completion {
    /// Primary label (one line).
    pub label: String,
    /// Optional muted subtitle (e.g. plugin id, page number, path).
    pub subtitle: Option<String>,
    /// The ref inserted on pick.
    pub source: SourceRef,
}

/// Visual spec for a chip pill. The shell paints the pill; the plugin
/// supplies the text + icon + tint so chips look native to the data
/// they reference.
#[derive(Clone, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct ChipView {
    /// Text inside the pill (plugin-chosen; e.g. `"PDF p.12"`).
    pub label: String,
    /// Optional emoji or asset reference to paint left of the label.
    pub icon: Option<String>,
    /// Optional background tint. `None` = shell default.
    pub tint: Option<Rgba>,
    /// Optional accessible description / tooltip.
    pub tooltip: Option<String>,
}
