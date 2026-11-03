//! The original-language text behind the translations: every Hebrew and
//! Greek word with its transliteration, grammar and meaning.
//!
//! The data comes as two packs (one per testament) and one senses file
//! per interface language, built by `originals-pack` in the
//! `extra_data_source` repository and published with its releases. A
//! [`Store`] reads them from [`dir`]; [`align`] pairs the words with an
//! installed translation; [`morph`] and [`respell`] turn codes and
//! transliterations into words a reader can use. [`Originals`] is the
//! service the reader and the settings share: it keeps the store open,
//! downloads the packs and aligns the translations in the background.

pub mod align;
#[cfg(feature = "importer")]
pub mod download;
pub mod morph;
pub mod respell;
mod store;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};

pub use originals_format::Testament;
pub use store::{
    AffixView, LemmaRef, LemmaView, Occurrence, SenseView, Store, StoreError, WordView,
};

use crate::db::Db;

/// `meta` key: `built_unix` of the packs on disk, set once every file
/// landed.
pub const META_BUILT: &str = "originals_built_unix";

/// Where the packs live on this device.
#[must_use]
pub fn dir() -> PathBuf {
    istmo_core::path::data_dir()
        .join("notinplus")
        .join("bible")
        .join("originals")
}

/// File name of a testament's pack on disk, decompressed.
#[must_use]
pub const fn pack_file(testament: Testament) -> &'static str {
    match testament {
        Testament::Old => "originals-ot.bin",
        Testament::New => "originals-nt.bin",
    }
}

/// File name of a language's senses on disk, decompressed.
#[must_use]
pub fn senses_file(language: &str) -> String {
    format!("senses-{language}.bin")
}

/// The testament of a book in the Protestant canon, by its id.
#[must_use]
pub fn testament_of(book_id: &str) -> Option<Testament> {
    let position = crate::canon::protestant_position(book_id)?;
    Some(if position < 39 {
        Testament::Old
    } else {
        Testament::New
    })
}

/// What the service is doing, for the card in the reader.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Progress {
    #[default]
    Idle,
    /// `done` of `total` files.
    Downloading { done: usize, total: usize },
    /// Pairing the words with this translation.
    Aligning { bible_id: String },
    /// The last download failed; the message is for the user.
    Failed(String),
}

/// The packs on this device and the work on them. One per Bible kind,
/// reachable through [`service`] from the settings and the installer.
#[derive(Debug)]
pub struct Originals {
    db: Db,
    dir: PathBuf,
    store: RwLock<Option<Arc<Store>>>,
    /// Set once a load was tried, so a missing pack is not re-read on
    /// every render.
    tried: AtomicBool,
    progress: RwLock<Progress>,
    listeners: Mutex<Vec<flume::Sender<()>>>,
    busy: AtomicBool,
}

static SERVICE: OnceLock<Arc<Originals>> = OnceLock::new();

/// Make `originals` the service the plugin shares; the first one stays.
pub fn register(originals: &Arc<Originals>) {
    let _ = SERVICE.set(Arc::clone(originals));
}

/// The shared service, once a Bible kind registered one.
#[must_use]
pub fn service() -> Option<&'static Arc<Originals>> {
    SERVICE.get()
}

