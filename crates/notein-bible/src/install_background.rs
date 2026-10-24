//! Download threads: a translation install, run by
//! [`crate::installer`], and the lazy footnote / heading fetches for one
//! chapter. Each writes straight into the shared [`crate::db::Db`].
//!
//! Gated behind the `importer` feature: the HTTP stack only pulls in
//! when the host wants real downloads.

use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use reqwest::blocking::Client;

use crate::db::Db;
use crate::import::{biblionlabs, helloao};
use crate::plugin_config::{self, SharedConfig};

/// 7-day TTL on cached manifests + per-book files. The installer walks
/// the biblionlabs cache dir in the same shape the service does; a stale
/// catalogue triggers a re-fetch so new translations show up without
/// nuking the on-disk cache.
const CACHE_TTL: Duration = Duration::from_secs(60 * 60 * 24 * 7);

/// Snapshot of the active install (or the last finished one). Written
/// by the installer thread, read by the settings panel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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

/// Fetch (or load from on-disk cache) the biblionlabs catalogue.
/// Blocking HTTP; call from a worker thread. Reuses the install
/// cache dir so a prior install warms the manifest file.
pub fn load_catalogue(
    config: &SharedConfig,
) -> Result<biblionlabs::CatalogueView, biblionlabs::ImportError> {
    let client = http_client().map_err(biblionlabs::ImportError::from)?;
    let cache_dir = plugin_config::platform_cache_dir();
    std::fs::create_dir_all(&cache_dir)?;
    let manifest_url = plugin_config::effective_manifest_url(config);
    biblionlabs::load_catalogue(&client, &cache_dir, &manifest_url, CACHE_TTL)
}

/// Spawn a lazy footnote fetch for `(bible, book, chapter)`.
///
/// Idempotent — the reader fires one per chapter open, the on-disk cache +
/// the `footnotes` row count both short-circuit duplicate work. Silent on
/// failure (we log at debug) so a 404 or empty response does not surface a
/// user-visible error.
pub fn spawn_fetch_footnotes(db: Db, bible_id: String, book_id: String, chapter: u16) {
    std::thread::Builder::new()
        .name(format!("bible-footnotes-{book_id}-{chapter}"))
        .spawn(move || {
            if db
                .has_footnotes_for_chapter(&bible_id, &book_id, chapter)
                .unwrap_or(false)
            {
                return;
            }
            let client = match http_client() {
                Ok(c) => c,
                Err(e) => {
                    log::debug!("footnotes: client build: {e}");
                    return;
                }
            };
            let cache_dir = plugin_config::platform_cache_dir();
            if let Err(e) = std::fs::create_dir_all(&cache_dir) {
                log::debug!("footnotes: mkdir: {e}");
                return;
            }
            let helloao = helloao::Helloao {
                db: &db,
                client: &client,
                cache_dir: &cache_dir,
                cache_ttl: CACHE_TTL,
            };
            match helloao.footnotes_for_chapter(&bible_id, &book_id, chapter) {
                Ok(n) => {
                    log::debug!("footnotes: {book_id} {chapter} → {n} rows");
                }
                Err(e) => log::debug!("footnotes: fetch failed for {book_id} {chapter}: {e}"),
            }
        })
        .map_or_else(|e| log::warn!("footnotes thread: {e}"), drop);
}

/// Lazy section-heading fetch.
///
/// Hits a sibling helloao translation chosen by
/// [`helloao::helloao_headings_id_for_bible`] (e.g. RV1960 → `spa_pdt` for
/// the standard "Pablo es enviado a Roma" style apparatus) and inserts into the
/// `headers` table so the reader renders them inline between paragraphs.
/// Skipped when the chapter already has rows OR when no sibling id is mapped.
pub fn spawn_fetch_headings(
    db: Db,
    bible_id: String,
    book_id: String,
    chapter: u16,
) {
    std::thread::Builder::new()
        .name(format!("bible-headings-{book_id}-{chapter}"))
        .spawn(move || {
            if db
                .has_headers_for_chapter(&bible_id, &book_id, chapter)
                .unwrap_or(false)
            {
                return;
            }
            let Some(source) = helloao::helloao_headings_id_for_bible(&bible_id) else {
                return;
            };
            let client = match http_client() {
                Ok(c) => c,
                Err(e) => {
                    log::debug!("headings: client build: {e}");
                    return;
                }
            };
            let cache_dir = plugin_config::platform_cache_dir();
            if let Err(e) = std::fs::create_dir_all(&cache_dir) {
                log::debug!("headings: mkdir: {e}");
                return;
            }
            let helloao = helloao::Helloao {
                db: &db,
                client: &client,
                cache_dir: &cache_dir,
                cache_ttl: CACHE_TTL,
            };
            match helloao.headings_for_chapter(&bible_id, source, &book_id, chapter) {
                Ok(n) => log::debug!("headings: {book_id} {chapter} → {n} rows (via {source})"),
                Err(e) => log::debug!("headings: fetch failed for {book_id} {chapter}: {e}"),
            }
        })
        .map_or_else(|e| log::warn!("headings thread: {e}"), drop);
}


/// Raw biblionlabs layout: `bibles/<id>/manifest.json` holds the book
/// names and sections, `bibles/<id>/books/<book>.json` one book each.
const BIBLIONLABS_BASE: &str =
    "https://raw.githubusercontent.com/biblionlabs/extra_data_source/refs/heads/main";

