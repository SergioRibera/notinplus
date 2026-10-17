//! Host-side implementation of [`notein_plugin::HostApi`].
//!
//! One process-wide [`HostInner`] stores every active progress bar +
//! every live toast. Mutations bump an atomic version counter so UI
//! components can react — the top-level app spawns a poller
//! ([`spawn_version_poller`]) that mirrors the atomic into a Freya
//! [`State<u64>`], and the dependent components re-render on bump.
//!
//! The HostApi methods are safe to call from any thread (background
//! installers, download pumps, worker threads).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use notein_plugin::{
    BookmarkDraft, BookmarkEditCallback, BookmarkEditRequest, Host, HostApi, NotificationKind,
    Progress,
};

/// Oldest-to-newest cap on retained notifications. Older entries get
/// evicted so a chatty plugin cannot grow the toast list unbounded.
const MAX_NOTIFICATIONS: usize = 16;

/// Age at which a `Finished` progress bar entry drops out so the UI
/// does not permanently display a 100%-full line.
const FINISHED_LINGER: Duration = Duration::from_millis(900);

/// Age at which a Success / Info notification auto-dismisses. Warning
/// + Error toasts persist until the user clicks them.
const AUTO_DISMISS: Duration = Duration::from_secs(4);

#[derive(Debug)]
pub struct HostInner {
    version: AtomicU64,
    progress: RwLock<HashMap<ProgressKey, ProgressEntry>>,
    notifications: RwLock<VecDeque<Notification>>,
    /// At most one bookmark edit modal is open at a time. The shell
    /// renders when `Some`, drains + fires the callback on save/cancel.
    pending_bookmark_edit: RwLock<Option<PendingBookmarkEdit>>,
}

/// Live bookmark edit request pulled by the shell's modal.
pub struct PendingBookmarkEdit {
    pub id: u64,
    pub plugin_id: String,
    pub request: BookmarkEditRequest,
    pub callback: BookmarkEditCallback,
}

impl std::fmt::Debug for PendingBookmarkEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingBookmarkEdit")
            .field("id", &self.id)
            .field("plugin_id", &self.plugin_id)
            .finish_non_exhaustive()
    }
}

static NEXT_BOOKMARK_EDIT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Hash, PartialEq, Eq, Debug)]
pub struct ProgressKey {
    pub plugin_id: String,
    pub token: String,
}

#[derive(Clone, Debug)]
pub struct ProgressEntry {
    pub plugin_id: String,
    pub token: String,
    pub label: String,
    pub done: u64,
    pub total: Option<u64>,
    pub finished: Option<FinishedState>,
    pub updated_at: Instant,
}

#[derive(Clone, Debug)]
pub struct FinishedState {
    pub success: bool,
    pub note: Option<String>,
    pub shown_at: Instant,
}

#[derive(Clone, Debug)]
pub struct Notification {
    pub id: u64,
    pub plugin_id: String,
    pub kind: NotificationKind,
    pub message: String,
    pub created_at: Instant,
}

static HOST: OnceLock<Arc<HostInner>> = OnceLock::new();
static NEXT_NOTIFICATION_ID: AtomicU64 = AtomicU64::new(1);

/// Process-wide host state. Lazy-constructed on first access.
#[must_use]
pub fn host() -> Arc<HostInner> {
    Arc::clone(HOST.get_or_init(|| {
        Arc::new(HostInner {
            version: AtomicU64::new(0),
            progress: RwLock::new(HashMap::new()),
            notifications: RwLock::new(VecDeque::new()),
            pending_bookmark_edit: RwLock::new(None),
        })
    }))
}

/// Shared `Host` handle the registry hands to every plugin. Thin
/// trait-object wrapper around [`HostInner`].
#[must_use]
pub fn host_handle() -> Host {
    host() as Host
}

impl HostInner {
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    fn bump(&self) {
        self.version.fetch_add(1, Ordering::AcqRel);
    }

    /// Snapshot every live progress entry — stale `Finished` entries
    /// are dropped during the walk so callers always see fresh state.
    pub fn snapshot_progress(&self) -> Vec<ProgressEntry> {
        let now = Instant::now();
        let mut guard = self.progress.write().unwrap();
        guard.retain(|_, e| {
            e.finished
                .as_ref()
                .map(|f| now.saturating_duration_since(f.shown_at) < FINISHED_LINGER)
                .unwrap_or(true)
        });
        guard.values().cloned().collect()
    }

    /// Snapshot the retained notifications, evicting auto-dismiss
    /// candidates (Info / Success older than [`AUTO_DISMISS`]).
    pub fn snapshot_notifications(&self) -> Vec<Notification> {
        let now = Instant::now();
        let mut guard = self.notifications.write().unwrap();
        guard.retain(|n| match n.kind {
            NotificationKind::Info | NotificationKind::Success => {
                now.saturating_duration_since(n.created_at) < AUTO_DISMISS
            }
            NotificationKind::Warning | NotificationKind::Error => true,
        });
        guard.iter().cloned().collect()
    }

    pub fn dismiss_notification(&self, id: u64) {
        let mut guard = self.notifications.write().unwrap();
        guard.retain(|n| n.id != id);
        drop(guard);
        self.bump();
    }

    /// Snapshot the currently open bookmark edit request, if any. The
    /// shell polls this each tick and mounts its editor modal when
    /// `Some`.
    pub fn snapshot_bookmark_edit(&self) -> Option<(u64, String, BookmarkEditRequest)> {
        let guard = self.pending_bookmark_edit.read().unwrap();
        guard
            .as_ref()
            .map(|p| (p.id, p.plugin_id.clone(), p.request.clone()))
    }

