//! The host handed to source plugins: everything they report goes
//! through the shell host ([`crate::plugins::host`]) — toasts, progress,
//! fonts and jumps — so there is one place that shows it.

use std::sync::{Arc, Mutex, OnceLock};

use notein_plugin::services::{HostServices, HostToast, Progress as JobProgress, ToastTone};
use notein_plugin::{
    BookmarkEditCallback, BookmarkEditRequest, Host, HostApi, NotificationKind, Progress,
};

/// Source-plugin host.
#[derive(Debug, Default)]
pub struct SourceHost {
    /// Label of each running job, kept so updates without one still read
    /// well.
    labels: Mutex<std::collections::HashMap<(String, String), String>>,
}

static HOST: OnceLock<Arc<SourceHost>> = OnceLock::new();

/// The source-plugin host handle.
#[must_use]
pub fn host_handle() -> Host {
    Arc::clone(HOST.get_or_init(Arc::default)) as Host
}

fn shell() -> Arc<crate::plugins::host::ShellHost> {
    crate::plugins::host::shell()
}

impl HostApi for SourceHost {
    fn report_progress(&self, plugin_id: &str, token: &str, progress: Progress) {
        let key = (plugin_id.to_owned(), token.to_owned());
        let mut labels = self.labels.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let job = match progress {
            Progress::Starting { label, total } => {
                labels.insert(key, label.clone());
                JobProgress::Running { label, done: 0, total }
            }
            Progress::Update { done, total, label } => {
                let label = match label {
                    Some(label) => {
                        labels.insert(key, label.clone());
                        label
                    }
                    None => labels.get(&key).cloned().unwrap_or_default(),
                };
                JobProgress::Running { label, done, total }
            }
            Progress::Finished { success, note } => {
                labels.remove(&key);
                JobProgress::Finished { success, note }
            }
        };
        drop(labels);
        shell().progress(plugin_id, token, job);
    }

    fn notify(&self, _plugin_id: &str, kind: NotificationKind, message: String) {
        let tone = match kind {
            NotificationKind::Info | NotificationKind::Warning => ToastTone::Info,
            NotificationKind::Success => ToastTone::Done,
            NotificationKind::Error => ToastTone::Error,
        };
        shell().toast(HostToast {
            text: message,
            tone,
            action: None,
        });
    }

    fn open_url(&self, url: &str) {
        shell().open_url(url);
    }

    fn load_font(&self, font_name: &str, font_data: Vec<u8>) {
        shell().load_font(font_name, font_data);
    }

    fn request_bookmark_edit(&self, plugin_id: &str, request: BookmarkEditRequest, callback: BookmarkEditCallback) {
        use notein_plugin::services::{AnnotationOutcome, AnnotationRequest};
        let draft = request.draft.clone();
        let done: notein_plugin::services::AnnotationDone = Arc::new(move |outcome| match outcome {
            AnnotationOutcome::Saved { color, body } => callback(Some(notein_plugin::BookmarkDraft {
                body,
                color: color.map(|c| [c[0], c[1], c[2], 0xFF]),
                refs: draft.refs.clone(),
            })),
            AnnotationOutcome::Deleted | AnnotationOutcome::Cancelled => callback(None),
        });
        shell().annotate(
            AnnotationRequest {
                doc: notein_plugin::doc::DocId([0; 16]),
                kind: plugin_id.to_owned(),
                title: request.title.to_string(),
                quote: None,
                color: request.draft.color.map(|c| [c[0], c[1], c[2]]),
                body: request.draft.body.clone(),
                target: request.anchor.map(|(x, y)| (x, y, 1.0, 1.0)),
                existing: !request.draft.body.is_empty(),
            },
            None,
            done,
        );
    }

    fn queue_source_anchor(&self, plugin_id: &str, item: [u8; 16], payload: Vec<u8>) {
        shell().queue_jump(plugin_id, notein_plugin::doc::DocId(item), payload);
    }

    fn take_source_anchor(&self, plugin_id: &str, item: [u8; 16]) -> Option<Vec<u8>> {
        shell().take_jump(plugin_id, notein_plugin::doc::DocId(item))
    }

    fn subscribe_source_anchor(&self, plugin_id: &str) -> flume::Receiver<[u8; 16]> {
        let jumps = shell().jumps(plugin_id);
        let (tx, rx) = flume::unbounded();
        let spawned = std::thread::Builder::new()
            .name("source-anchor-fanout".to_owned())
            .spawn(move || {
                while let Ok(jump) = jumps.recv() {
                    if tx.send(jump.doc.0).is_err() {
                        break;
                    }
                }
            });
        if let Err(err) = spawned {
            log::warn!("source anchors: {err}");
        }
        rx
    }
}

/// Hand the host to every registered canvas and source plugin.
pub fn install_host_in_registry(registry: &notein_plugin::PluginRegistry) {
    let handle = host_handle();
    for canvas in registry.canvases() {
        canvas.install_host(Arc::clone(&handle));
    }
    for source in registry.sources() {
        source.install_host(Arc::clone(&handle));
    }
}
