//! Host-side API surface the shell gives to every plugin.
//!
//! Plugins reach for the host for cross-cutting services the shell is
//! the right owner of:
//!
//! - Progress reporting for long-running jobs (downloads, imports).
//!   Rendered as a thin indeterminate / determinate bar across the top
//!   of the window.
//! - Transient notifications ("install finished", "download failed").
//!   Rendered as toast-style overlays.
//! - Opening URLs in the platform-default handler. Lifted here so
//!   plugins don't each embed their own `webbrowser` / `open` dep.
//!
//! The host hands each registered plugin a cloneable
//! [`Host`] handle via [`crate::CanvasKind::install_host`] /
//! [`crate::Source::install_host`]; plugins store the clone and call
//! into it from their mutation paths. Default no-op impls mean an
//! existing plugin keeps compiling after the trait grows this method.
//!
//! The handle is intentionally minimal — it stays additive and typed in
//! this crate so hosts can grow richer facilities without turning the
//! surface into a one-off IPC channel.

use std::borrow::Cow;
use std::sync::Arc;

use crate::{Rgba, SourceRef};

/// Shared handle every plugin receives from the host. Clone cheaply.
pub type Host = Arc<dyn HostApi>;

/// Callback the host invokes once the bookmark edit modal closes.
/// `Some(draft)` when the user saved; `None` when cancelled / dismissed.
pub type BookmarkEditCallback = Arc<dyn Fn(Option<BookmarkDraft>) + Send + Sync>;

/// Callback the host invokes while the user is editing — fires on
/// every colour / body change so plugins can paint a live preview
/// without waiting for commit. The payload is the current in-progress
/// draft; final commit still flows through [`BookmarkEditCallback`].
pub type BookmarkEditPreview = Arc<dyn Fn(BookmarkDraft) + Send + Sync>;

pub trait HostApi: Send + Sync + std::fmt::Debug {
    /// Push a progress update for `token` (unique per operation inside
    /// `plugin_id`). The host surfaces the latest state per token.
    ///
    /// Emitting [`Progress::Finished`] drops the entry a short time
    /// later so the bar + any toast have a chance to animate out.
    fn report_progress(&self, plugin_id: &str, token: &str, progress: Progress);

    /// Push a notification toast. `message` is the short text shown to
    /// the user; the host owns display timing + dismissal.
    fn notify(&self, plugin_id: &str, kind: NotificationKind, message: String);

    /// Request the host open `url` in the platform's default handler.
    /// Lets plugins stay transport-free while still shipping deep-link
    /// chip clicks, "help" links from settings, etc.
    fn open_url(&self, url: &str);

    /// Register a font typeface under `font_name` so the renderer
    /// resolves `font_family("<font_name>")` references coming from
    /// the plugin's own UI. Plugins ship their fonts (typically via
    /// `include_bytes!`) and call this from `install_host` so the
    /// shell never has to know what fonts a plugin needs at startup.
    ///
    /// Multiple variants (regular / bold / italic) register under the
    /// same `font_name` — the font provider groups them by family and
    /// resolves weight / slant per request. `font_name` collisions
    /// across plugins overwrite the earlier registration, so plugin
    /// authors are expected to namespace (e.g. `"Lora"` is fine for
    /// a single owner, otherwise prefix with the plugin id).
    ///
    /// Default no-op so hosts without a renderer stay usable.
    fn load_font(&self, font_name: &str, font_data: Vec<u8>) {
        let _ = (font_name, font_data);
    }

    /// Ask the shell to open its full bookmark editor (body with
    /// autocomplete, color picker, source-ref chips) prefilled with
    /// `request.draft`. The host fires `on_done` once with the saved
    /// draft or `None` on cancel. Plugins get the shell's rich
    /// bookmark UX without reinventing it; shell stays the single
    /// owner of chip autocomplete + cross-plugin ref semantics.
    ///
    /// Default implementation returns `None` so plugins stay usable
    /// when embedded by hosts that don't yet surface the editor.
    fn request_bookmark_edit(
        &self,
        plugin_id: &str,
        request: BookmarkEditRequest,
        on_done: BookmarkEditCallback,
    ) {
        let _ = (plugin_id, request);
        on_done(None);
    }

    /// Queue a plugin-private anchor payload for `(plugin_id, item)`.
    /// Chip-tap navigation calls this before pushing the host route;
    /// the mounted viewer pulls it via [`Self::take_source_anchor`]
    /// (or [`Self::subscribe_source_anchor`] for same-route re-taps).
    ///
    /// The host is pure transport — the payload format is private to
    /// `plugin_id`. Default no-op so hosts without routing stay usable.
    fn queue_source_anchor(&self, plugin_id: &str, item: [u8; 16], payload: Vec<u8>) {
        let _ = (plugin_id, item, payload);
    }

    /// Take + clear the pending anchor payload for `(plugin_id, item)`.
    /// Default returns `None`.
    fn take_source_anchor(&self, plugin_id: &str, item: [u8; 16]) -> Option<Vec<u8>> {
        let _ = (plugin_id, item);
        None
    }

    /// Subscribe to anchor-queue notifications for `plugin_id`. The
    /// receiver fires with the target item id every time
    /// [`Self::queue_source_anchor`] runs under the same plugin id.
    /// Default returns a disconnected receiver so plugin viewers that
    /// drive their own re-tap handling never block.
    fn subscribe_source_anchor(&self, plugin_id: &str) -> flume::Receiver<[u8; 16]> {
        let _ = plugin_id;
        let (_tx, rx) = flume::bounded(0);
        rx
    }
}

/// Minimal bookmark payload exchanged between plugin + host when the
/// shell drives the editor UI. Deliberately a subset of the full
/// [`crate::Bookmark`] record — anchor + timestamps + lamport belong to
/// whichever side owns storage, so this struct only carries the
/// user-authored fields the modal mutates.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct BookmarkDraft {
    pub body: String,
    pub color: Option<Rgba>,
    pub refs: Vec<SourceRef>,
}

/// Request shape for [`HostApi::request_bookmark_edit`].
#[derive(Clone)]
pub struct BookmarkEditRequest {
    /// Prefilled draft — empty for a brand-new bookmark, populated
    /// for edits.
    pub draft: BookmarkDraft,
    /// Header string the modal shows ("Resaltar verso 5" etc.).
    pub title: Cow<'static, str>,
    /// Approximate anchor the plugin would prefer the modal to float
    /// near (gesture location in window coords). The shell may ignore
    /// it when its modal layout is fixed.
    pub anchor: Option<(f32, f32)>,
    /// Optional live-preview sink. When set, the shell fires it on
    /// every edit (colour wheel drag, body keystroke) so the plugin
    /// can paint a preview before the user commits.
    pub on_preview: Option<BookmarkEditPreview>,
}

impl std::fmt::Debug for BookmarkEditRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BookmarkEditRequest")
            .field("draft", &self.draft)
            .field("title", &self.title)
            .field("anchor", &self.anchor)
            .field("on_preview", &self.on_preview.is_some())
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Progress {
    /// Operation accepted; label is shown next to the bar.
    /// `total = None` renders indeterminate.
    Starting {
        label: String,
        total: Option<u64>,
    },
    /// Incremental update. `label = Some(_)` overrides the starting
    /// label when a longer-running task wants to describe sub-steps.
    Update {
        done: u64,
        total: Option<u64>,
        label: Option<String>,
    },
    /// Operation ended. `success = false` tints the bar warning / error
    /// so the user notices the failure before any accompanying
    /// notification.
    Finished {
        success: bool,
        note: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationKind {
    Info,
    Success,
    Warning,
    Error,
}
