//! New versions: knowing one is out, saying so, and installing it.
//!
//! The release workflow publishes `latest.json` with each stable
//! release. [`check`] reads it (from a copy kept for two days, see
//! [`check::MAX_AGE`]); when it names a version newer than this one
//! and the person did not skip it, Home shows a chip that opens the
//! release notes. From there the file for this device downloads with
//! its progress in a sheet, its SHA-256 is checked, and it goes to the
//! system installer on Android and Windows or is shown in its folder
//! elsewhere.

pub mod check;
pub mod download;
pub mod release;
pub mod ui;

use std::sync::Arc;
use std::time::Duration;

use freya::prelude::*;
use notein_i18n::tr;
use notein_plugin::{NetFailure, UserMessage};

pub use check::UpdateRecord;
pub use release::{Asset, Device, Release};

use crate::failure::NetFailureExt;
use crate::persist::Persisted;

/// The version this build is.
pub const INSTALLED: &str = env!("CARGO_PKG_VERSION");

/// How often a running app looks again. The look is free while the
/// kept manifest is fresh, so this only bounds how late a stale one is
/// noticed.
const RECHECK: Duration = Duration::from_secs(6 * 60 * 60);

/// Why checking or downloading failed.
#[derive(Debug)]
pub enum UpdateError {
    Http(reqwest::Error),
    Manifest(serde_json::Error),
    Io(std::io::Error),
    /// The downloaded file is not the one the manifest describes.
    Checksum,
    Cancelled,
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(err) => write!(f, "network: {err}"),
            Self::Manifest(err) => write!(f, "latest.json: {err}"),
            Self::Io(err) => write!(f, "disk: {err}"),
            Self::Checksum => f.write_str("the download does not match its SHA-256"),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for UpdateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(err) => Some(err),
            Self::Manifest(err) => Some(err),
            Self::Io(err) => Some(err),
            Self::Checksum | Self::Cancelled => None,
        }
    }
}

impl UserMessage for UpdateError {
    fn user_message(&self) -> String {
        UpdateFailure::from(self).user_message()
    }
}

/// What the interface keeps of a failed check, download or install:
/// small and `Copy`, worded when it is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateFailure {
    Net(NetFailure),
    Disk(std::io::ErrorKind),
    /// `latest.json` could not be read.
    Manifest,
    Checksum,
    Cancelled,
    /// The system installer would not open the file.
    Installer,
}

impl From<&UpdateError> for UpdateFailure {
    fn from(err: &UpdateError) -> Self {
        match err {
            UpdateError::Http(err) => Self::Net(err.net_failure()),
            UpdateError::Manifest(_) => Self::Manifest,
            UpdateError::Io(err) => Self::Disk(err.kind()),
            UpdateError::Checksum => Self::Checksum,
            UpdateError::Cancelled => Self::Cancelled,
        }
    }
}

impl UserMessage for UpdateFailure {
    fn user_message(&self) -> String {
        match self {
            Self::Net(failure) => failure.user_message(),
            Self::Disk(kind) => tr!("errors-update-disk", "reason" => kind.user_message()),
            Self::Manifest => tr!("errors-update-manifest"),
            Self::Checksum => tr!("update-failed-checksum"),
            Self::Cancelled => tr!("update-failed"),
            Self::Installer => tr!("update-failed-installer"),
        }
    }
}

impl From<reqwest::Error> for UpdateError {
    fn from(err: reqwest::Error) -> Self {
        Self::Http(err)
    }
}

impl From<std::io::Error> for UpdateError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// Where the last check stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CheckStatus {
    #[default]
    Idle,
    Checking,
    /// The last check found nothing newer.
    UpToDate,
    /// The last manual check failed.
    Failed(UpdateFailure),
}

/// What the interface knows about updates.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// The newest release, when it is newer than this build.
    pub newer: Option<Arc<Release>>,
    pub record: UpdateRecord,
    pub status: CheckStatus,
}

impl Snapshot {
    /// The release Home announces: newer and not skipped.
    #[must_use]
    pub fn announced(&self) -> Option<&Arc<Release>> {
        self.newer
            .as_ref()
            .filter(|release| self.record.skipped.as_deref() != Some(release.version.as_str()))
    }
}

/// Reactive update state, provided once at the app root.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Updates {
    state: State<Snapshot>,
}

impl std::fmt::Debug for Updates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updates").finish_non_exhaustive()
    }
}

impl Updates {
    /// The store installed by [`use_updates_root`].
    #[must_use]
    pub fn get() -> Self {
        consume_root_context::<Self>()
    }

    /// Current state; subscribes the caller.
    #[must_use]
    pub fn read(&self) -> Snapshot {
        self.state.read().clone()
    }