    /// Resolve the pending bookmark edit by firing its callback with
    /// `result` and clearing the slot. The shell calls this from the
    /// modal's save (`Some(draft)`) + cancel (`None`) handlers. The
    /// `id` guard prevents a stale modal from resolving a request that
    /// was superseded by a new one.
    pub fn resolve_bookmark_edit(&self, id: u64, result: Option<BookmarkDraft>) {
        let callback = {
            let mut guard = self.pending_bookmark_edit.write().unwrap();
            match guard.as_ref() {
                Some(p) if p.id == id => guard.take().map(|p| p.callback),
                _ => None,
            }
        };
        if let Some(cb) = callback {
            cb(result);
            self.bump();
        }
    }
}

impl HostApi for HostInner {
    fn report_progress(&self, plugin_id: &str, token: &str, progress: Progress) {
        let key = ProgressKey {
            plugin_id: plugin_id.to_string(),
            token: token.to_string(),
        };
        let mut guard = self.progress.write().unwrap();
        let entry = guard.entry(key.clone()).or_insert(ProgressEntry {
            plugin_id: key.plugin_id.clone(),
            token: key.token.clone(),
            label: String::new(),
            done: 0,
            total: None,
            finished: None,
            updated_at: Instant::now(),
        });
        match progress {
            Progress::Starting { label, total } => {
                entry.label = label;
                entry.done = 0;
                entry.total = total;
                entry.finished = None;
            }
            Progress::Update { done, total, label } => {
                entry.done = done;
                if total.is_some() {
                    entry.total = total;
                }
                if let Some(l) = label {
                    entry.label = l;
                }
            }
            Progress::Finished { success, note } => {
                entry.finished = Some(FinishedState {
                    success,
                    note,
                    shown_at: Instant::now(),
                });
            }
        }
        entry.updated_at = Instant::now();
        drop(guard);
        self.bump();
    }

    fn notify(&self, plugin_id: &str, kind: NotificationKind, message: String) {
        let id = NEXT_NOTIFICATION_ID.fetch_add(1, Ordering::Relaxed);
        let notification = Notification {
            id,
            plugin_id: plugin_id.to_string(),
            kind,
            message,
            created_at: Instant::now(),
        };
        let mut guard = self.notifications.write().unwrap();
        guard.push_back(notification);
        while guard.len() > MAX_NOTIFICATIONS {
            guard.pop_front();
        }
        drop(guard);
        self.bump();
    }

    fn open_url(&self, url: &str) {
        // Reuse the shell's url opener — same path shell-side chip
        // navigation goes through.
        crate::source_ref_nav::open_external_url(url);
    }

    fn load_font(&self, font_name: &str, font_data: Vec<u8>) {
        // Freya's `Platform::get()` panics outside the scheduler's
        // root context — and most `install_host` calls fire during
        // `plugins::hydrate_from_disk`, which runs *before* `launch`.
        // So every registration lands in `PENDING_FONTS`; the root
        // shell drains the buffer once per mount via
        // [`flush_pending_fonts`] where `Platform::get()` is safe.
        //
        // Plugins loaded hot (post-launch) also funnel through here;
        // the drain runs again on the next `AppShell` re-render.
        let mut guard = pending_fonts().lock().unwrap_or_else(|e| e.into_inner());
        guard.push((font_name.to_owned(), font_data));
    }

    fn request_bookmark_edit(
        &self,
        plugin_id: &str,
        request: BookmarkEditRequest,
        on_done: BookmarkEditCallback,
    ) {
        let id = NEXT_BOOKMARK_EDIT_ID.fetch_add(1, Ordering::Relaxed);
        // If a prior request is still open, release it with None so
        // the previous plugin cleans up before the new one takes over.
        let stale = {
            let mut guard = self.pending_bookmark_edit.write().unwrap();
            let stale = guard.take();
            *guard = Some(PendingBookmarkEdit {
                id,
                plugin_id: plugin_id.to_string(),
                request,
                callback: on_done,
            });
            stale
        };
        if let Some(prev) = stale {
            (prev.callback)(None);
        }
        self.bump();
    }
}

/// Font registrations queued by [`HostInner::load_font`] before the
/// freya root context exists. Drained by [`flush_pending_fonts`].
static PENDING_FONTS: OnceLock<Mutex<Vec<(String, Vec<u8>)>>> = OnceLock::new();

fn pending_fonts() -> &'static Mutex<Vec<(String, Vec<u8>)>> {
    PENDING_FONTS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Push any buffered font registrations through `freya_core::Platform`
/// so renderer windows pick up new typefaces. Must be called from
/// inside a freya render / event-handler context (e.g. a root-scope
/// `use_hook`) — [`freya_core::platform::Platform::get`] panics
/// otherwise.
///
/// Idempotent: a second call after the buffer has been drained is a
/// no-op, so wiring this into every `AppShell::render` is safe.
pub fn flush_pending_fonts() {
    let Some(buffer) = PENDING_FONTS.get() else {
        return;
    };
    let drained: Vec<(String, Vec<u8>)> = {
        let mut guard = buffer.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *guard)
    };
    if drained.is_empty() {
        return;
    }
    let platform = freya_core::platform::Platform::get();
    for (name, bytes) in drained {
        platform.load_font(name, bytes);
    }
}

/// Install `host_handle()` into every registered canvas / source
/// trait object. Idempotent — call at startup right after
/// [`crate::builtin::register_all`] + the dynamic plugin scan.
pub fn install_host_in_registry(registry: &notein_plugin::PluginRegistry) {
    let handle = host_handle();
    for canvas in registry.canvases() {
        canvas.install_host(Arc::clone(&handle));
    }
    for source in registry.sources() {
        source.install_host(Arc::clone(&handle));
    }
}
