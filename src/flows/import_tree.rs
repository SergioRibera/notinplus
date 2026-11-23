//! A folder dropped on the library comes in as a folder tree: one review
//! sheet for all of it, then every document it holds, each into a
//! library folder named like the one it was in.
//!
//! Only documents some plugin imports are taken; folders with none, in
//! themselves or below, are not created. The walk never follows links,
//! skips hidden files and stops at [`MAX_DEPTH`] levels and
//! [`MAX_FILES`] documents.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use freya::prelude::*;
use notin_i18n::tr;
use notin_ui_kit::{
    Button, ModalController, Notice, NoticeTone, Sheet, SheetFooter, Text, TextRole, Toast,
    ToastTone, Toasts,
};

use crate::files::Picked;
use crate::library::FolderId;

/// Deepest folder walked into.
const MAX_DEPTH: usize = 16;
/// Most documents one tree brings.
const MAX_FILES: usize = 2000;
/// Names listed in the review before "and N more".
const LISTED_SKIPS: usize = 5;
/// Files systems leave in folders, never anyone's documents.
const SYSTEM_FILES: [&str; 3] = ["thumbs.db", "desktop.ini", "icon\r"];

/// A folder of the tree; index 0 is the dropped one.
#[derive(Debug)]
struct Dir {
    name: String,
    parent: Option<usize>,
}

#[derive(Debug)]
struct Found {
    dir: usize,
    path: PathBuf,
}

/// What a walk of the dropped folder found.
#[derive(Debug, Default)]
struct Tree {
    dirs: Vec<Dir>,
    files: Vec<Found>,
    /// Names of files no plugin imports (or too large).
    skipped: Vec<String>,
    /// Stopped at [`MAX_FILES`] or [`MAX_DEPTH`].
    truncated: bool,
}

impl Tree {
    /// Walk `root`, keeping files whose extension is in `importable`.
    fn walk(root: &Path, importable: &HashSet<String>) -> Self {
        let mut tree = Self::default();
        tree.dirs.push(Dir {
            name: display_name(root),
            parent: None,
        });
        let mut pending = vec![(root.to_path_buf(), 0_usize, 0_usize)];
        while let Some((path, index, depth)) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            let mut entries: Vec<_> = entries.flatten().collect();
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') || SYSTEM_FILES.contains(&name.to_lowercase().as_str()) {
                    continue;
                }
                // Not followed: a link may lead out of the tree or round
                // in a loop.
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    if depth + 1 >= MAX_DEPTH {
                        tree.truncated = true;
                        continue;
                    }
                    tree.dirs.push(Dir {
                        name,
                        parent: Some(index),
                    });
                    pending.push((entry.path(), tree.dirs.len() - 1, depth + 1));
                } else if kind.is_file() {
                    if !importable.contains(&extension(&name)) {
                        tree.skipped.push(name);
                    } else if tree.files.len() >= MAX_FILES {
                        tree.truncated = true;
                    } else {
                        tree.files.push(Found {
                            dir: index,
                            path: entry.path(),
                        });
                    }
                }
            }
        }
        tree
    }

    /// Which folders hold a document, in themselves or below.
    fn needed(&self) -> Vec<bool> {
        let mut needed = vec![false; self.dirs.len()];
        for file in &self.files {
            let mut dir = Some(file.dir);
            while let Some(index) = dir {
                if std::mem::replace(&mut needed[index], true) {
                    break;
                }
                dir = self.dirs[index].parent;
            }
        }
        needed
    }
}

fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// Extensions some plugin imports.
fn importable() -> HashSet<String> {
    crate::plugins::catalog()
        .entries()
        .into_iter()
        .filter_map(|entry| {
            if let notin_plugin::create::CreationFlow::Import { extensions, .. } = entry.entry.flow
            {
                Some(extensions)
            } else {
                None
            }
        })
        .flatten()
        .map(|ext| ext.to_ascii_lowercase())
        .collect()
}

/// Review the tree under `root` and, once the person agrees, import it
/// into a new folder inside `parent`. Returns how many documents came
/// in.
#[sismo::function("flows::import_tree")]
#[expect(clippy::future_not_send, reason = "runs on the UI executor")]
pub(super) async fn run(root: PathBuf, parent: FolderId) -> usize {
    let importable = importable();
    let (tx, rx) = flume::bounded(1);
    let walked = std::thread::Builder::new()
        .name("import-tree-walk".into())
        .spawn({
            let root = root.clone();
            move || {
                let _ = tx.send(Tree::walk(&root, &importable));
            }
        });
    if let Err(err) = walked {
        tracing::warn!("import tree: {err}");
        return 0;
    }
    let Ok(tree) = rx.recv_async().await else {
        return 0;
    };
    let name = display_name(&root);
    if tree.files.is_empty() {
        Toasts::get().show(
            Toast::new(tr!("flows-import-tree-empty", "name" => name)).tone(ToastTone::Error),
        );
        return 0;
    }
    if !review(&name, &tree).await {
        return 0;
    }
    import(tree, parent).await
}

