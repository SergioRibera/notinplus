//! What the shell does for plugins.
//!
//! Every registered plugin receives a [`Host`]. Through it a plugin
//! shows toasts and progress, opens references, asks for the shared
//! annotation popover, cites a selection and moves to anchors the user
//! picked elsewhere (a chip, a search result, an outline row).

use std::sync::Arc;

use crate::bookmark::SourceRef;
use crate::doc::DocId;
use crate::face::{PaneKey, RawStore};

/// Shared handle to the shell's services. Cheap to clone.
pub type Host = Arc<dyn HostServices>;

/// Tone of a plugin toast.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ToastTone {
    #[default]
    Info,
    Done,
    Error,
    /// Something arrived over the network (a download finished).
    Download,
}

/// A toast a plugin asks the shell to show.
#[derive(Clone)]
pub struct HostToast {
    pub text: String,
    pub tone: ToastTone,
    /// Button label and what it does.
    pub action: Option<(String, Arc<dyn Fn() + Send + Sync>)>,
}

impl std::fmt::Debug for HostToast {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostToast")
            .field("text", &self.text)
            .field("tone", &self.tone)
            .finish_non_exhaustive()
    }
}

impl HostToast {
    #[must_use]
    pub fn new(text: impl Into<String>, tone: ToastTone) -> Self {
        Self {
            text: text.into(),
            tone,
            action: None,
        }
    }

    #[must_use]
    pub fn action(
        mut self,
        label: impl Into<String>,
        run: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        self.action = Some((label.into(), Arc::new(run)));
        self
    }
}

/// State of a long job (download, install, indexing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    /// `total = None` shows an indeterminate bar.
    Running {
        label: String,
        done: u64,
        total: Option<u64>,
    },
    Finished {
        success: bool,
        note: Option<String>,
    },
}

/// Where a reference opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Placement {
    /// The user's setting (Ajustes › Notas › Abrir referencias al lado).
    #[default]
    Preferred,
    /// The tab the user is in.
    Here,
    /// The other pane, splitting the mesa if needed.
    Beside,
}

/// A request for the annotation popover (verses, PDF fragments, canvas
/// pins all use the same one).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AnnotationRequest {
    pub doc: DocId,
    /// Kind that owns the annotated place.
    pub kind: String,
    /// Header ("Resaltar versos 16–17").
    pub title: String,
    /// Selected text shown as a quote.
    pub quote: Option<String>,
    /// Current colour (sRGB); `None` = no colour.
    pub color: Option<[u8; 3]>,
    /// Markdown body.
    pub body: String,
    /// Area the popover points at, in window coordinates
    /// `(x, y, width, height)`.
    pub target: Option<(f32, f32, f32, f32)>,
    /// The annotation already exists: shows Borrar.
    pub existing: bool,
}

/// How the annotation popover closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnnotationOutcome {
    Saved {
        color: Option<[u8; 3]>,
        body: String,
    },
    Deleted,
    Cancelled,
}

/// Called once when the annotation popover closes.
pub type AnnotationDone = Arc<dyn Fn(AnnotationOutcome) + Send + Sync>;
/// Called while the user edits, for a live preview of the colour.
pub type AnnotationPreview = Arc<dyn Fn(Option<[u8; 3]>) + Send + Sync>;

/// A move to an anchor inside a document, queued by the shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Jump {
    pub doc: DocId,
    pub anchor: Vec<u8>,
}

/// A press on one of a face's [`HeaderAction`](crate::face::HeaderAction)s.
#[derive(Clone, Debug, PartialEq)]
pub struct HeaderPress {
    pub doc: DocId,
    /// The pane whose header was pressed; a document open in two panes
    /// reacts only in this one.
    pub pane: Option<PaneKey>,
    /// [`HeaderAction::id`](crate::face::HeaderAction::id).
    pub action: String,
    /// Window point of the press, to hang a popover from.
    pub at: (f32, f32),
}

/// Services the shell provides. Implemented by the app; plugins only
/// call it.
pub trait HostServices: Send + Sync + std::fmt::Debug {
    fn toast(&self, toast: HostToast);

    /// Report progress of the job `token`.
    fn progress(&self, plugin: &str, token: &str, progress: Progress);

    fn open_url(&self, url: &str);

    /// Make `family` available to text the plugin renders.
    fn load_font(&self, family: &str, data: Vec<u8>);

    /// Open `target` (another document or a place in one).
    fn navigate(&self, target: SourceRef, placement: Placement);

