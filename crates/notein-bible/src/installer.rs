//! Translation downloads, one at a time, in the order asked for. The
//! catalog sheet and the first-use card both read from here.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use crate::db::Db;
use crate::install_background::{self, InstallStage, SharedInstallProgress};
use crate::plugin_config::SharedConfig;

/// Where one translation is in the queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallState {
    Queued,
    /// Books done of total (total is 0 until the manifest is read).
    Downloading {
        done: usize,
        total: usize,
    },
    Failed(String),
}

#[derive(Debug, Default)]
struct Queue {
    waiting: VecDeque<String>,
    states: HashMap<String, InstallState>,
    running: bool,
    listeners: Vec<flume::Sender<()>>,
}

impl Queue {
    fn announce(&mut self) {
        self.listeners.retain(|tx| tx.send(()).is_ok());
    }
}

/// Downloads translations into the Bible database.
#[derive(Clone, Debug)]
pub struct Installer {
    db: Db,
    config: SharedConfig,
    progress: SharedInstallProgress,
    queue: Arc<Mutex<Queue>>,
}

impl Installer {
    #[must_use]
    pub fn new(db: Db, config: SharedConfig) -> Self {
        Self {
            db,
            config,
            progress: install_background::new_shared_progress(),
            queue: Arc::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ask for `bible_id`; it starts when the ones before it finish.
    pub fn enqueue(&self, bible_id: &str) {
        let start = {
            let mut queue = self.lock();
            if matches!(
                queue.states.get(bible_id),
                Some(InstallState::Queued | InstallState::Downloading { .. })
            ) {
                return;
            }
            queue
                .states
                .insert(bible_id.to_owned(), InstallState::Queued);
            queue.waiting.push_back(bible_id.to_owned());
            queue.announce();
            !std::mem::replace(&mut queue.running, true)
        };
        if start {
            let worker = self.clone();
            let spawned = std::thread::Builder::new()
                .name("bible-installer".to_owned())
                .spawn(move || worker.drain());
            if let Err(err) = spawned {
                log::error!("bible installer: {err}");
                self.lock().running = false;
            }
        }
    }

    fn drain(&self) {
        loop {
            let next = {
                let mut queue = self.lock();
                if let Some(id) = queue.waiting.pop_front() {
                    queue
                        .states
                        .insert(id.clone(), InstallState::Downloading { done: 0, total: 0 });
                    queue.announce();
                    id
                } else {
                    queue.running = false;
                    return;
                }
            };
            let watcher = self.watch(&next);
            install_background::run(
                next.clone(),
                self.db.clone(),
                self.config.clone(),
                self.progress.clone(),
            );
            watcher.store(true, std::sync::atomic::Ordering::Relaxed);
            let finished = self
                .progress
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            let mut queue = self.lock();
            match finished.stage {
                InstallStage::Failed => {
                    queue.states.insert(
                        next,
                        InstallState::Failed(
                            finished
                                .error
                                .unwrap_or_else(|| "La descarga falló".to_owned()),
                        ),
                    );
                }
                InstallStage::Idle
                | InstallStage::Starting
                | InstallStage::Downloading
                | InstallStage::Finished => {
                    queue.states.remove(&next);
                }
            }
            queue.announce();
        }
    }

    /// Copy the installer's progress into the queue state while
    /// `bible_id` downloads; stops when the returned flag is set.
    fn watch(&self, bible_id: &str) -> Arc<std::sync::atomic::AtomicBool> {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (this, id, flag) = (self.clone(), bible_id.to_owned(), Arc::clone(&stop));
        let spawned = std::thread::Builder::new()
            .name("bible-install-watch".to_owned())
            .spawn(move || {
                while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    let p = this
                        .progress
                        .read()
                        .unwrap_or_else(PoisonError::into_inner)
                        .clone();
                    if p.bible_id != id {
                        continue;
                    }
                    let mut queue = this.lock();
                    let next = InstallState::Downloading {
                        done: p.done_books,
                        total: p.total_books,
                    };
                    if queue.states.get(&id) != Some(&next) {
                        queue.states.insert(id.clone(), next);
                        queue.announce();
                    }
                }
            });
        if let Err(err) = spawned {
            log::warn!("bible install watch: {err}");
        }
        stop
    }

    /// State of `bible_id`, when it is queued, downloading or failed.
    #[must_use]
    pub fn state(&self, bible_id: &str) -> Option<InstallState> {
        self.lock().states.get(bible_id).cloned()
    }

    /// Whether anything is queued or downloading.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.lock().running
    }

    /// Fires whenever a state changes.
    #[must_use]
    pub fn changes(&self) -> flume::Receiver<()> {
        let (tx, rx) = flume::unbounded();
        self.lock().listeners.push(tx);
        rx
    }

    /// The catalogue of translations available to download. Blocking.
    ///
    /// # Errors
    /// The catalogue could not be fetched or read.
    pub fn catalogue(
        &self,
    ) -> Result<crate::import::biblionlabs::CatalogueView, crate::import::biblionlabs::ImportError>
    {
        install_background::load_catalogue(&self.config)
    }
}