/// One sheet for the whole tree; `true` when the person imports.
async fn review(name: &str, tree: &Tree) -> bool {
    let folders = tree.needed().iter().filter(|needed| **needed).count();
    let documents = tree.files.len();
    let (tx, rx) = flume::bounded(1);
    let cancelled = tx.clone();
    let skipped = (!tree.skipped.is_empty()).then(|| {
        let listed = tree.skipped[..tree.skipped.len().min(LISTED_SKIPS)].join(", ");
        let more = tree.skipped.len().saturating_sub(LISTED_SKIPS);
        Notice::new(
            NoticeTone::Info,
            tr!("flows-import-tree-skipped", "count" => tree.skipped.len()),
        )
        .detail(if more > 0 {
            tr!("flows-import-tree-skipped-more", "names" => listed, "more" => more)
        } else {
            listed
        })
    });
    Sheet::titled(tr!("flows-import-tree-title", "name" => name.to_owned()))
        .on_close(move |()| {
            let _ = cancelled.try_send(false);
        })
        .child(
            rect()
                .width(Size::fill())
                .spacing(12.0)
                .child(Text::new(tr!(
                    "flows-import-tree-summary",
                    "documents" => documents,
                    "folders" => folders
                )))
                .maybe_child(skipped)
                .maybe_child(tree.truncated.then(|| {
                    Text::new(tr!("flows-import-tree-truncated", "max" => MAX_FILES))
                        .role(TextRole::Hint)
                })),
        )
        .footer(SheetFooter::new([
            Button::new(tr!("common-cancel"))
                .ghost()
                .on_press(|_| ModalController::get().close())
                .into_element(),
            Button::new(tr!("flows-import-tree-submit", "count" => documents))
                .primary()
                .on_press(move |_| {
                    let mut modal = ModalController::get();
                    modal.clear_top_on_close();
                    modal.close();
                    let _ = tx.try_send(true);
                })
                .into_element(),
        ]))
        .push();
    rx.recv_async().await.unwrap_or(false)
}

/// Create the folders and import every document, with a progress toast.
#[expect(clippy::future_not_send, reason = "runs on the UI executor")]
async fn import(tree: Tree, parent: FolderId) -> usize {
    let Ok(handle) = crate::library::shared().await else {
        return 0;
    };
    let needed = tree.needed();
    let mut folders: Vec<Option<FolderId>> = vec![None; tree.dirs.len()];
    for (index, dir) in tree.dirs.iter().enumerate() {
        if !needed[index] {
            continue;
        }
        // Parents come before their children in `dirs`.
        let Some(inside) = dir.parent.map_or(Some(parent), |p| folders[p]) else {
            continue;
        };
        let created = handle
            .lock()
            .await
            .create_folder(inside, &dir.name, None, Vec::new())
            .await;
        match created {
            Ok(folder) => folders[index] = Some(folder),
            Err(err) => tracing::warn!("import tree folder {}: {err}", dir.name),
        }
    }

    let total = tree.files.len();
    let mut toasts = Toasts::get();
    let progress = |done: usize| {
        #[expect(clippy::cast_precision_loss, reason = "a fraction for a bar")]
        let fraction = done as f32 / total.max(1) as f32;
        Toast::new(tr!("flows-import-tree-importing")).progress(
            fraction,
            tr!("flows-import-tree-progress", "done" => done, "total" => total),
        )
    };
    let toast = toasts.show(progress(0));
    let mut imported = 0;
    let mut failed = Vec::new();
    for (done, file) in tree.files.into_iter().enumerate() {
        let name = display_name(&file.path);
        let (Some(folder), Some(entry)) = (folders[file.dir], super::create::import_entry(&name))
        else {
            failed.push(name);
            continue;
        };
        let stored = match crate::opened_files::read(file.path).await {
            Ok(bytes) => {
                let picked = Picked {
                    name: name.clone(),
                    bytes,
                };
                super::create::import_quietly(&entry, folder, &picked).await
            }
            Err(err) => Err(notin_plugin::UserMessage::user_message(&err)),
        };
        match stored {
            Ok(_) => imported += 1,
            Err(why) => {
                tracing::warn!("import tree {name}: {why}");
                failed.push(name);
            }
        }
        toasts.update(toast, progress(done + 1));
    }
    toasts.dismiss(toast);
    if !failed.is_empty() {
        toasts.show(
            Toast::new(tr!(
                "flows-import-queue-refused",
                "count" => failed.len(),
                "names" => failed.join(", ")
            ))
            .tone(ToastTone::Error),
        );
    }
    imported
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_documents_and_keeps_folders_that_hold_some() {
        let root = std::env::temp_dir().join(format!("notin-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["a/b", "empty/deeper", ".hidden"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        for (file, body) in [
            ("top.pdf", "%PDF-"),
            ("a/b/note.md", "# n"),
            ("a/photo.heic", "x"),
            (".hidden/secret.md", "x"),
            ("empty/deeper/Thumbs.db", "x"),
        ] {
            std::fs::write(root.join(file), body).unwrap();
        }
        let importable: HashSet<String> = ["pdf", "md"].map(String::from).into();
        let tree = Tree::walk(&root, &importable);
        assert_eq!(tree.files.len(), 2);
        assert_eq!(tree.skipped, ["photo.heic"]);
        let needed: Vec<&str> = tree
            .dirs
            .iter()
            .zip(tree.needed())
            .filter(|(_, needed)| *needed)
            .map(|(dir, _)| dir.name.as_str())
            .collect();
        assert_eq!(
            needed.len(),
            3,
            "root, a and a/b; not empty/ nor .hidden: {needed:?}"
        );
        assert!(!needed.contains(&"empty"));
        let _ = std::fs::remove_dir_all(root);
    }
}