    /// Show the annotation popover.
    fn annotate(
        &self,
        request: AnnotationRequest,
        preview: Option<AnnotationPreview>,
        done: AnnotationDone,
    );

    /// Put a reference on the clipboard and offer to paste it in the
    /// notebook beside.
    fn copy_reference(&self, reference: SourceRef, label: String);

    /// Insert a reference at the end of `from`'s notebook (creating one
    /// when it has none).
    fn insert_in_notebook(&self, from: DocId, reference: SourceRef);

    /// Moves queued for documents of `kind`.
    fn jumps(&self, kind: &str) -> flume::Receiver<Jump>;

    /// Take the move queued for `doc` while it was not open.
    fn take_jump(&self, kind: &str, doc: DocId) -> Option<Vec<u8>>;

    /// Presses on the header actions of documents of `kind`.
    fn header_presses(&self, kind: &str) -> flume::Receiver<HeaderPress>;

    /// The system share sheet is there to send things to.
    fn can_share(&self) -> bool;

    /// Send `what` through the system share sheet; the shell says so
    /// when it fails.
    fn share(&self, what: Share);

    /// The stored content of `doc`, a document of `kind`, outside its
    /// face.
    fn store(&self, doc: DocId, kind: &str) -> Arc<dyn RawStore>;

    /// The body of `doc` the shell merges with other devices and people;
    /// for kinds whose [`KindInfo::body`](crate::kind::KindInfo::body)
    /// is set. Keep it while the document is on screen.
    fn body(&self, doc: DocId) -> Arc<dyn TextBody>;

    /// The name of `doc` in the library. Call it from the UI thread.
    fn doc_name(&self, doc: DocId) -> Option<String>;

    /// The records of `doc`, for kinds whose
    /// [`KindInfo::records`](crate::kind::KindInfo::records) is set. Keep
    /// it while the document is on screen.
    fn records(&self, doc: DocId) -> Arc<dyn Records>;

    /// Who else has `doc` open, and where they are in it. Keep it while
    /// the document is on screen: holding it is what tells the others
    /// this person is here.
    fn presence(&self, doc: DocId) -> Arc<dyn Presence>;
}

/// Values a document keeps by key, merged with every device and person.
///
/// Two people changing different keys both keep their change; the same
/// key keeps one. Use it for anything several people
/// add to at once (highlights, comments, list entries) instead of one
/// content blob, where the last writer wins.
pub trait Records: Send + Sync {
    /// Every record, sorted by key.
    fn entries(&self) -> Vec<(String, Vec<u8>)>;
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.entries()
            .into_iter()
            .find_map(|(held, value)| (held == key).then_some(value))
    }
    fn set(&self, key: &str, value: &[u8]);
    fn remove(&self, key: &str);
    /// Fires when records arrive from elsewhere.
    fn changes(&self) -> flume::Receiver<()>;
}

/// Someone else with the same document open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    /// Stable per device, to tell people apart.
    pub id: [u8; 32],
    pub name: String,
    /// The color everything of theirs is drawn in.
    pub color: [u8; 3],
    /// What their face last published, in its own format; empty until it
    /// does.
    pub state: Vec<u8>,
}

/// This person's place in a document and everyone else's, sent as it
/// changes and never stored.
pub trait Presence: Send + Sync {
    /// Tell the others where this person is: a caret, a selection, a
    /// page. The face picks the format; keep it small (it is sent on
    /// every change) and send it again whenever it changes.
    fn publish(&self, state: Vec<u8>);
    /// Everyone else here now.
    fn peers(&self) -> Vec<Peer>;
    /// Fires when someone arrives, moves or leaves.
    fn changes(&self) -> flume::Receiver<()>;
    /// The person this device follows, when they are in this document: the
    /// face shows what they see (their page, their caret). Changes fire
    /// [`Self::changes`].
    fn followed(&self) -> Option<Peer>;
    /// This person moved on their own: stop following.
    fn stop_following(&self);
}

/// Something to send through the system share sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Share {
    Text {
        text: String,
        subject: String,
    },
    File {
        name: String,
        mime: String,
        bytes: Vec<u8>,
        subject: String,
    },
}

/// A text body the shell keeps merged with every device and person the
/// document is shared with.
pub trait TextBody: Send + Sync {
    fn text(&self) -> String;
    /// Replace the text, telling the others this person types; `false`
    /// when nothing changed.
    fn set(&self, text: &str) -> bool;
    /// Fires when text arrives from elsewhere.
    fn changes(&self) -> flume::Receiver<()>;
}
