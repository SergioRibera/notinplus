//! What the shell does for plugins.
//!
//! Every registered plugin receives a [`Host`]. Through it a plugin
//! shows toasts and progress, opens references, asks for the shared
//! annotation popover, cites a selection and moves to anchors the user
//! picked elsewhere (a chip, a search result, an outline row).

use std::sync::Arc;

use crate::bookmark::SourceRef;
use crate::doc::DocId;

/// Shared handle to the shell's services. Cheap to clone.
pub type Host = Arc<dyn HostServices>;

/// Tone of a plugin toast.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ToastTone {
    #[default]
    Info,
    Done,
    Error,
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
}
