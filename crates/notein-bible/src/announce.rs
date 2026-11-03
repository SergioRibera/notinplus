//! What the plugin tells the shell while its jobs run: a progress toast
//! per download, drawn on every screen, and a note when one ends.
//!
//! Three jobs report here: translation installs (books, cross-references,
//! headings), the cross-reference set downloaded again at a new level,
//! and the original-language packs with their alignment.

use std::sync::{Arc, OnceLock};

use notein_i18n::tr;
use notein_sdk::UserMessage;
use notein_sdk::services::{Host, HostToast, Progress, ToastTone as HostTone};

use crate::db::Db;
use crate::install_background::InstallFailure;
use crate::installer::{Installer, Outcome, Phase, Step};
use crate::kind::{BibleKind, KIND};
use crate::originals::{self, Originals};
use crate::plugin_config::CrossRefLevel;

static HOST: OnceLock<Host> = OnceLock::new();

/// The jobs the shell draws a bar for, one bar each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Job {
    Install,
    CrossRefs,
    Originals,
}

impl Job {
    const fn token(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::CrossRefs => "cross-refs",
            Self::Originals => "originals",
        }
    }

    fn report(self, progress: Progress) {
        if let Some(host) = HOST.get() {
            host.progress(KIND, self.token(), progress);
        }
    }

    fn running(self, label: String, done: usize, total: Option<usize>) {
        self.report(Progress::Running {
            label,
            done: u64::try_from(done).unwrap_or(u64::MAX),
            total: total.map(|total| u64::try_from(total).unwrap_or(u64::MAX)),
        });
    }

    fn finished(self, success: bool, note: Option<String>) {
        self.report(Progress::Finished { success, note });
    }
}

/// Start telling the shell about every job of `kind`, for the life of
/// the app, and pick up the downloads a closed app cut short.
#[cfg_attr(feature = "sismo", sismo::function("bible::announce_start"))]
pub(crate) fn start(host: Host, kind: &Arc<BibleKind>) {
    let _ = HOST.set(host);
    let installer = kind.installer().clone();
    watch_installs(installer.clone(), kind.db().clone());
    watch_originals(Arc::clone(kind.originals()), kind.db().clone());
    installer.resume();
}

/// How the cross-reference set re-download is going.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CrossRefsJob {
    Running {
        level: CrossRefLevel,
        done: usize,
        total: usize,
    },
    Done,
    Failed(InstallFailure),
}

/// Tell the shell about the cross-reference set downloaded again.
pub(crate) fn cross_refs(job: CrossRefsJob) {
    match job {
        CrossRefsJob::Running { level, done, total } => Job::CrossRefs.running(
            tr!("catalog-progress-cross-refs-level", "level" => level.label()),
            done,
            (total > 0).then_some(total),
        ),
        CrossRefsJob::Done => {
            Job::CrossRefs.finished(true, Some(tr!("catalog-toast-cross-refs-done")));
        }
        CrossRefsJob::Failed(reason) => Job::CrossRefs.finished(
            false,
            Some(tr!("catalog-toast-cross-refs-failed", "reason" => reason.user_message())),
        ),
    }
}

/// Short name of an installed translation, its id otherwise.
pub(crate) fn bible_name(db: &Db, bible_id: &str) -> String {
    db.list_bibles()
        .ok()
        .and_then(|bibles| bibles.into_iter().find(|b| b.id == bible_id))
        .map_or_else(|| bible_id.to_owned(), |b| b.abbrev)
}

/// The bar of the translation downloading now, and a note (with a retry
/// on failures) when one ends. Every outcome is followed by a change
/// signal, so one loop sees both.
fn watch_installs(installer: Installer, db: Db) {
    let changes = installer.changes();
    let outcomes = installer.outcomes();
    let spawned = std::thread::Builder::new()
        .name("bible-announce-installs".to_owned())
        .spawn(move || {
            let mut shown: Option<(String, Step)> = None;
            while changes.recv().is_ok() {
                while changes.try_recv().is_ok() {}
                while let Ok(outcome) = outcomes.try_recv() {
                    shown = None;
                    finish_install(&installer, &db, outcome);
                }
                let Some((bible_id, step)) = installer.active() else {
                    continue;
                };
                if shown.as_ref() == Some(&(bible_id.clone(), step)) {
                    continue;
                }
                Job::Install.running(
                    install_label(&bible_name(&db, &bible_id), step.phase),
                    percent(step.fraction()),
                    Some(100),
                );
                shown = Some((bible_id, step));
            }
        });
    if let Err(err) = spawned {
        log::error!("bible: install announcer: {err}");
    }
}

fn install_label(name: &str, phase: Phase) -> String {
    match phase {
        Phase::Books => tr!("catalog-progress-books", "name" => name),
        Phase::CrossRefs => tr!("catalog-progress-cross-refs", "name" => name),
        Phase::Headings => tr!("catalog-progress-headings", "name" => name),
    }
}

fn finish_install(installer: &Installer, db: &Db, outcome: Outcome) {
    match outcome {
        Outcome::Installed(bible_id) => Job::Install.finished(
            true,
            Some(tr!("catalog-toast-installed", "name" => bible_name(db, &bible_id))),
        ),
        Outcome::Failed { bible_id, reason } => {
            Job::Install.finished(false, None);
            let Some(host) = HOST.get() else {
                return;
            };
            let installer = installer.clone();
            host.toast(
                HostToast::new(
                    tr!(
                        "catalog-toast-failed",
                        "name" => bible_name(db, &bible_id),
                        "reason" => reason.user_message(),
                    ),
                    HostTone::Error,
                )
                .action(tr!("catalog-retry"), move || installer.enqueue(&bible_id)),
            );
        }
    }
}

/// The bar of the pack download, then of each translation being paired,
/// and a note when the originals are ready or the download failed.
fn watch_originals(originals: Arc<Originals>, db: Db) {
    let changes = originals.changes();
    let spawned = std::thread::Builder::new()
        .name("bible-announce-originals".to_owned())
        .spawn(move || {
            let mut shown = originals::Progress::Idle;
            while changes.recv().is_ok() {
                while changes.try_recv().is_ok() {}
                let now = originals.progress();
                if now == shown {
                    continue;
                }
                match &now {
                    originals::Progress::Downloading { done, total } => Job::Originals.running(
                        tr!("originals-progress-download"),
                        *done,
                        (*total > 0).then_some(*total),
                    ),
                    originals::Progress::Aligning { bible_id } => Job::Originals.running(
                        tr!("originals-progress-align", "name" => bible_name(&db, bible_id)),
                        0,
                        None,
                    ),
                    originals::Progress::Idle => {
                        Job::Originals.finished(true, Some(tr!("originals-toast-ready")));
                    }
                    originals::Progress::Failed(reason) => {
                        Job::Originals.finished(false, Some(reason.clone()));
                    }
                }
                shown = now;
            }
        });
    if let Err(err) = spawned {
        log::error!("bible: originals announcer: {err}");
    }
}

/// `fraction` (0 to 1) as a whole percentage.
fn percent(fraction: f32) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=100
    let percent = (fraction.clamp(0.0, 1.0) * 100.0).round() as usize;
    percent
}

#[cfg(test)]
mod tests {
    use super::percent;

    #[test]
    fn fractions_become_whole_percentages() {
        assert_eq!(percent(0.0), 0);
        assert_eq!(percent(0.705), 71);
        assert_eq!(percent(1.5), 100);
    }
}
