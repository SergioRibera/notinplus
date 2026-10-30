//! Download threads: a translation install, run by
//! [`crate::installer`], and the lazy footnote / heading fetches for one
//! chapter. Each writes straight into the shared [`crate::db::Db`].
//!
//! Gated behind the `importer` feature: the HTTP stack only pulls in
//! when the host wants real downloads.

use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use notein_i18n::tr;
use notein_sdk::{NetFailure, UserMessage};
use reqwest::blocking::Client;

use crate::db::{Db, DbError};
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
    pub step: Step,
    pub error: Option<InstallFailure>,
}

/// Why a download or the catalogue failed, kept typed so the sheet
/// words it in the language showing when it is read.
#[derive(Clone, Debug)]
pub enum InstallFailure {
    Net(NetFailure),
    Disk(std::io::ErrorKind),
    Database(Arc<DbError>),
    /// The server answered with something that is not a translation.
    BadData,
    /// The download thread could not be spawned.
    CouldNotStart,
    /// The Bible database is not open on this device.
    Unavailable,
    /// The install stopped without saying why.
    Unknown,
}

impl PartialEq for InstallFailure {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Net(a), Self::Net(b)) => a == b,
            (Self::Disk(a), Self::Disk(b)) => a == b,
            (Self::Database(a), Self::Database(b)) => Arc::ptr_eq(a, b),
            (Self::BadData, Self::BadData)
            | (Self::CouldNotStart, Self::CouldNotStart)
            | (Self::Unavailable, Self::Unavailable)
            | (Self::Unknown, Self::Unknown) => true,
            _ => false,
        }
    }
}

impl Eq for InstallFailure {}

impl std::fmt::Display for InstallFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Net(failure) => write!(f, "network: {failure}"),
            Self::Disk(kind) => write!(f, "disk: {kind}"),
            Self::Database(err) => write!(f, "{err}"),
            Self::BadData => f.write_str("unexpected response"),
            Self::CouldNotStart => f.write_str("installer thread did not start"),
            Self::Unavailable => f.write_str("bible database unavailable"),
            Self::Unknown => f.write_str("install failed"),
        }
    }
}

impl std::error::Error for InstallFailure {}

impl From<biblionlabs::ImportError> for InstallFailure {
    fn from(err: biblionlabs::ImportError) -> Self {
        use biblionlabs::ImportError;
        match err {
            ImportError::Http(err) => Self::Net(NetFailure::classify(
                err.is_connect(),
                err.is_timeout(),
                err.status().map(|status| status.as_u16()),
            )),
            ImportError::Json { .. } | ImportError::Format { .. } => Self::BadData,
            ImportError::Io(err) => Self::Disk(err.kind()),
            ImportError::Db(err) => Self::Database(Arc::new(err)),
        }
    }
}

impl UserMessage for InstallFailure {
    fn user_message(&self) -> String {
        match self {
            Self::Net(failure) => failure.user_message(),
            Self::Disk(kind) => tr!("errors-install-disk", "reason" => kind.user_message()),
            Self::Database(err) => err.user_message(),
            Self::BadData => tr!("errors-install-bad-data"),
            Self::CouldNotStart => tr!("errors-install-could-not-start"),
            Self::Unavailable => tr!("catalog-unavailable"),
            Self::Unknown => tr!("errors-install-unknown"),
        }
    }
}

/// The parts of a download, in order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Books,
    CrossRefs,
    Headings,
}

/// How far a download is: `done` of `total` units of `phase` (`total` is 0
/// until the phase knows its size).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Step {
    pub phase: Phase,
    pub done: usize,
    pub total: usize,
}

impl Step {
    #[must_use]
    pub const fn new(phase: Phase, done: usize, total: usize) -> Self {
        Self { phase, done, total }
    }

