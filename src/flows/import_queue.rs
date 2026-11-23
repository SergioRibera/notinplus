//! Files that arrive several at a time (dropped on the window, opened
//! with the app) go through here, one sheet at a time.
//!
//! The next file waits until no sheet is open: a sheet the person is
//! filling is never replaced, whatever opened it. Each import sheet says
//! where it stands ("2 of 5") and can cancel the rest; nothing opens
//! after an import, and one toast at the end says what came in and what
//! could not.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use freya::prelude::*;
use notin_i18n::tr;
use notin_ui_kit::{ModalController, Toast, ToastTone, Toasts};

use super::create::{Context, Outcome, QueueSlot};
use crate::files::Picked;
use crate::library::FolderId;

/// How often a waiting queue looks whether the open sheet closed.
const IDLE_POLL: Duration = Duration::from_millis(120);
/// Pause between one sheet closing and the next opening, so the click
/// or key that closed the first one cannot land on the second.
const SETTLE: Duration = Duration::from_millis(350);

/// One thing to bring in.
#[derive(Debug)]
pub enum Pending {
    /// A file on disk, read when its turn comes.
    Path(PathBuf),
    /// A file already read (opened with the app).
    Read(Picked),
    /// A folder whose documents are imported as a folder tree.
    Tree(PathBuf),
    /// A file dropped on an open document, which takes it in.
    IntoDocument {
        path: PathBuf,
        document: crate::library::ItemId,
    },
}

/// Who handed the files over, which decides what they may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Dropped on the window: documents and `.notin` packages, never a
    /// plugin.
    Dropped,
    /// Opened with the app: a plugin package goes to its review too.
    Opened,
}

#[derive(Debug)]
struct Waiting {
    item: Pending,
    context: Context,
    source: Source,
}

#[derive(Default)]
struct Queue {
    waiting: VecDeque<Waiting>,
    running: bool,
    /// Size of the current run, for "2 of 5".
    total: usize,
    position: usize,
    imported: usize,
    /// Where the last import of the run went, for "View".
    folder: Option<FolderId>,
    /// Names of files nothing could take.
    refused: Vec<String>,
}

thread_local! {
    static QUEUE: RefCell<Queue> = RefCell::new(Queue::default());
}

fn with<T>(f: impl FnOnce(&mut Queue) -> T) -> T {
    QUEUE.with(|queue| f(&mut queue.borrow_mut()))
}

/// Bring `items` in, after whatever is queued already. `refused` names
/// what the caller already knows nothing can take.
#[sismo::function("flows::import_queue_add")]
pub fn add(items: Vec<Pending>, context: &Context, source: Source, refused: Vec<String>) {
    let start = with(|queue| {
        queue.total += items.len();
        queue.refused.extend(refused);
        queue.waiting.extend(items.into_iter().map(|item| Waiting {
            item,
            context: context.clone(),
            source,
        }));
        !std::mem::replace(&mut queue.running, true)
    });
    if start {
        spawn_in_window(traced!("task::flows::import_queue", run()));
    }
}

/// Drop what waits after the current file.
fn cancel_rest() {
    with(|queue| queue.waiting.clear());
}

#[expect(clippy::future_not_send, reason = "runs on the UI executor")]
async fn run() {
    loop {
        wait_idle().await;
        let Some(next) = with(|queue| {
            let next = queue.waiting.pop_front()?;
            queue.position += 1;
            Some((next, queue.position, queue.total))
        }) else {
            break;
        };
        let (waiting, position, total) = next;
        take(waiting, position, total).await;
    }
    finish();
}

/// Until no sheet is open, and a moment after.
async fn wait_idle() {
    loop {
        while ModalController::get().is_open() {
            async_io::Timer::after(IDLE_POLL).await;
        }
        async_io::Timer::after(SETTLE).await;
        if !ModalController::get().is_open() {
            return;
        }
    }
}

