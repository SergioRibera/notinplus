//! Files opened from outside the library, each in a view of its own.
//!
//! A `.pdf`, `.md` or `.txt` the system hands over shows with nothing of
//! the library around it, and stays out of the library until the person
//! saves it there.
//!
//! On desktop each file opens in a new window, so the window the person
//! was looking at never changes. Phones and tablets have one window: the
//! view covers the current screen and closing it returns there.
//!
//! Markdown and text edits go back to the file they came from, a second
//! after the typing stops (on Android and iOS the file is a copy the
//! other app shared, and the edits stay in that copy).

mod pdf;
mod recents;
mod text;
mod view;

use std::cell::OnceCell;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use freya::prelude::*;
use notein_i18n::tr;

pub use recents::{Recent, reopen, use_recents};
pub use view::ExternalView;

use crate::files::Picked;

/// Whether every file gets a window of its own (desktop) or covers the
/// one window there is (phones, tablets).
const OWN_WINDOWS: bool = !cfg!(any(target_os = "android", target_os = "ios"));

/// What the view can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Markdown,
    Text,
    Pdf,
}

impl FileKind {
    /// The kind of `name`, by extension. `None` for files the view does
    /// not show.
    #[must_use]
    pub fn of(name: &str) -> Option<Self> {
        let ext = Path::new(name)
            .extension()
            .and_then(|ext| ext.to_str())?
            .to_ascii_lowercase();
        match ext.as_str() {
            "md" | "markdown" => Some(Self::Markdown),
            "txt" => Some(Self::Text),
            "pdf" => Some(Self::Pdf),
            _ => None,
        }
    }

    /// Short label of the type badge.
    #[must_use]
    pub const fn badge(self) -> &'static str {
        match self {
            Self::Markdown => "MD",
            Self::Text => "TXT",
            Self::Pdf => "PDF",
        }
    }

    /// Whether the view edits it.
    #[must_use]
    pub const fn editable(self) -> bool {
        match self {
            Self::Markdown | Self::Text => true,
            Self::Pdf => false,
        }
    }

    const fn mime(self) -> &'static str {
        match self {
            Self::Markdown => "text/markdown",
            Self::Text => "text/plain",
            Self::Pdf => "application/pdf",
        }
    }
}

/// Where the shown file lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Location {
    /// The file itself: edits are written back to it.
    Original,
    /// A copy another app shared (Android, iOS): edits stay in the copy.
    Copy,
}

/// A file opened from outside the library, read into memory.
#[derive(Clone)]
pub struct ExternalFile {
    pub path: PathBuf,
    pub name: String,
    pub kind: FileKind,
    pub location: Location,
    /// Whether edits can be written to [`Self::path`].
    pub writable: bool,
    content: Arc<[u8]>,
}

impl PartialEq for ExternalFile {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
            && self.name == other.name
            && self.writable == other.writable
            && Arc::ptr_eq(&self.content, &other.content)
    }
}

impl std::fmt::Debug for ExternalFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalFile")
            .field("path", &self.path)
            .field("kind", &self.kind)
            .field("location", &self.location)
            .field("writable", &self.writable)
            .field("size", &self.content.len())
            .finish_non_exhaustive()
    }
}

impl ExternalFile {
    /// `picked`, read from `path`, if the view shows its kind.
    #[must_use]
    pub fn new(path: PathBuf, picked: Picked) -> Option<Self> {
        let kind = FileKind::of(&picked.name)?;
        let writable =
            kind.editable() && std::fs::OpenOptions::new().append(true).open(&path).is_ok();
        Some(Self {
            path,
            name: picked.name,
            kind,
            location: if OWN_WINDOWS {
                Location::Original
            } else {
                Location::Copy
            },
            writable,
            content: picked.bytes.into(),
        })
    }

    /// The bytes as they were read.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.content
    }

    /// Name of the folder holding the file; `None` for a copy, whose
    /// folder means nothing to the person.
    #[must_use]
    pub fn folder(&self) -> Option<String> {
        match self.location {
            Location::Copy => None,
            Location::Original => self
                .path
                .parent()
                .and_then(Path::file_name)
                .map(|name| name.to_string_lossy().into_owned()),
        }
    }

    /// Folder and size, the line under the name.
    #[must_use]
    pub fn details(&self) -> String {
        let size = crate::format::size(self.content.len() as u64);
        match self.folder() {
            Some(folder) => tr!("external-files-details", "folder" => folder, "size" => size),
            None => size,
        }
    }
}

/// One open view of a file: the file and the edits waiting to be written.
#[derive(Clone, Debug)]
pub struct Session {
    pub file: ExternalFile,
    autosave: Arc<Autosave>,
}

impl PartialEq for Session {
    fn eq(&self, other: &Self) -> bool {
        self.file == other.file && Arc::ptr_eq(&self.autosave, &other.autosave)
    }
}

impl From<ExternalFile> for Session {
    fn from(file: ExternalFile) -> Self {
        Self {
            autosave: Arc::new(Autosave::from(file.path.clone())),
            file,
        }
    }
}

/// Text waiting to be written to a file, and the bell that wakes the
/// writer.
#[derive(Debug)]
struct Autosave {
    path: PathBuf,
    pending: Mutex<Option<String>>,
    wake: (flume::Sender<()>, flume::Receiver<()>),
}

impl From<PathBuf> for Autosave {
    fn from(path: PathBuf) -> Self {
        Self {
            path,
            pending: Mutex::new(None),
            wake: flume::unbounded(),
        }
    }
}

impl Autosave {
    /// Keep `text` to be written once the typing stops.
    fn schedule(&self, text: String) {
        *self.pending.lock().unwrap_or_else(PoisonError::into_inner) = Some(text);
        let _ = self.wake.0.send(());
    }

