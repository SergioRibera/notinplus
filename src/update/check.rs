//! Asking GitHub for the newest release, at most every couple of days.
//!
//! The last `latest.json` is kept on disk with the time it was fetched.
//! Until it is [`MAX_AGE`] old the app answers from it; past that, the
//! next check fetches again and replaces it. A manual check always
//! fetches.

use std::time::Duration;

use super::{Release, UpdateError};
use crate::persist::Persisted;

/// Where the release workflow publishes the manifest. `latest` only
/// points at stable releases, so pre-releases are never offered.
const MANIFEST_URL: &str =
    "https://github.com/SergioRibera/notinplus/releases/latest/download/latest.json";

/// How long a fetched manifest answers before it is fetched again.
pub const MAX_AGE: Duration = Duration::from_secs(2 * 24 * 60 * 60);

/// Give up on GitHub after this long.
const TIMEOUT: Duration = Duration::from_secs(15);

/// What the app remembers about updates.
#[istmo::message]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateRecord {
    /// Unix ms of the last successful fetch; 0 = never.
    pub fetched_at: u64,
    /// Body of the last `latest.json` fetched.
    pub manifest: String,
    /// Version the person chose not to hear about.
    pub skipped: Option<String>,
    /// The person turned automatic checks off.
    pub manual_only: bool,
}

impl Persisted for UpdateRecord {
    const FILE: &'static str = "updates.v1.bin";
}

impl UpdateRecord {
    /// Whether the kept manifest is still fresh at `now`.
    #[must_use]
    pub fn is_fresh(&self, now: u64) -> bool {
        let max_age = u64::try_from(MAX_AGE.as_millis()).unwrap_or(u64::MAX);
        !self.manifest.is_empty() && now.saturating_sub(self.fetched_at) < max_age
    }

    /// The kept manifest, if it still reads.
    #[must_use]
    pub fn release(&self) -> Option<Release> {
        (!self.manifest.is_empty())
            .then(|| Release::parse(&self.manifest).ok())
            .flatten()
    }
}

/// The newest release, from the kept manifest while it is fresh and
/// `force` is off, else from GitHub. Blocking; call off the UI thread.
///
/// # Errors
/// The fetch failed and there was nothing kept to fall back on (or
/// `force` asked for a fresh answer), or the manifest did not read.
pub fn latest(force: bool) -> Result<Release, UpdateError> {
    let mut record = UpdateRecord::load();
    let now = crate::format::now();
    if !force
        && record.is_fresh(now)
        && let Some(release) = record.release()
    {
        return Ok(release);
    }
    match fetch() {
        Ok(body) => {
            let release = Release::parse(&body).map_err(UpdateError::Manifest)?;
            record.fetched_at = now;
            record.manifest = body;
            if let Err(err) = record.save() {
                log::warn!("update cache save: {err}");
            }
            Ok(release)
        }
        Err(err) => match record.release() {
            Some(stale) if !force => {
                log::info!("update check: {err}; using the kept manifest");
                Ok(stale)
            }
            _ => Err(err),
        },
    }
}

fn fetch() -> Result<String, UpdateError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("notinplus/", env!("CARGO_PKG_VERSION")))
        .build()?;
    Ok(client
        .get(MANIFEST_URL)
        .send()?
        .error_for_status()?
        .text()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 24 * 60 * 60 * 1000;

    #[test]
    fn a_manifest_answers_for_two_days() {
        let record = UpdateRecord {
            fetched_at: 10 * DAY,
            manifest: "{}".into(),
            ..UpdateRecord::default()
        };
        assert!(record.is_fresh(10 * DAY));
        assert!(record.is_fresh(12 * DAY - 1));
        assert!(!record.is_fresh(12 * DAY));
        assert!(!UpdateRecord::default().is_fresh(0));
    }
}
