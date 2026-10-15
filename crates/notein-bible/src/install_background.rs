//! Fire-and-forget installer thread.
//!
//! `set_config(KEY_INSTALL_BIBLE, "spa_rv1960")` from the shell lands
//! here. We spawn a dedicated OS thread, load the catalogue, resolve
//! the per-Bible endpoints and populate the shared [`crate::db::Db`].
//! Progress is written to a shared [`InstallProgress`] slot that the
//! settings UI polls.
//!
//! Gated behind the `importer` feature — the HTTP stack only pulls in
//! when the host wants real downloads.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use notein_sdk::{NotificationKind, Progress as HostProgress};
use reqwest::blocking::Client;

use crate::db::Db;
use crate::import::biblionlabs;
use crate::plugin_config::{self, SharedConfig};

const PLUGIN_ID: &str = "io.notein.bible";
const PROGRESS_TOKEN: &str = "install";

/// 7-day TTL on cached manifests + per-book files. The installer walks
/// the biblionlabs cache dir in the same shape the service does; a stale
/// catalogue triggers a re-fetch so new translations show up without
/// nuking the on-disk cache.
const CACHE_TTL: Duration = Duration::from_secs(60 * 60 * 24 * 7);

/// Snapshot of the active install (or the last finished one). Written
/// by the installer thread, read by the settings panel.
#[derive(Clone, Debug, Default)]
pub struct InstallProgress {
    pub bible_id: String,
    pub stage: InstallStage,
    pub done_books: usize,
    pub total_books: usize,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum InstallStage {
    #[default]
    Idle,
    Starting,
    Downloading,
    Finished,
    Failed,
}

/// Shared handle. The canvas / source side exposes this to the UI so
/// the settings panel can observe installer activity without re-walking
/// the cache dir.
pub type SharedInstallProgress = Arc<RwLock<InstallProgress>>;

#[must_use]
pub fn new_shared_progress() -> SharedInstallProgress {
    Arc::new(RwLock::new(InstallProgress::default()))
}

/// Spawn an install thread. Returns immediately. Safe to call multiple
/// times — each call races; the shared progress reflects the last one
/// to update.
pub fn spawn_install(
    bible_id: String,
    db: Db,
    config: SharedConfig,
    progress: SharedInstallProgress,
) {
    std::thread::Builder::new()
        .name(format!("bible-install-{bible_id}"))
        .spawn(move || run(bible_id, db, config, progress))
        .expect("failed to spawn bible installer thread");
}

fn run(bible_id: String, db: Db, config: SharedConfig, progress: SharedInstallProgress) {
    {
        let mut p = progress.write().unwrap();
        p.bible_id = bible_id.clone();
        p.stage = InstallStage::Starting;
        p.done_books = 0;
        p.total_books = 0;
        p.error = None;
    }

    let host = plugin_config::host(&config);
    if let Some(h) = &host {
        h.report_progress(
            PLUGIN_ID,
            PROGRESS_TOKEN,
            HostProgress::Starting {
                label: format!("Descargando {bible_id}…"),
                total: None,
            },
        );
    }

    let client = match Client::builder().user_agent("notein-bible/0.1").build() {
        Ok(c) => c,
        Err(e) => {
            fail(&progress, host.as_ref(), format!("http client build: {e}"));
            return;
        }
    };

    let cache_dir = plugin_config::effective_cache_dir(&config);
    if let Err(e) = std::fs::create_dir_all(&cache_dir) {
        fail(&progress, host.as_ref(), format!("mkdir cache: {e}"));
        return;
    }

    let manifest_url = plugin_config::effective_manifest_url(&config);
    let base = base_for(&manifest_url);
    let desc_url = format!("{base}/bibles/{bible_id}/extra.json");
    let books_template = format!("{base}/bibles/{{bible_id}}/books/{{book}}.json");

    let progress_cb_handle = progress.clone();
    let host_cb = host.clone();
    let progress_cb = move |ev: biblionlabs::Progress| {
        let mut p = progress_cb_handle.write().unwrap();
        match ev {
            biblionlabs::Progress::ManifestFetched { total_books } => {
                p.stage = InstallStage::Downloading;
                p.total_books = total_books;
                if let Some(h) = &host_cb {
                    h.report_progress(
                        PLUGIN_ID,
                        PROGRESS_TOKEN,
                        HostProgress::Update {
                            done: 0,
                            total: Some(total_books as u64),
                            label: Some("Descargando libros…".into()),
                        },
                    );
                }
            }
            biblionlabs::Progress::BookDone { .. } => {
                p.done_books += 1;
                if let Some(h) = &host_cb {
                    h.report_progress(
                        PLUGIN_ID,
                        PROGRESS_TOKEN,
                        HostProgress::Update {
                            done: p.done_books as u64,
                            total: Some(p.total_books as u64),
                            label: None,
                        },
                    );
                }
            }
            biblionlabs::Progress::Started { .. } | biblionlabs::Progress::Finished { .. } => {}
        }
    };

    let result = biblionlabs::install_bible(
        &db,
        &client,
        &bible_id,
        &cache_dir,
        &desc_url,
        &books_template,
        CACHE_TTL,
        progress_cb,
    );

    match result {
        Ok(verses) => {
            let mut p = progress.write().unwrap();
            p.stage = InstallStage::Finished;
            p.error = None;
            drop(p);
            if let Some(h) = &host {
                h.report_progress(
                    PLUGIN_ID,
                    PROGRESS_TOKEN,
                    HostProgress::Finished { success: true, note: None },
                );
                h.notify(
                    PLUGIN_ID,
                    NotificationKind::Success,
                    format!("Biblia {bible_id} instalada ({verses} versículos)"),
                );
            }
        }
        Err(e) => fail(&progress, host.as_ref(), format!("{e}")),
    }
}

fn fail(progress: &SharedInstallProgress, host: Option<&notein_sdk::Host>, message: String) {
    log::warn!("bible: install failed: {message}");
    let mut p = progress.write().unwrap();
    p.stage = InstallStage::Failed;
    p.error = Some(message.clone());
    drop(p);
    if let Some(h) = host {
        h.report_progress(
            PLUGIN_ID,
            PROGRESS_TOKEN,
            HostProgress::Finished {
                success: false,
                note: Some(message.clone()),
            },
        );
        h.notify(
            PLUGIN_ID,
            NotificationKind::Error,
            format!("Instalación falló: {message}"),
        );
    }
}

/// Derive the catalogue base URL from a `.../manifest.json` endpoint.
/// Falls back to the fetch.bible root if the URL does not end in
/// `/manifest.json`.
fn base_for(manifest_url: &str) -> String {
    manifest_url
        .strip_suffix("/manifest.json")
        .map(str::to_string)
        .unwrap_or_else(|| "https://v1.fetch.bible".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_for_strips_manifest_suffix() {
        assert_eq!(
            base_for("https://v1.fetch.bible/manifest.json"),
            "https://v1.fetch.bible"
        );
        assert_eq!(
            base_for("https://mirror.local/bible/manifest.json"),
            "https://mirror.local/bible"
        );
        assert_eq!(
            base_for("https://other.example"),
            "https://v1.fetch.bible"
        );
    }
}