#[sismo::function("flows::import_queue_take")]
#[expect(clippy::future_not_send, reason = "runs on the UI executor")]
async fn take(waiting: Waiting, position: usize, total: usize) {
    let Waiting {
        item,
        context,
        source,
    } = waiting;
    let picked = match item {
        Pending::Tree(path) => {
            let imported = super::import_tree::run(path, context.folder).await;
            record(imported, context.folder);
            return;
        }
        Pending::IntoDocument { path, document } => {
            super::add_to_document::run(path, document, position, total).await;
            return;
        }
        Pending::Read(picked) => picked,
        Pending::Path(path) => {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            match crate::opened_files::read(path).await {
                Ok(bytes) => Picked { name, bytes },
                Err(err) => {
                    crate::opened_files::failed(
                        &name,
                        &notin_plugin::UserMessage::user_message(&err),
                    );
                    return;
                }
            }
        }
    };
    match crate::opened_files::ExternalKind::from(picked.name.as_str()) {
        crate::opened_files::ExternalKind::Bundle => {
            super::transfer::review_bundle(picked).await;
        }
        crate::opened_files::ExternalKind::Plugin => match source {
            Source::Opened => crate::screens::settings::review_package(picked).await,
            Source::Dropped => {
                Toasts::get().show(
                    Toast::new(tr!("flows-import-queue-plugin", "name" => picked.name))
                        .tone(ToastTone::Error),
                );
            }
        },
        crate::opened_files::ExternalKind::Document => {
            import(picked, context, position, total).await;
        }
    }
}

/// One document through its import sheet; returns once it ended.
async fn import(picked: Picked, context: Context, position: usize, total: usize) {
    if !matches_contents(&picked) {
        with(|queue| queue.refused.push(picked.name));
        return;
    }
    let name = picked.name.clone();
    let folder = context.folder;
    let (tx, rx) = flume::bounded(1);
    let slot = QueueSlot {
        position,
        total,
        on_done: Rc::new(move |outcome| {
            let _ = tx.try_send(outcome);
        }),
        cancel_rest: Rc::new(cancel_rest),
    };
    if !super::create::import_queued(picked, context, slot) {
        with(|queue| queue.refused.push(name));
        return;
    }
    // A sheet dropped without a word (the plugin went away) counts as
    // skipped: every sender is gone.
    if let Ok(Outcome::Imported(_)) = rx.recv_async().await {
        record(1, folder);
    }
}

fn record(imported: usize, folder: FolderId) {
    if imported > 0 {
        with(|queue| {
            queue.imported += imported;
            queue.folder = Some(folder);
        });
    }
}

/// A file whose name says PDF must hold one: a renamed file goes to no
/// importer that would misread it.
fn matches_contents(picked: &Picked) -> bool {
    let pdf = std::path::Path::new(&picked.name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
    !pdf || picked.bytes.starts_with(b"%PDF-")
}

/// One toast for the whole run, then a clean slate.
fn finish() {
    let queue = with(std::mem::take);
    let mut toasts = Toasts::get();
    if queue.imported > 0 {
        let mut toast = Toast::new(tr!("flows-import-queue-imported", "count" => queue.imported));
        if let Some(folder) = queue.folder {
            toast = toast.action(tr!("common-see"), move |()| {
                crate::screens::home::open_folder(folder);
            });
        }
        toasts.show(toast);
    }
    if !queue.refused.is_empty() {
        toasts.show(
            Toast::new(tr!(
                "flows-import-queue-refused",
                "count" => queue.refused.len(),
                "names" => queue.refused.join(", ")
            ))
            .tone(ToastTone::Error),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_renamed_file_is_not_a_pdf() {
        let picked = |name: &str, bytes: &[u8]| Picked {
            name: name.into(),
            bytes: bytes.to_vec(),
        };
        assert!(matches_contents(&picked("a.pdf", b"%PDF-1.7 ...")));
        assert!(!matches_contents(&picked("a.PDF", b"PK\x03\x04")));
        assert!(matches_contents(&picked("a.md", b"# hi")));
    }
}
