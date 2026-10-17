//! Fire-and-forget installer thread.
//!
//! The install modal calls [`crate::BibleCanvas::request_install`],
//! which lands here. We spawn a dedicated OS thread, load the
//! catalogue, resolve the per-Bible endpoints and populate the shared
//! [`crate::db::Db`]. Progress is written to a shared
//! [`InstallProgress`] slot that the modal polls.
//!
//! Gated behind the `importer` feature — the HTTP stack only pulls in
//! when the host wants real downloads.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use notein_sdk::{NotificationKind, Progress as HostProgress};
use reqwest::blocking::Client;

use crate::db::Db;
use crate::import::{biblionlabs, helloao};
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
    let client = Client::builder()
        .user_agent("notein-bible/0.1")
        .build()
        .map_err(biblionlabs::ImportError::from)?;
    let cache_dir = plugin_config::platform_cache_dir();
    std::fs::create_dir_all(&cache_dir)?;
    let manifest_url = plugin_config::effective_manifest_url(config);
    biblionlabs::load_catalogue(&client, &cache_dir, &manifest_url, CACHE_TTL)
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

/// Spawn a lazy footnote fetch for `(bible, book, chapter)`. Idempotent
/// — the reader fires one per chapter open, the on-disk cache + the
/// `footnotes` row count both short-circuit duplicate work. Silent on
/// failure (we log at debug) so a 404 or empty response does not
/// surface a user-visible error.
pub fn spawn_fetch_footnotes(db: Db, config: SharedConfig, bible_id: String, book_id: String, chapter: u16) {
    std::thread::Builder::new()
        .name(format!("bible-footnotes-{book_id}-{chapter}"))
        .spawn(move || {
            if db
                .has_footnotes_for_chapter(&bible_id, &book_id, chapter)
                .unwrap_or(false)
            {
                return;
            }
            let client = match Client::builder().user_agent("notein-bible/0.1").build() {
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
            match helloao::install_footnotes_for_chapter(
                &db,
                &client,
                &cache_dir,
                &bible_id,
                &book_id,
                chapter,
                CACHE_TTL,
            ) {
                Ok(n) => {
                    log::debug!("footnotes: {book_id} {chapter} → {n} rows");
                }
                Err(e) => log::debug!("footnotes: fetch failed for {book_id} {chapter}: {e}"),
            }
        })
        .expect("failed to spawn footnotes fetch thread");
}

/// Lazy section-heading fetch. Hits a sibling helloao translation
/// chosen by [`helloao::helloao_headings_id_for_bible`] (e.g. RV1960
/// → `spa_pdt` for the standard "Pablo es enviado a Roma" style
/// apparatus) and inserts into the `headers` table so the reader
/// renders them inline between paragraphs. Skipped when the chapter
/// already has rows OR when no sibling id is mapped.
pub fn spawn_fetch_headings(
    db: Db,
    config: SharedConfig,
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
            let client = match Client::builder().user_agent("notein-bible/0.1").build() {
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
            match helloao::install_headings_for_chapter(
                &db,
                &client,
                &cache_dir,
                &bible_id,
                source,
                &book_id,
                chapter,
                CACHE_TTL,
            ) {
                Ok(n) => log::debug!("headings: {book_id} {chapter} → {n} rows (via {source})"),
                Err(e) => log::debug!("headings: fetch failed for {book_id} {chapter}: {e}"),
            }
        })
        .expect("failed to spawn headings fetch thread");
}


