//! Canvas plugin trait — owns a full pane.
//!
//! A canvas plugin controls rendering, input, and (optionally)
//! bookmarks within one pane. Multiple instances of the same canvas
//! plugin may run concurrently — the host disambiguates by instance
//! handle.
//!
//! Bookmark payloads cross as bincode-encoded `Vec<u8>` so the plugin
//! surface stays independent of the concrete `Bookmark` struct that
//! lives in the host crate today. The sdk crate (Phase 2.2) re-exports
//! the shared bookmark model so plugin authors can decode / encode
//! these payloads without rolling their own.
//!
//! [`FrameCtxHandle`] is an opaque pointer to the host's active draw
//! surface. The host side wraps a Skia canvas handle; downstream
//! plugins unwrap it via a surface-API trait the sdk will expose.
//! Keep the ABI side thin: raw ptr + length only.

use bincode::{Decode, Encode};

use crate::config::{ConfigSchema, ConfigValue};
use crate::creation::{CreationEntry, ItemCreationResult};
use crate::error::{CanvasError, ConfigError, CreationError};
use crate::ids::{BookmarkId, WorldPoint};

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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum PointerKind {
    /// Stylus / pen with pressure.
    Pen,
    /// Finger touch (no pressure).
    Touch,
    /// Mouse.
    Mouse,
}

/// User input delivered to the active canvas instance.
#[derive(Clone, Copy, Debug, PartialEq, Encode, Decode)]
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
    /// Bincoded `Vec<Bookmark>` — opaque to the shell; decoded via sdk
    /// helpers on the host side.
    fn list_bookmarks(&self, instance: u64) -> Vec<u8>;
    /// Replace the bookmark whose id matches the payload. Payload is a
    /// bincoded `Bookmark`.
    fn update_bookmark(&self, instance: u64, bookmark_bytes: &[u8]);
    fn delete_bookmark(&self, instance: u64, id: BookmarkId);
    fn navigate_to_bookmark(&self, instance: u64, id: BookmarkId);

    /// CRDT snapshot of the instance state (plugin-private encoding).
    fn serialize_snapshot(&self, instance: u64) -> Vec<u8>;
    /// Apply one remote op. Op bytes are plugin-private.
    fn apply_remote_op(&self, instance: u64, op: &[u8]);

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
