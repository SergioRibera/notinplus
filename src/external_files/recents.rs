//! Files opened from outside the library lately, kept on this device
//! only, so Home can offer to continue with them.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use freya::prelude::*;
use notein_i18n::tr;
use notein_plugin::UserMessage;
use notein_ui_kit::{Toast, ToastTone, Toasts};

use super::{ExternalFile, FileKind};
use crate::persist::Persisted;

/// Files remembered at most.
const LIMIT: usize = 20;

/// A file opened from outside the library.
#[istmo::message]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recent {
    pub path: String,
    pub name: String,
    /// Unix millis of the last time it was opened.
    pub opened_at: u64,
}

impl Recent {
    /// What the view shows of it.
    #[must_use]
    pub fn kind(&self) -> Option<FileKind> {
        FileKind::of(&self.name)
    }

    /// Name of the folder it was in.
    #[must_use]
    pub fn folder(&self) -> String {
        Path::new(&self.path)
            .parent()
            .and_then(Path::file_name)
            .map_or_else(
                || self.path.clone(),
                |name| name.to_string_lossy().into_owned(),
            )
    }
}

/// The remembered files, newest first.
#[istmo::message]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Recents {
    pub(super) files: Vec<Recent>,
}

impl Persisted for Recents {
    const FILE: &'static str = "external-recents.bin";
}

impl Recents {
    /// Put `path` first, opened at `now`.
    fn remember(&mut self, path: String, name: String, now: u64) {
        self.files.retain(|recent| recent.path != path);
        self.files.insert(
            0,
            Recent {
                path,
                name,
                opened_at: now,
            },
        );
        self.files.truncate(LIMIT);
    }

    /// Drop `path`. Whether it was there.
    fn forget(&mut self, path: &str) -> bool {
        let before = self.files.len();
        self.files.retain(|recent| recent.path != path);
        self.files.len() != before
    }

    /// Load, change with `edit` and store, telling the screens that show
    /// the list when it changed.
    fn update(edit: impl FnOnce(&mut Self) -> bool) {
        let mut recents = Self::load();
        if !edit(&mut recents) {
            return;
        }
        if let Err(err) = recents.save() {
            log::warn!("external recents: {err}");
        }
        notify();
    }
}

static LISTENERS: Mutex<Vec<flume::Sender<()>>> = Mutex::new(Vec::new());

fn notify() {
    LISTENERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|tx| tx.send(()).is_ok());
}

/// Remember that `file` was opened.
pub(super) fn remember(file: &ExternalFile) {
    let path = file.path.to_string_lossy().into_owned();
    let name = file.name.clone();
    Recents::update(move |recents| {
        recents.remember(path, name, crate::format::now());
        true
    });
}

/// Stop offering `path` (it is in the library now, or gone).
pub(super) fn forget(path: &Path) {
    let path = path.to_string_lossy().into_owned();
    Recents::update(move |recents| recents.forget(&path));
}

/// The remembered files, kept up to date.
#[must_use]
pub fn use_recents() -> State<Arc<Vec<Recent>>> {
    let mut recents = use_state(|| Arc::new(Recents::load().files));
    use_hook(move || {
        let (tx, rx) = flume::unbounded();
        LISTENERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tx);
        spawn(async move {
            while rx.recv_async().await.is_ok() {
                while rx.try_recv().is_ok() {}
                recents.set(Arc::new(Recents::load().files));
            }
        });
    });
    recents
}

/// Open `recent` again from where it was, or say it is gone and forget it.
#[sismo::function("external_files::reopen")]
pub fn reopen(recent: &Recent) {
    let recent = recent.clone();
    spawn_in_window(async move {
        let path = PathBuf::from(&recent.path);
        if !path.is_file() {
            Toasts::get().show(
                Toast::new(tr!("external-files-gone", "folder" => recent.folder()))
                    .tone(ToastTone::Error),
            );
            forget(&path);
            return;
        }
        match crate::opened_files::read(path.clone()).await {
            Ok(bytes) => {
                let picked = crate::files::Picked {
                    name: recent.name.clone(),
                    bytes,
                };
                if let Some(file) = ExternalFile::new(path, picked) {
                    super::show(file);
                }
            }
            Err(err) => {
                log::warn!("reopen {}: {err}", recent.path);
                crate::opened_files::failed(&recent.name, &err.user_message());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::Recents;

    #[test]
    fn newest_first_without_repeats_and_bounded() {
        let mut recents = Recents::default();
        for n in 0..25 {
            recents.remember(format!("/tmp/{n}.md"), format!("{n}.md"), n);
        }
        recents.remember("/tmp/7.md".to_owned(), "7.md".to_owned(), 99);
        assert_eq!(recents.files.len(), super::LIMIT);
        assert_eq!(recents.files[0].path, "/tmp/7.md");
        assert_eq!(
            recents
                .files
                .iter()
                .filter(|r| r.path == "/tmp/7.md")
                .count(),
            1
        );
        assert!(recents.forget("/tmp/7.md"));
        assert!(!recents.forget("/tmp/7.md"));
    }
}