    /// Look for a newer release in the background. `force` skips the
    /// kept manifest (the "Check now" button); without it nothing is
    /// fetched while the copy is fresh, and nothing at all when the
    /// person turned checks off.
    #[sismo::function("update::check")]
    pub fn check(self, force: bool) {
        let mut state = self.state;
        if !force && state.peek().record.manual_only {
            return;
        }
        if state.peek().status == CheckStatus::Checking {
            return;
        }
        state.write().status = CheckStatus::Checking;
        let (tx, rx) = flume::bounded(1);
        let spawned = std::thread::Builder::new()
            .name("notein-update-check".to_owned())
            .spawn(move || {
                let _ = tx.send(check::latest(force));
            });
        if let Err(err) = spawned {
            log::warn!("update check thread: {err}");
            state.write().status = CheckStatus::Idle;
            return;
        }
        spawn_in_window(async move {
            let Ok(result) = rx.recv_async().await else {
                return;
            };
            let mut snapshot = state.write();
            snapshot.record = UpdateRecord::load();
            match result {
                Ok(release) => {
                    let newer = release.is_newer_than(INSTALLED);
                    snapshot.newer = newer.then(|| Arc::new(release));
                    snapshot.status = if newer {
                        CheckStatus::Idle
                    } else {
                        CheckStatus::UpToDate
                    };
                }
                Err(err) => {
                    log::warn!("update check: {err}");
                    snapshot.status = if force {
                        CheckStatus::Failed(UpdateFailure::from(&err))
                    } else {
                        CheckStatus::Idle
                    };
                }
            }
        });
    }

    /// Stop announcing `version`; the next one is announced again.
    #[sismo::function("update::skip")]
    pub fn skip(self, version: &str) {
        self.edit(|record| record.skipped = Some(version.to_owned()));
    }

    /// Announce the skipped version again.
    #[sismo::function("update::unskip")]
    pub fn unskip(self) {
        self.edit(|record| record.skipped = None);
    }

    /// Turn automatic checks on or off.
    #[sismo::function("update::set_automatic")]
    pub fn set_automatic(self, on: bool) {
        self.edit(|record| record.manual_only = !on);
        if on {
            self.check(false);
        }
    }

    #[sismo::function("update::edit")]
    fn edit(self, change: impl FnOnce(&mut UpdateRecord)) {
        let mut state = self.state;
        let mut record = UpdateRecord::load();
        change(&mut record);
        if let Err(err) = record.save() {
            log::error!("update settings save: {err}");
        }
        state.write().record = record;
    }
}

/// Install the update store and start looking: once now, then every
/// [`RECHECK`] while the app stays open.
pub fn use_updates_root() {
    let state = use_state(|| Snapshot {
        record: UpdateRecord::load(),
        ..Snapshot::default()
    });
    use_hook(move || {
        let updates = Updates { state };
        provide_context_for_scope_id(updates, ScopeId::ROOT);
        spawn_in_window(async move {
            loop {
                updates.check(false);
                async_io::Timer::after(RECHECK).await;
            }
        });
    });
}

/// [`use_updates_root`] for headless renders: announces `newer`
/// without touching the network.
#[cfg(test)]
pub(crate) fn use_preview_updates(newer: Option<Release>) {
    let state = use_state(move || Snapshot {
        newer: newer.map(Arc::new),
        ..Snapshot::default()
    });
    use_hook(move || provide_context_for_scope_id(Updates { state }, ScopeId::ROOT));
}

#[cfg(test)]
mod tests {
    use notein_i18n::Lang;
    use notein_plugin::NetFailure;

    use super::*;

    fn release(version: &str) -> Arc<Release> {
        Arc::new(Release {
            version: version.into(),
            url: String::new(),
            published_at: String::new(),
            notes: String::new(),
            assets: Vec::new(),
        })
    }

    #[test]
    fn a_skipped_version_is_not_announced_but_the_next_one_is() {
        let mut snapshot = Snapshot {
            newer: Some(release("0.2.0")),
            ..Snapshot::default()
        };
        assert!(snapshot.announced().is_some());
        snapshot.record.skipped = Some("0.2.0".into());
        assert!(snapshot.announced().is_none());
        snapshot.newer = Some(release("0.3.0"));
        assert!(snapshot.announced().is_some());
    }

    #[test]
    fn update_failures_say_what_to_do_in_spanish() {
        Lang::Es.make_current();
        let disk = UpdateError::Io(std::io::ErrorKind::StorageFull.into());
        let message = disk.user_message();
        assert!(message.contains("descarga"), "{message}");
        assert!(message.contains("espacio"), "{message}");
        let offline = UpdateFailure::Net(NetFailure::Offline).user_message();
        assert!(offline.contains("conexión"), "{offline}");
    }
}