fn http_client() -> reqwest::Result<Client> {
    Client::builder().user_agent("notein-bible/0.1").build()
}

/// Edit the shared progress slot; a writer that panicked leaves it usable.
fn edit(progress: &SharedInstallProgress, f: impl FnOnce(&mut InstallProgress)) {
    f(&mut progress.write().unwrap_or_else(PoisonError::into_inner));
}

/// Install `bible_id` into `db`, reporting through `progress`. Blocks
/// until the download, the verses and the section headings are done.
pub(crate) fn run(
    bible_id: &str,
    db: &Db,
    config: &SharedConfig,
    progress: &SharedInstallProgress,
) {
    log::info!("bible install thread: starting for {bible_id}");
    edit(progress, |p| {
        *p = InstallProgress {
            bible_id: bible_id.to_owned(),
            stage: InstallStage::Starting,
            ..InstallProgress::default()
        };
    });

    let client = match http_client() {
        Ok(c) => c,
        Err(e) => return fail(progress, &format!("http client build: {e}")),
    };
    let cache_dir = plugin_config::platform_cache_dir();
    if let Err(e) = std::fs::create_dir_all(&cache_dir) {
        return fail(progress, &format!("mkdir cache: {e}"));
    }

    let desc_url = format!("{BIBLIONLABS_BASE}/bibles/{bible_id}/manifest.json");
    let books_template = format!("{BIBLIONLABS_BASE}/bibles/{{bible_id}}/books/{{book}}.json");
    let meta = catalogue_meta(&client, &cache_dir, config, bible_id);
    let install = biblionlabs::Install {
        db,
        client: &client,
        bible_id,
        cache_dir: &cache_dir,
        desc_url: &desc_url,
        books_url_template: &books_template,
        cache_ttl: CACHE_TTL,
        meta: &meta,
    };
    let on_progress = |event: biblionlabs::Progress| {
        edit(progress, |p| match event {
            biblionlabs::Progress::ManifestFetched { total_books } => {
                p.stage = InstallStage::Downloading;
                p.total_books = total_books;
            }
            biblionlabs::Progress::BookDone { .. } => p.done_books += 1,
            biblionlabs::Progress::Started { .. } | biblionlabs::Progress::Finished { .. } => {}
        });
    };

    match install.run(on_progress) {
        Ok(verses) => {
            // Headings come right after the verses so the first passage
            // opened already shows them; the per-chapter fetch is only a
            // fallback.
            prefetch_all_headings(db, &client, &cache_dir, bible_id, progress);
            edit(progress, |p| {
                p.stage = InstallStage::Finished;
                p.error = None;
            });
            log::info!("bible install: {bible_id} done, {verses} verses");
        }
        Err(e) => fail(progress, &e.to_string()),
    }
}

/// Name, language and abbreviation of `bible_id` from the catalogue; the
/// per-Bible manifest leaves them out.
fn catalogue_meta(
    client: &Client,
    cache_dir: &std::path::Path,
    config: &SharedConfig,
    bible_id: &str,
) -> biblionlabs::BibleMeta {
    let manifest_url = plugin_config::effective_manifest_url(config);
    match biblionlabs::load_catalogue(client, cache_dir, &manifest_url, CACHE_TTL) {
        Ok(cat) => cat
            .bibles
            .iter()
            .find(|b| b.id == bible_id)
            .map(|b| biblionlabs::BibleMeta {
                name_local: b.name_local.clone(),
                name_english: b.name_english.clone(),
                language: b.language.clone(),
                abbrev: None,
            })
            .unwrap_or_default(),
        Err(e) => {
            log::warn!("bible install: catalogue lookup failed, the name falls back to the id: {e}");
            biblionlabs::BibleMeta::default()
        }
    }
}

/// Section headings for every chapter of the new Bible, from its helloao
/// sibling translation.
///
/// Each chapter is rewritten whole and cached on disk, so running it again
/// is cheap. A chapter that fails is logged and skipped.
fn prefetch_all_headings(
    db: &Db,
    client: &Client,
    cache_dir: &std::path::Path,
    bible_id: &str,
    progress: &SharedInstallProgress,
) {
    let Some(source) = helloao::helloao_headings_id_for_bible(bible_id) else {
        log::debug!("headings prefetch: no sibling id for {bible_id}; skipping");
        return;
    };
    // Restart the counters so the progress moves through this pass too.
    edit(progress, |p| {
        p.stage = InstallStage::Downloading;
        p.done_books = 0;
        p.total_books = 66;
    });
    let helloao = helloao::Helloao {
        db,
        client,
        cache_dir,
        cache_ttl: CACHE_TTL,
    };
    let result = helloao.all_headings(bible_id, source, |done, total, _book_name| {
            edit(progress, |p| {
                p.done_books = done;
                p.total_books = total;
            });
        },
    );
    match result {
        Ok(n) => log::info!("headings prefetch: {bible_id} inserted {n} rows via {source}"),
        Err(e) => log::warn!("headings prefetch: {bible_id} failed: {e}"),
    }
}

fn fail(progress: &SharedInstallProgress, message: &str) {
    log::error!("bible: install failed: {message}");
    edit(progress, |p| {
        p.stage = InstallStage::Failed;
        p.error = Some(message.to_owned());
    });
}
