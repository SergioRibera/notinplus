//! The services the shell gives plugins.
//!
//! Plugins may call the host from any thread. Everything that touches
//! the UI is queued and applied on the UI thread by [`use_host_pump`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use freya::prelude::*;
use notein_plugin::bookmark::SourceRef;
use notein_plugin::doc::DocId;
use notein_plugin::services::{
    AnnotationDone, AnnotationPreview, AnnotationRequest, Host, HostServices, HostToast, Jump,
    Placement, Progress, ToastTone as HostTone,
};
use notein_ui_kit::{Toast, ToastId, ToastTone, Toasts};

/// Something the UI thread must do for a plugin.
enum HostEvent {
    Toast(HostToast),
    Progress {
        key: (String, String),
        progress: Progress,
    },
    Font(String, Vec<u8>),
    Navigate(SourceRef, Placement),
    Annotate {
        request: AnnotationRequest,
        preview: Option<AnnotationPreview>,
        done: AnnotationDone,
    },
    CopyReference(SourceRef, String),
    InsertInNotebook(DocId, SourceRef),
}

/// The shell's [`HostServices`].
pub struct ShellHost {
    events: flume::Sender<HostEvent>,
    pump: flume::Receiver<HostEvent>,
    pending_jumps: Mutex<HashMap<(String, DocId), Vec<u8>>>,
    jump_listeners: Mutex<Vec<(String, flume::Sender<Jump>)>>,
}

impl std::fmt::Debug for ShellHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellHost").finish_non_exhaustive()
    }
}

static HOST: OnceLock<Arc<ShellHost>> = OnceLock::new();

fn shell_host() -> &'static Arc<ShellHost> {
    HOST.get_or_init(|| {
        let (events, pump) = flume::unbounded();
        Arc::new(ShellHost {
            events,
            pump,
            pending_jumps: Mutex::default(),
            jump_listeners: Mutex::default(),
        })
    })
}

/// The host handle handed to plugins.
#[must_use]
pub fn handle() -> Host {
    Arc::clone(shell_host()) as Host
}

impl ShellHost {
    fn send(&self, event: HostEvent) {
        if self.events.send(event).is_err() {
            log::warn!("host event dropped: pump gone");
        }
    }

    /// Queue a move to `anchor` inside `doc` (a document of `kind`) and
    /// wake its open faces.
    pub fn queue_jump(&self, kind: &str, doc: DocId, anchor: Vec<u8>) {
        self.pending_jumps
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((kind.to_owned(), doc), anchor.clone());
        self.jump_listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(k, tx)| {
                k != kind
                    || tx
                        .send(Jump {
                            doc,
                            anchor: anchor.clone(),
                        })
                        .is_ok()
            });
    }
}

/// The shell host, for shell code that queues jumps directly.
#[must_use]
pub fn shell() -> Arc<ShellHost> {
    Arc::clone(shell_host())
}

impl HostServices for ShellHost {
    fn toast(&self, toast: HostToast) {
        self.send(HostEvent::Toast(toast));
    }

    fn progress(&self, plugin: &str, token: &str, progress: Progress) {
        self.send(HostEvent::Progress {
            key: (plugin.to_owned(), token.to_owned()),
            progress,
        });
    }

    fn open_url(&self, url: &str) {
        crate::external::open_url(url);
    }

    fn load_font(&self, family: &str, data: Vec<u8>) {
        self.send(HostEvent::Font(family.to_owned(), data));
    }

    fn navigate(&self, target: SourceRef, placement: Placement) {
        self.send(HostEvent::Navigate(target, placement));
    }

    fn annotate(
        &self,
        request: AnnotationRequest,
        preview: Option<AnnotationPreview>,
        done: AnnotationDone,
    ) {
        self.send(HostEvent::Annotate {
            request,
            preview,
            done,
        });
    }

    fn copy_reference(&self, reference: SourceRef, label: String) {
        self.send(HostEvent::CopyReference(reference, label));
    }

    fn insert_in_notebook(&self, from: DocId, reference: SourceRef) {
        self.send(HostEvent::InsertInNotebook(from, reference));
    }

    fn jumps(&self, kind: &str) -> flume::Receiver<Jump> {
        let (tx, rx) = flume::unbounded();
        self.jump_listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((kind.to_owned(), tx));
        rx
    }

    fn take_jump(&self, kind: &str, doc: DocId) -> Option<Vec<u8>> {
        self.pending_jumps
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(kind.to_owned(), doc))
    }
}

/// How long a finished job's toast stays.
const FINISHED_TOAST: Duration = Duration::from_millis(3_500);

fn tone(tone: HostTone) -> ToastTone {
    match tone {
        HostTone::Info => ToastTone::Info,
        HostTone::Done => ToastTone::Done,
        HostTone::Error => ToastTone::Error,
    }
}

fn apply(event: HostEvent, jobs: &mut HashMap<(String, String), ToastId>) {
    let mut toasts = Toasts::get();
    match event {
        HostEvent::Toast(toast) => {
            let mut shown = Toast::new(toast.text).tone(tone(toast.tone));
            if let Some((label, run)) = toast.action {
                shown = shown.action(label, move |()| run());
            }
            toasts.show(shown);
        }
        HostEvent::Progress { key, progress } => match progress {
            Progress::Running { label, done, total } => {
                #[allow(clippy::cast_precision_loss)] // progress fraction for display only
                let (value, caption) = match total {
                    Some(total) if total > 0 => {
                        let fraction = done as f32 / total as f32;
                        (fraction, format!("{} %", (fraction * 100.0).round()))
                    }
                    _ => (0.0, String::new()),
                };
                let toast = Toast::new(label)
                    .tone(ToastTone::Download)
                    .progress(value, caption);
                match jobs.get(&key) {
                    Some(id) => toasts.update(*id, toast),
                    None => {
                        let id = toasts.show(toast);
                        jobs.insert(key, id);
                    }
                }
            }
            Progress::Finished { success, note } => {
                if let Some(id) = jobs.remove(&key) {
                    toasts.dismiss(id);
                }
                if let Some(note) = note {
                    toasts.show(
                        Toast::new(note)
                            .tone(if success {
                                ToastTone::Done
                            } else {
                                ToastTone::Error
                            })
                            .duration(FINISHED_TOAST),
                    );
                }
            }
        },
        HostEvent::Font(family, data) => Platform::get().load_font(family, data),
        HostEvent::Navigate(target, placement) => super::navigate::open(&target, placement),
        HostEvent::Annotate {
            request,
            preview,
            done,
        } => {
            crate::annotation::AnnotationPopover::open(request, preview, done);
        }
        HostEvent::CopyReference(reference, label) => crate::references::copy(&reference, &label),
        HostEvent::InsertInNotebook(from, reference) => {
            crate::references::insert_in_notebook(crate::library::ItemId(from.0), reference);
        }
    }
}

/// Apply queued host events on the UI thread. Call once at the root.
pub fn use_host_pump() {
    use_hook(|| {
        let pump = shell_host().pump.clone();
        spawn_forever(async move {
            let mut jobs = HashMap::new();
            while let Ok(event) = pump.recv_async().await {
                apply(event, &mut jobs);
            }
        });
    });
}