    /// The text waiting to be written, taken.
    fn take(&self) -> Option<String> {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// Write what is waiting now, on this thread: the view is closing.
    fn flush(&self) {
        if let Some(text) = self.take()
            && let Err(err) = std::fs::write(&self.path, text)
        {
            log::error!("save {}: {err}", self.path.display());
        }
    }
}

/// Show `file` in its own view.
#[sismo::function("external_files::show")]
pub fn show(file: ExternalFile) {
    recents::remember(&file);
    let session = Session::from(file);
    if OWN_WINDOWS {
        launch_window(session);
    } else if let Some(mut shown) = try_consume_root_context::<Shown>() {
        shown.0.set(Some(session));
    }
}

/// Close the view of `session`, writing what is still waiting first.
pub(crate) fn close(session: &Session) {
    session.autosave.flush();
    if OWN_WINDOWS {
        Platform::get().close_window(Platform::window_id());
    } else if let Some(mut shown) = try_consume_root_context::<Shown>() {
        shown.0.set(None);
    }
}

/// Show `file` in place of the view of `session`.
pub(crate) fn replace(session: &Session, file: ExternalFile) {
    session.autosave.flush();
    if OWN_WINDOWS {
        let current = Platform::window_id();
        in_main_window(move || {
            launch_window(Session::from(file));
            Platform::get().close_window(current);
        });
    } else if let Some(mut shown) = try_consume_root_context::<Shown>() {
        shown.0.set(Some(Session::from(file)));
    }
}

#[sismo::function("external_files::launch_window")]
fn launch_window(session: Session) {
    let title = tr!("external-files-window-title", "name" => session.file.name.clone());
    let closing = Arc::clone(&session.autosave);
    let mut window = WindowConfig::new_app(view::ExternalWindow { session })
        .with_size(960.0, 760.0)
        .with_min_size(360.0, 420.0)
        .with_background(notein_ui_kit::palette().bg)
        .with_app_id(env!("ISTMO_APP_BUNDLE_ID"))
        .with_on_close(move |_, _| {
            closing.flush();
            CloseDecision::Close
        });
    if let Some(icon) = istmo::app_icon!() {
        window = window.with_icon(LaunchConfig::window_icon(icon));
    }
    spawn_in_window(async move {
        let id = Platform::get().launch_window(window).await;
        Platform::get().set_window_title(id, title);
    });
}

/// The view on screen in a single-window app, at its root.
#[derive(Clone, Copy, PartialEq)]
struct Shown(State<Option<Session>>);

/// Install the slot of the view at the root of the app window.
pub fn use_external_root() {
    let shown = use_state(|| None::<Session>);
    use_hook(|| provide_context_for_scope_id(Shown(shown), ScopeId::ROOT));
}

/// The view over the whole app window while a file is open (phones and
/// tablets). Nothing on desktop, where files open in windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExternalOverlay;

impl Component for ExternalOverlay {
    #[sismo::function("external_files::overlay")]
    fn render(&self) -> impl IntoElement {
        let Some(Shown(shown)) = try_consume_root_context::<Shown>() else {
            return rect().into_element();
        };
        let Some(session) = shown.read().clone() else {
            return rect().into_element();
        };
        let key = session.file.path.clone();
        rect()
            .position(Position::new_absolute().top(0.0).left(0.0))
            .width(Size::fill())
            .height(Size::fill())
            .layer(Layer::Overlay)
            .background(notein_ui_kit::palette().bg)
            .child(rect().expanded().key(key).child(ExternalView { session }))
            .into_element()
    }
}

thread_local! {
    /// Work for the app window, sent from the windows of opened files.
    /// Every window runs on the UI thread, so the closures need not be
    /// `Send`.
    static MAIN: OnceCell<flume::Sender<Box<dyn FnOnce()>>> = const { OnceCell::new() };
}

/// Run what the windows of opened files ask of the app window (opening
/// a document in the library, following a link), for as long as it is
/// open. Call once from the app window.
#[sismo::function("external_files::listen")]
pub fn listen() {
    let (tx, rx) = flume::unbounded::<Box<dyn FnOnce()>>();
    if MAIN.with(|main| main.set(tx)).is_err() {
        return;
    }
    let window = Platform::window_id();
    spawn_in_window(traced!("task::external_files::main", async move {
        while let Ok(run) = rx.recv_async().await {
            run();
            Platform::get().focus_window(window);
        }
    }));
}

/// Run `run` in the app window, or here when there is no other.
pub(crate) fn in_main_window(run: impl FnOnce() + 'static) {
    let tx = MAIN.with(|main| main.get().cloned());
    match tx {
        Some(tx) if OWN_WINDOWS => {
            if let Err(flume::SendError(run)) = tx.send(Box::new(run)) {
                run();
            }
        }
        _ => run(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_by_extension() {
        assert_eq!(FileKind::of("Romanos.PDF"), Some(FileKind::Pdf));
        assert_eq!(FileKind::of("apuntes.md"), Some(FileKind::Markdown));
        assert_eq!(FileKind::of("apuntes.markdown"), Some(FileKind::Markdown));
        assert_eq!(FileKind::of("lista.txt"), Some(FileKind::Text));
        assert_eq!(FileKind::of("foto.png"), None);
        assert_eq!(FileKind::of("sin-extension"), None);
    }

    #[test]
    fn autosave_writes_only_what_waits() {
        let dir = std::env::temp_dir().join(format!("notein-autosave-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("nota.md");
        std::fs::write(&path, "antes").expect("seed");
        let autosave = Autosave::from(path.clone());
        autosave.flush();
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "antes");
        autosave.schedule("uno".to_owned());
        autosave.schedule("dos".to_owned());
        autosave.flush();
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "dos");
        assert!(autosave.take().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