fn run(bible_id: String, db: Db, config: SharedConfig, progress: SharedInstallProgress) {
    log::info!("bible install thread: starting for {bible_id}");
    {
        let mut p = progress.write().unwrap();
        p.bible_id = bible_id.clone();
        p.stage = InstallStage::Starting;
        p.done_books = 0;
        p.total_books = 0;
        p.error = None;
    }

    let host = plugin_config::host(&config);
    if host.is_none() {
        log::warn!("bible install: host handle not installed; progress + notifications disabled");
    }
    if let Some(h) = &host {
        h.report_progress(
            PLUGIN_ID,
            PROGRESS_TOKEN,
            HostProgress::Starting {
                label: format!("Descargando {bible_id}…"),
                total: None,
            },
        );
        h.notify(
            PLUGIN_ID,
            NotificationKind::Info,
            format!("Descarga iniciada: {bible_id}"),
        );
    }

    let client = match Client::builder().user_agent("notein-bible/0.1").build() {
        Ok(c) => c,
        Err(e) => {
            fail(&progress, host.as_ref(), format!("http client build: {e}"));
            return;
        }
    };

    let cache_dir = plugin_config::platform_cache_dir();
    log::info!("bible install: cache_dir={}", cache_dir.display());
    if let Err(e) = std::fs::create_dir_all(&cache_dir) {
        fail(&progress, host.as_ref(), format!("mkdir cache: {e}"));
        return;
    }

    // biblionlabs raw layout. The per-bible `manifest.json` holds
    // `book_names` / `chapter_headings` / `sections`; per-book files
    // live at `bibles/<id>/books/<book>.json`. The fetch.bible catalogue
    // endpoint uses a different shape (`extra.json`) that this
    // installer does NOT currently parse — switch here if we grow a
    // fetch.bible path.
    const BIBLIONLABS_BASE: &str =
        "https://raw.githubusercontent.com/biblionlabs/extra_data_source/refs/heads/main";
    let desc_url = format!("{BIBLIONLABS_BASE}/bibles/{bible_id}/manifest.json");
    let books_template =
        format!("{BIBLIONLABS_BASE}/bibles/{{bible_id}}/books/{{book}}.json");
    log::info!("bible install: desc_url={desc_url}");

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
                    h.notify(
                        PLUGIN_ID,
                        NotificationKind::Info,
                        format!("Catálogo listo — descargando {total_books} libros"),
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

    // Resolve catalogue-side name / language / abbrev — the per-bible
    // biblionlabs manifest is silent on these so without this lookup
    // the DB ends up storing `bible_id` as the display name.
    let meta = {
        let manifest_url = plugin_config::effective_manifest_url(&config);
        match biblionlabs::load_catalogue(&client, &cache_dir, &manifest_url, CACHE_TTL) {
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
                log::warn!("bible install: catalogue lookup failed, DB name will fall back to id: {e}");
                biblionlabs::BibleMeta::default()
            }
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
        meta,
        progress_cb,
    );

    match result {
        Ok(verses) => {
            // Pre-fetch section headings for every (book, chapter)
            // right after the main install so the reader renders
            // subtitles the first time the user opens a passage —
            // the lazy per-chapter fetch is only a fallback.
            prefetch_all_headings(&db, &client, &cache_dir, &bible_id, &progress, host.as_ref());

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

/// Walk every (book, chapter) in the newly installed Bible and
/// populate the `headers` table from helloao's sibling translation.
/// Idempotent — [`helloao::install_headings_for_chapter`] wipes +
/// rewrites per chapter, and the per-chapter JSON cache stops
/// re-downloads of unchanged payloads.
///
/// Failures are logged + swallowed per chapter: a flaky network for
/// one book must not fail the whole install.
fn prefetch_all_headings(
    db: &Db,
    client: &Client,
    cache_dir: &std::path::Path,
    bible_id: &str,
    progress: &SharedInstallProgress,
    host: Option<&notein_sdk::Host>,
) {
    let Some(source) = helloao::helloao_headings_id_for_bible(bible_id) else {
        log::debug!("headings prefetch: no sibling id for {bible_id}; skipping");
        return;
    };
    let books = match db.list_books(bible_id) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("headings prefetch: list_books failed: {e:?}");
            return;
        }
    };
    // Reset the shared counters so the install modal animates through
    // the subtitle pass instead of looking frozen at the last state
    // from `install_bible`.
    {
        let mut p = progress.write().unwrap();
        p.stage = InstallStage::Downloading;
        p.done_books = 0;
        p.total_books = books.len();
    }
    if let Some(h) = host {
        h.report_progress(
            PLUGIN_ID,
            PROGRESS_TOKEN,
            HostProgress::Update {
                done: 0,
                total: Some(books.len() as u64),
                label: Some("Descargando subtítulos…".into()),
            },
        );
    }
    for (idx, book) in books.iter().enumerate() {
        let chapters = db.chapter_count(bible_id, &book.book_id).unwrap_or(0);
        for chapter in 1..=chapters {
            if let Err(e) = helloao::install_headings_for_chapter(
                db,
                client,
                cache_dir,
                bible_id,
                source,
                &book.book_id,
                chapter,
                CACHE_TTL,
            ) {
                log::debug!(
                    "headings prefetch: {bible_id} {} {chapter}: {e}",
                    book.book_id
                );
            }
        }
        {
            let mut p = progress.write().unwrap();
            p.done_books = idx + 1;
        }
        if let Some(h) = host {
            h.report_progress(
                PLUGIN_ID,
                PROGRESS_TOKEN,
                HostProgress::Update {
                    done: (idx + 1) as u64,
                    total: Some(books.len() as u64),
                    label: Some(format!("Subtítulos — {}", book.name)),
                },
            );
        }
    }
}

fn fail(progress: &SharedInstallProgress, host: Option<&notein_sdk::Host>, message: String) {
    log::error!("bible: install failed: {message}");
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

