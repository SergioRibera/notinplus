//! Canvas plugin trait — owns a full pane.
//!
//! A canvas plugin controls rendering, input, and (optionally)
//! bookmarks within one pane. Multiple instances of the same canvas
//! plugin may run concurrently — the host disambiguates by instance
//! handle.
//!
//! Shared types ([`Bookmark`], [`crate::BookmarkId`], [`InputEvent`])
//! cross the trait boundary as typed Rust values — zero encode /
//! decode on the hot path. Payloads that only the plugin itself
//! understands (CRDT op bytes in [`CanvasKind::apply_remote_op`],
//! snapshot bytes from [`CanvasKind::serialize_snapshot`], the item's
//! stored bytes in [`CanvasKind::open`]) stay opaque `Vec<u8>` /
//! `&[u8]` — the shell is pure transport for those.
//!
//! Phase 3 adds a stabby wrapper layer for dynamic plugins that need
//! ABI stability across dylib boundaries. That layer serializes the
//! typed values at the FFI crossing only — the in-process trait kept
//! here never pays that cost.
//!
//! [`FrameCtxHandle`] is an opaque pointer to the host's active draw
//! surface bundled with its physical dimensions and scale factor. The
//! host side wraps a Skia canvas handle; downstream plugins unwrap it
//! via a surface-API trait the sdk will expose.

use rkyv::{Archive, Deserialize, Serialize};

use crate::bookmark::{Bookmark, WorldPoint};
use crate::config::{ConfigSchema, ConfigValue};
use crate::creation::{CreationEntry, ItemCreationResult};
use crate::error::{CanvasError, ConfigError, CreationError};
use crate::ids::{BookmarkId, OpId};

/// Opaque handle to the host's current draw context.
///
/// The host passes this to [`CanvasKind::render`]. Plugins reinterpret
/// via a sdk-provided conversion function; the raw pointer is never
/// dereferenced in this crate.
#[derive(Clone, Copy, Debug)]
pub struct FrameCtxHandle {
    /// Type-erased pointer to the host's draw context.
    pub ptr: *mut std::ffi::c_void,
    /// Width of the target surface in physical pixels.
    pub width_px: u32,
    /// Height of the target surface in physical pixels.
    pub height_px: u32,
    /// Scale factor from logical pixels to physical pixels.
    pub scale_factor: f32,
}

// Safe because the host never shares the ptr across threads — one
// pane renders on one thread at a time. Plugins must not retain the
// handle beyond the `render` call.
unsafe impl Send for FrameCtxHandle {}
unsafe impl Sync for FrameCtxHandle {}

/// Pointer-event kind. Mirrors the host's input abstraction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub enum PointerKind {
    /// Stylus / pen with pressure.
    Pen,
    /// Finger touch (no pressure).
    Touch,
    /// Mouse.
    Mouse,
}

/// User input delivered to the active canvas instance.
#[derive(Clone, Copy, Debug, PartialEq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub enum InputEvent {
    PointerDown {
        x: f32,
        y: f32,
        pressure: f32,
        kind: PointerKind,
    },
    PointerMove {
        x: f32,
        y: f32,
        pressure: f32,
        kind: PointerKind,
    },
    PointerUp {
        x: f32,
        y: f32,
        kind: PointerKind,
    },
    Scroll {
        dx: f32,
        dy: f32,
    },
    /// Keyboard key down. `code` is the host's key code (platform-
    /// specific); the plugin interprets.
    KeyDown {
        code: u32,
        modifiers: u32,
    },
    KeyUp {
        code: u32,
        modifiers: u32,
    },
}