impl Originals {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self::at(db, dir())
    }

    /// A service reading packs from `dir` instead of the platform
    /// directory.
    #[must_use]
    pub const fn at(db: Db, dir: PathBuf) -> Self {
        Self {
            db,
            dir,
            store: RwLock::new(None),
            tried: AtomicBool::new(false),
            progress: RwLock::new(Progress::Idle),
            listeners: Mutex::new(Vec::new()),
            busy: AtomicBool::new(false),
        }
    }

    /// The packs, read on first use. `None` until they are downloaded,
    /// or when the files on disk are not usable (logged; a download
    /// replaces them).
    #[cfg_attr(feature = "sismo", sismo::function("bible::originals_store"))]
    pub fn store(&self) -> Option<Arc<Store>> {
        if let Some(store) = self
            .store
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            return Some(Arc::clone(store));
        }
        if self.tried.swap(true, Ordering::AcqRel) {
            return None;
        }
        self.reload()
    }

    /// Read the packs again (after a download).
    #[cfg_attr(feature = "sismo", sismo::function("bible::originals_reload"))]
    pub fn reload(&self) -> Option<Arc<Store>> {
        let loaded = match Store::open(&self.dir) {
            Ok(store) => Some(Arc::new(store)),
            Err(StoreError::Missing) => None,
            Err(err) => {
                log::warn!("{err}");
                None
            }
        };
        self.tried.store(true, Ordering::Release);
        self.store
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .clone_from(&loaded);
        self.announce();
        loaded
    }

    /// Whether the packs are on this device and readable.
    #[must_use]
    pub fn ready(&self) -> bool {
        self.store().is_some()
    }

    #[must_use]
    pub fn progress(&self) -> Progress {
        self.progress
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_progress(&self, progress: Progress) {
        *self
            .progress
            .write()
            .unwrap_or_else(PoisonError::into_inner) = progress;
        self.announce();
    }

    /// Fires whenever the store or the progress changes.
    #[must_use]
    pub fn changes(&self) -> flume::Receiver<()> {
        let (tx, rx) = flume::unbounded();
        self.listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(tx);
        rx
    }

    fn announce(&self) {
        self.listeners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|tx| tx.send(()).is_ok());
    }

    /// Whether a download or an alignment is running.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    /// Download the packs (or the files missing), then align every
    /// installed translation. Nothing happens while another job runs.
    #[cfg(feature = "importer")]
    #[cfg_attr(feature = "sismo", sismo::function("bible::originals_spawn_download"))]
    pub fn spawn_download(self: &Arc<Self>) {
        if self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let this = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("bible-originals".to_owned())
            .spawn(move || {
                this.set_progress(Progress::Downloading { done: 0, total: 0 });
                let result = reqwest::blocking::Client::builder()
                    .user_agent("notein-bible/0.1")
                    .build()
                    .map_err(download::DownloadError::from)
                    .and_then(|client| {
                        download::ensure(&this.db, &client, &this.dir, |done, total| {
                            this.set_progress(Progress::Downloading { done, total });
                        })
                    });
                match result {
                    Ok(()) => {
                        this.reload();
                        this.set_progress(Progress::Idle);
                        this.align_missing();
                    }
                    Err(err) => {
                        use notein_sdk::UserMessage as _;
                        log::warn!("{err}");
                        let failure = crate::install_background::InstallFailure::from(err);
                        this.set_progress(Progress::Failed(failure.user_message()));
                    }
                }
                this.busy.store(false, Ordering::Release);
            });
        if let Err(err) = spawned {
            log::warn!("bible originals thread: {err}");
            self.busy.store(false, Ordering::Release);
        }
    }

    /// Align, in the background, every installed translation that is
    /// not aligned against the packs on disk yet.
    #[cfg_attr(feature = "sismo", sismo::function("bible::originals_spawn_align"))]
    pub fn spawn_align_missing(self: &Arc<Self>) {
        if self.store().is_none() || self.busy.swap(true, Ordering::AcqRel) {
            return;
        }
        let this = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("bible-originals-align".to_owned())
            .spawn(move || {
                this.align_missing();
                this.busy.store(false, Ordering::Release);
            });
        if let Err(err) = spawned {
            log::warn!("bible originals align thread: {err}");
            self.busy.store(false, Ordering::Release);
        }
    }

    /// Blocking: align the translations that need it, one by one.
    fn align_missing(&self) {
        let Some(store) = self.store() else {
            return;
        };
        let bibles = match self.db.list_bibles() {
            Ok(bibles) => bibles,
            Err(err) => {
                log::warn!("originals: translations: {err}");
                return;
            }
        };
        for bible in bibles {
            if align::is_done(&self.db, &store, &bible.id) {
                continue;
            }
            self.set_progress(Progress::Aligning {
                bible_id: bible.id.clone(),
            });
            if let Err(err) = align::run(&self.db, &store, &bible.id) {
                log::warn!("originals: aligning {}: {err}", bible.id);
            }
        }
        self.set_progress(Progress::Idle);
    }

    /// Which translation tokens render each original word of a verse,
    /// by position; empty until the translation is aligned.
    #[must_use]
    pub fn links(
        &self,
        bible_id: &str,
        book_id: &str,
        chapter: u16,
        verse: u16,
    ) -> Vec<crate::db::LinkRow> {
        let Some(store) = self.store() else {
            return Vec::new();
        };
        let (Some(testament), Some(book)) = (testament_of(book_id), store.book_index(book_id))
        else {
            return Vec::new();
        };
        self.db
            .alignment_links(
                bible_id,
                align::testament_code(testament),
                book,
                chapter,
                verse,
            )
            .unwrap_or_default()
    }

    /// How `bible_id` renders a lemma across the text, most frequent
    /// first; empty until aligned.
    #[must_use]
    pub fn renderings(&self, bible_id: &str, lemma: LemmaRef) -> Vec<crate::db::RenderingRow> {
        self.db
            .renderings(
                bible_id,
                align::testament_code(lemma.testament),
                lemma.index,
            )
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn books_fall_in_their_testament() {
        assert_eq!(testament_of("gen"), Some(Testament::Old));
        assert_eq!(testament_of("MAL"), Some(Testament::Old));
        assert_eq!(testament_of("mat"), Some(Testament::New));
        assert_eq!(testament_of("tob"), None);
    }

    #[test]
    fn service_reads_the_packs_once_and_reloads_on_demand() {
        let dir =
            std::env::temp_dir().join(format!("notein-originals-service-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let originals = Originals::at(Db::in_memory().unwrap(), dir.clone());
        assert!(originals.store().is_none());
        store::tests::write_sample(&dir);
        assert!(
            originals.store().is_none(),
            "a missing pack is not re-read on every call"
        );
        assert!(originals.reload().is_some());
        assert!(originals.ready());
        assert_eq!(originals.progress(), Progress::Idle);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
