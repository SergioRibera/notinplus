//! Asking GitHub for the newest release, at most every couple of days.
//!
//! GitHub's `releases/latest` never names a pre-release, so the check
//! reads the list of releases, picks the newest this build should move
//! to (see [`pick`]) and fetches the `latest.json` published with it.
//! The last one is kept on disk with the time it was fetched. Until it
//! is [`MAX_AGE`] old the app answers from it; past that, the next check
//! fetches again and replaces it. A manual check always fetches.

use std::time::Duration;

use serde::Deserialize;

use super::{INSTALLED, Release, UpdateError};
use crate::persist::Persisted;

/// The repository's releases, newest first.
const RELEASES_URL: &str =
    "https://api.github.com/repos/SergioRibera/notinplus/releases?per_page=30";

/// The manifest each release carries.
const MANIFEST: &str = "latest.json";

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
#[sismo::function("update::check_latest")]
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

/// A release as the GitHub API lists it; only what [`pick`] reads.
#[derive(Debug, Deserialize)]
struct Listed {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ListedAsset>,
}

#[derive(Debug, Deserialize)]
struct ListedAsset {
    name: String,
    browser_download_url: String,
}

/// The `latest.json` of the newest release this build should move to:
/// published (not a draft), carrying a manifest, and stable unless this
/// build is a pre-release itself, so someone on a stable version is
/// never offered an alpha.
fn pick<'a>(releases: &'a [Listed], installed: &str) -> Option<&'a str> {
    let on_prerelease = semver::Version::parse(installed).is_ok_and(|v| !v.pre.is_empty());
    releases
        .iter()
        .filter(|r| !r.draft && (on_prerelease || !r.prerelease))
        .filter_map(|r| {
            let version = semver::Version::parse(r.tag_name.trim_start_matches('v')).ok()?;
            let manifest = r.assets.iter().find(|a| a.name == MANIFEST)?;
            Some((version, manifest.browser_download_url.as_str()))
        })
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, url)| url)
}

#[sismo::function("update::fetch_manifest")]
fn fetch() -> Result<String, UpdateError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(concat!("notinplus/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let listing = client
        .get(RELEASES_URL)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()?
        .error_for_status()?
        .text()?;
    let releases: Vec<Listed> = serde_json::from_str(&listing).map_err(UpdateError::Manifest)?;
    let url = pick(&releases, INSTALLED).ok_or(UpdateError::NoRelease)?;
    Ok(client.get(url).send()?.error_for_status()?.text()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 24 * 60 * 60 * 1000;

    /// A page of the API as GitHub answers it, trimmed to what is read.
    const LISTING: &str = r#"[
      {"tag_name": "v0.2.0-alpha.1", "draft": true, "prerelease": true,
       "assets": [{"name": "latest.json", "browser_download_url": "draft"}]},
      {"tag_name": "v0.1.0-alpha.3", "draft": false, "prerelease": true,
       "assets": [{"name": "SHA256SUMS", "browser_download_url": "sums"},
                  {"name": "latest.json", "browser_download_url": "alpha.3"}]},
      {"tag_name": "v0.1.0-alpha.10", "draft": false, "prerelease": true,
       "assets": [{"name": "latest.json", "browser_download_url": "alpha.10"}]},
      {"tag_name": "v0.0.9", "draft": false, "prerelease": false,
       "assets": [{"name": "latest.json", "browser_download_url": "0.0.9"}]},
      {"tag_name": "v0.1.0-alpha.11", "draft": false, "prerelease": true,
       "assets": []}
    ]"#;

    #[test]
    fn a_prerelease_build_moves_to_the_newest_published_release_by_version() {
        let releases: Vec<Listed> = serde_json::from_str(LISTING).unwrap();
        // Not the draft, not the one without a manifest, and `alpha.10`
        // above `alpha.3` whatever the list order.
        assert_eq!(pick(&releases, "0.1.0-alpha.3"), Some("alpha.10"));
    }

    #[test]
    fn a_stable_build_is_only_offered_stable_releases() {
        let releases: Vec<Listed> = serde_json::from_str(LISTING).unwrap();
        assert_eq!(pick(&releases, "0.0.8"), Some("0.0.9"));
        let only_alphas: Vec<Listed> = releases.into_iter().filter(|r| r.prerelease).collect();
        assert_eq!(pick(&only_alphas, "0.0.8"), None);
    }

    #[test]
    #[ignore = "asks GitHub"]
    fn the_published_releases_answer() {
        let release = Release::parse(&fetch().expect("fetch")).expect("manifest");
        assert!(!release.version.is_empty());
        assert!(!release.assets.is_empty());
    }

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