/// Canvas plugin.
///
/// Implementations are expected to be `Send + Sync` so the shell can
/// register them behind an `Arc<dyn CanvasKind>`.
pub trait CanvasKind: Send + Sync + std::fmt::Debug {
    /// Stable id (e.g. `"io.notein.canvas.stroke"`).
    fn kind_id(&self) -> &str;
    /// Human label (e.g. `"Lienzo infinito"`).
    fn kind_label(&self) -> &str;
    /// Capabilities declared by this canvas (e.g. `"bookmarks"`,
    /// `"strokes"`, `"layers"`). The host feature-gates UI off this.
    fn capabilities(&self) -> &[&'static str];

    fn open(&self, item_bytes: &[u8]) -> Result<u64, CanvasError>;
    fn close(&self, instance: u64);

    /// Paint a frame to the host-provided draw surface.
    ///
    /// Called on the UI thread. Plugin must not retain `ctx` past
    /// this call.
    fn render(&self, instance: u64, ctx: FrameCtxHandle);

    fn on_input(&self, instance: u64, event: InputEvent);

    /// Create a bookmark at `at`. Returns the new id, or `None` if the
    /// canvas does not support bookmarks.
    fn create_bookmark(&self, instance: u64, at: WorldPoint) -> Option<BookmarkId>;
    fn list_bookmarks(&self, instance: u64) -> Vec<Bookmark>;
    fn update_bookmark(&self, instance: u64, bookmark: Bookmark);
    fn delete_bookmark(&self, instance: u64, id: BookmarkId);
    fn navigate_to_bookmark(&self, instance: u64, id: BookmarkId);

    /// CRDT snapshot of the instance state. Plugin-private encoding —
    /// the shell stores + ships the bytes but never decodes them.
    fn serialize_snapshot(&self, instance: u64) -> Vec<u8>;

    /// `(snapshot_bytes, at)` — the snapshot plus the op id up to and
    /// including which it reflects state. Plugins that have not reached
    /// Phase 10 can keep the default impl, which returns an empty
    /// snapshot at the zero op id; the shell treats that as "replay the
    /// full log on load".
    fn snapshot_at(&self, instance: u64) -> (Vec<u8>, OpId) {
        let _ = instance;
        (Vec::new(), OpId::default())
    }

    /// Reconstruct instance state from a previously-taken snapshot
    /// along with its op id. Plugins that do not yet implement
    /// snapshotting leave the default no-op; the shell will fall back
    /// to replaying the full op log.
    fn restore(&self, instance: u64, snapshot: &[u8], at: OpId) {
        let _ = (instance, snapshot, at);
    }

    /// Local ops produced since the last call. The shell drains the
    /// vec, persists it to the log, and (phase 10) broadcasts to
    /// peers. Default is empty — plugins without Phase 10 wiring stay
    /// local-only.
    fn emit_pending_ops(&self, instance: u64) -> Vec<Vec<u8>> {
        let _ = instance;
        Vec::new()
    }

    /// Apply one remote op. Op bytes are plugin-private; the shell is
    /// pure CRDT transport.
    fn apply_remote_op(&self, instance: u64, op: &[u8]);

    /// Return `true` when `op` can be undone by this plugin. The shell
    /// uses this to decide whether to push the op onto the local undo
    /// stack; `false` ops are applied but never undoable (e.g. remote
    /// bookkeeping, cursor pings).
    fn is_undoable(&self, instance: u64, op: &[u8]) -> bool {
        let _ = (instance, op);
        false
    }

    /// Compute an inverse op for `op` so that applying it undoes the
    /// effect of the original. Returns `None` when `op` is not
    /// invertible by this plugin. The shell emits the inverse as a
    /// fresh op (new OpId / lamport) rather than mutating the log,
    /// matching Figma / Google Docs semantics.
    fn inverse_op(&self, instance: u64, op: &[u8]) -> Option<Vec<u8>> {
        let _ = (instance, op);
        None
    }

    fn config_schema(&self) -> ConfigSchema;
    fn get_config(&self, key: &str) -> Option<ConfigValue>;
    fn set_config(&self, key: &str, value: ConfigValue) -> Result<(), ConfigError>;

    fn creation_entries(&self) -> Vec<CreationEntry>;
    fn execute_creation(
        &self,
        entry_id: &str,
        import_bytes: Option<&[u8]>,
        name_hint: Option<&str>,
    ) -> Result<ItemCreationResult, CreationError>;
}
