//! Translation downloads, one at a time, in the order asked for. The
//! catalog sheet and the first-use card both read from here.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, PoisonError};

use crate::db::Db;
use crate::install_background::{self, InstallStage, SharedInstallProgress};
pub use crate::install_background::{InstallFailure, Phase, Step};
use crate::plugin_config::SharedConfig;

/// Where one translation is in the queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallState {
    Queued,
    Downloading(Step),
    Failed(InstallFailure),
}

/// How a download ended, for whoever wants to tell the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Installed(String),
    Failed {
        bible_id: String,
        reason: InstallFailure,
    },
}

#[derive(Debug, Default)]
struct Queue {
    waiting: VecDeque<String>,
    states: HashMap<String, InstallState>,
    running: bool,
    listeners: Vec<flume::Sender<()>>,
    outcomes: Vec<flume::Sender<Outcome>>,
}

impl Queue {
    fn announce(&mut self) {
        self.listeners.retain(|tx| tx.send(()).is_ok());
    }

    fn finish(&mut self, bible_id: String, outcome: Option<Outcome>) {
        match &outcome {
            Some(Outcome::Failed { reason, .. }) => {
                self.states
                    .insert(bible_id, InstallState::Failed(reason.clone()));
            }
            Some(Outcome::Installed(_)) | None => {
                self.states.remove(&bible_id);
            }
        }
        if let Some(outcome) = outcome {
            self.outcomes.retain(|tx| tx.send(outcome.clone()).is_ok());
        }
        self.announce();
    }

    /// Start the next waiting install; `None` once the queue ran dry.
    fn start_next(&mut self) -> Option<String> {
        let Some(id) = self.waiting.pop_front() else {
            self.running = false;
            return None;
        };
        self.states
            .insert(id.clone(), InstallState::Downloading(Step::default()));
        self.announce();
        Some(id)
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
    #[cfg_attr(feature = "sismo", sismo::function("bible::installer_enqueue"))]
    pub fn enqueue(&self, bible_id: &str) {
        let start = {
            let mut queue = self.lock();
            if matches!(
                queue.states.get(bible_id),
                Some(InstallState::Queued | InstallState::Downloading(_))
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
                let mut queue = self.lock();
                queue.running = false;
                let stuck: Vec<String> = queue.waiting.drain(..).collect();
                for bible_id in stuck {
                    let reason = InstallFailure::CouldNotStart;
                    queue.finish(bible_id.clone(), Some(Outcome::Failed { bible_id, reason }));
                }
            }
        }
    }

    #[cfg_attr(feature = "sismo", sismo::function("bible::installer_drain"))]
    fn drain(&self) {
        loop {
            let Some(next) = self.lock().start_next() else {
                return;
            };
            #[cfg(feature = "sismo")]
            sismo::scope!("bible::installer_install");
            let watcher = self.watch(&next);
            install_background::run(&next, &self.db, &self.config, &self.progress);
            watcher.store(true, std::sync::atomic::Ordering::Relaxed);
            let finished = self
                .progress
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            let outcome = match finished.stage {
                InstallStage::Failed => Some(Outcome::Failed {
                    bible_id: next.clone(),
                    reason: finished.error.unwrap_or(InstallFailure::Unknown),
                }),
                InstallStage::Finished => Some(Outcome::Installed(next.clone())),
                InstallStage::Idle | InstallStage::Starting | InstallStage::Downloading => None,
            };
            self.lock().finish(next, outcome);
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
                    let next = InstallState::Downloading(p.step);
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

    /// The translation downloading now, with how far it is.
    #[must_use]
    pub fn active(&self) -> Option<(String, Step)> {
        self.lock()
            .states
            .iter()
            .find_map(|(id, state)| match state {
                InstallState::Downloading(step) => Some((id.clone(), *step)),
                InstallState::Queued | InstallState::Failed(_) => None,
            })
    }

    /// Queue again every download a closed app cut short.
    #[cfg_attr(feature = "sismo", sismo::function("bible::installer_resume"))]
    pub fn resume(&self) {
        match self.db.pending_installs() {
            Ok(pending) => pending.iter().for_each(|id| self.enqueue(id)),
            Err(e) => log::warn!("bible installer: pending downloads: {e}"),
        }
    }

    /// Every download that ends from now on.
    #[must_use]
    pub fn outcomes(&self) -> flume::Receiver<Outcome> {
        let (tx, rx) = flume::unbounded();
        self.lock().outcomes.push(tx);
        rx
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
    #[cfg_attr(feature = "sismo", sismo::function("bible::installer_catalogue"))]
    pub fn catalogue(
        &self,
    ) -> Result<crate::import::biblionlabs::CatalogueView, crate::import::biblionlabs::ImportError>
    {
        install_background::load_catalogue(&self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finish_reports_outcome_and_settles_state() {
        let mut queue = Queue::default();
        let (tx, rx) = flume::unbounded();
        queue.outcomes.push(tx);
        queue.states.insert(
            "rvr".to_owned(),
            InstallState::Downloading(Step::new(Phase::Books, 3, 66)),
        );
        queue.finish("rvr".to_owned(), Some(Outcome::Installed("rvr".to_owned())));
        assert_eq!(queue.states.get("rvr"), None);
        assert_eq!(rx.try_recv(), Ok(Outcome::Installed("rvr".to_owned())));

        let failed = Outcome::Failed {
            bible_id: "nvi".to_owned(),
            reason: InstallFailure::Net(notein_sdk::NetFailure::Offline),
        };
        queue.finish("nvi".to_owned(), Some(failed.clone()));
        assert_eq!(
            queue.states.get("nvi"),
            Some(&InstallState::Failed(InstallFailure::Net(
                notein_sdk::NetFailure::Offline
            )))
        );
        assert_eq!(rx.try_recv(), Ok(failed));
    }

    #[test]
    fn step_fraction_moves_forward_through_the_phases() {
        let steps = [
            Step::new(Phase::Books, 0, 0),
            Step::new(Phase::Books, 33, 66),
            Step::new(Phase::Books, 66, 66),
            Step::new(Phase::CrossRefs, 10, 66),
            Step::new(Phase::Headings, 0, 0),
            Step::new(Phase::Headings, 66, 66),
        ];
        let fractions = steps.map(Step::fraction);
        assert!(fractions.windows(2).all(|w| w[0] <= w[1]), "{fractions:?}");
        assert!((fractions[5] - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn interrupted_install_is_silent() {
        let mut queue = Queue::default();
        let (tx, rx) = flume::unbounded();
        queue.outcomes.push(tx);
        queue.finish("rvr".to_owned(), None);
        assert!(rx.try_recv().is_err());
    }
}
