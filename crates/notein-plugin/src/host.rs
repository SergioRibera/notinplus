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

use std::sync::Arc;

/// Shared handle every plugin receives from the host. Clone cheaply.
pub type Host = Arc<dyn HostApi>;

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