    /// The whole download, 0 to 1. The books take most of it; the
    /// cross-references and the section headings follow.
    #[must_use]
    pub fn fraction(self) -> f32 {
        let (start, share): (f32, f32) = match self.phase {
            Phase::Books => (0.0, 0.7),
            Phase::CrossRefs => (0.7, 0.2),
            Phase::Headings => (0.9, 0.1),
        };
        #[allow(clippy::cast_precision_loss)] // book counts
        let part = if self.total == 0 {
            0.0
        } else {
            self.done.min(self.total) as f32 / self.total as f32
        };
        share.mul_add(part, start)
    }
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
/// user-visible error. `landed` runs when new rows were stored.
pub fn spawn_fetch_footnotes(
    db: Db,
    bible_id: String,
    book_id: String,
    chapter: u16,
    landed: impl FnOnce() + Send + 'static,
) {
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
                    if n > 0 {
                        landed();
                    }
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
pub fn spawn_fetch_headings(db: Db, bible_id: String, book_id: String, chapter: u16) {
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
        Err(e) => return fail(progress, biblionlabs::ImportError::Http(e)),
    };
    let cache_dir = plugin_config::platform_cache_dir();
    if let Err(e) = std::fs::create_dir_all(&cache_dir) {
        return fail(progress, biblionlabs::ImportError::Io(e));
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
                p.step = Step::new(Phase::Books, 0, total_books);
            }
            biblionlabs::Progress::BookDone { .. } => p.step.done += 1,
            biblionlabs::Progress::CrossRefs { done, total } => {
                p.step = Step::new(Phase::CrossRefs, done, total);
            }
            biblionlabs::Progress::Started { .. } | biblionlabs::Progress::Finished { .. } => {}
        });
    };

    match install.run(on_progress) {
        Ok(verses) => {
            // The translation shows only after its headings, so the first
            // passage opened already has them.
            prefetch_all_headings(db, &client, &cache_dir, bible_id, progress);
            if let Err(e) = db.finish_install(bible_id) {
                return fail(progress, biblionlabs::ImportError::Db(e));
            }
            edit(progress, |p| {
                p.stage = InstallStage::Finished;
                p.error = None;
            });
            log::info!("bible install: {bible_id} done, {verses} verses");
        }
        Err(e) => fail(progress, e),
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
            log::warn!(
                "bible install: catalogue lookup failed, the name falls back to the id: {e}"
            );
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
    edit(progress, |p| {
        p.stage = InstallStage::Downloading;
        p.step = Step::new(Phase::Headings, 0, 0);
    });
    let helloao = helloao::Helloao {
        db,
        client,
        cache_dir,
        cache_ttl: CACHE_TTL,
    };
    let result = helloao.all_headings(bible_id, source, |done, total, _book_name| {
        edit(progress, |p| {
            p.step = Step::new(Phase::Headings, done, total);
        });
    });
    match result {
        Ok(n) => log::info!("headings prefetch: {bible_id} inserted {n} rows via {source}"),
        Err(e) => log::warn!("headings prefetch: {bible_id} failed: {e}"),
    }
}

fn fail(progress: &SharedInstallProgress, err: biblionlabs::ImportError) {
    log::error!("bible: install failed: {err}");
    let failure = InstallFailure::from(err);
    edit(progress, |p| {
        p.stage = InstallStage::Failed;
        p.error = Some(failure);
    });
}

#[cfg(test)]
mod tests {
    use notein_i18n::Lang;
    use notein_sdk::{NetFailure, UserMessage};

    use super::InstallFailure;
    use crate::import::biblionlabs::ImportError;

    #[test]
    fn install_failures_read_in_spanish() {
        Lang::Es.make_current();
        let disk = InstallFailure::from(ImportError::Io(std::io::ErrorKind::StorageFull.into()));
        let message = disk.user_message();
        assert!(message.contains("traducción"), "{message}");
        assert!(message.contains("espacio"), "{message}");
        let server = InstallFailure::Net(NetFailure::Status(502)).user_message();
        assert!(server.contains("502"), "{server}");
        let garbled = InstallFailure::from(ImportError::Format {
            reason: "non-JSON response".to_owned(),
        });
        assert!(!garbled.user_message().contains("non-JSON"));
    }
}
