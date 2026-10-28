//! `latest.json`, the update manifest every stable release publishes,
//! and which of its files fits this device.

use std::cmp::Ordering;

use serde::Deserialize;

/// The newest stable release, as `latest.json` describes it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Release {
    /// `0.2.0`, without the `v`.
    pub version: String,
    /// Release page on GitHub.
    pub url: String,
    /// RFC 3339 time it was published.
    #[serde(default)]
    pub published_at: String,
    /// Markdown with this release's changes.
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// One downloadable file of a release.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Asset {
    /// `android`, `linux`, `macos` or `windows`.
    pub platform: String,
    /// `x86_64`, `aarch64`, `universal` or `unknown`.
    pub arch: String,
    /// Package format: `apk`, `appimage`, `deb`, `nsis`, `dmg`…
    pub kind: String,
    /// File name.
    pub name: String,
    pub url: String,
    /// Bytes.
    pub size: u64,
    /// Lower-case hex SHA-256 of the file.
    pub sha256: String,
}

impl Release {
    /// Read a `latest.json` body.
    ///
    /// # Errors
    /// The body is not the manifest.
    pub fn parse(body: &str) -> serde_json::Result<Self> {
        serde_json::from_str(body)
    }

    /// Whether this release is newer than `installed`. Versions that
    /// don't parse are never newer.
    #[must_use]
    pub fn is_newer_than(&self, installed: &str) -> bool {
        match (
            semver::Version::parse(&self.version),
            semver::Version::parse(installed),
        ) {
            (Ok(release), Ok(installed)) => release.cmp(&installed) == Ordering::Greater,
            _ => false,
        }
    }

    /// When it was published ("ayer", "26/10"), if the manifest says.
    #[must_use]
    pub fn published_on(&self) -> Option<String> {
        let date = chrono::DateTime::parse_from_rfc3339(&self.published_at).ok()?;
        let ms = u64::try_from(date.timestamp_millis()).ok()?;
        Some(crate::format::day(ms, crate::format::now()))
    }

    /// Files `device` can use, best first.
    #[must_use]
    pub fn assets_for(&self, device: &Device) -> Vec<&Asset> {
        let mut fits: Vec<_> = self
            .assets
            .iter()
            .filter_map(|asset| device.rank(asset).map(|rank| (rank, asset)))
            .collect();
        fits.sort_by_key(|(rank, _)| *rank);
        fits.into_iter().map(|(_, asset)| asset).collect()
    }
}

impl Asset {
    /// How the format reads in a menu: `AppImage`, `.deb`, `APK`…
    #[must_use]
    pub fn format_label(&self) -> &str {
        match self.kind.as_str() {
            "apk" => "APK",
            "appimage" => "AppImage",
            "deb" => ".deb",
            "rpm" => ".rpm",
            "flatpak" => "Flatpak",
            "tar.gz" => ".tar.gz",
            "dmg" => "Imagen de disco (.dmg)",
            "pkg" => "Instalador (.pkg)",
            "app" => "App (.zip)",
            "nsis" => "Instalador (.exe)",
            "msi" => "Instalador MSI",
            "zip" => "Portátil (.zip)",
            other => other,
        }
    }
}

/// What a device runs, for picking its file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    platform: &'static str,
    arch: &'static str,
    /// Package formats in order of preference.
    kinds: Vec<&'static str>,
}

impl Device {
    /// The device the app runs on.
    #[must_use]
    pub fn current() -> Self {
        let platform = std::env::consts::OS;
        let kinds = match platform {
            "android" => vec!["apk"],
            "windows" => vec!["nsis", "msi", "zip"],
            "macos" => vec!["dmg", "pkg", "app", "tar.gz"],
            _ => linux_kinds(),
        };
        Self {
            platform,
            arch: std::env::consts::ARCH,
            kinds,
        }
    }

    /// `None` when `asset` does not run here; lower is better.
    fn rank(&self, asset: &Asset) -> Option<(usize, u8)> {
        if asset.platform != self.platform {
            return None;
        }
        let kind = self.kinds.iter().position(|k| *k == asset.kind)?;
        let arch = if asset.arch == self.arch {
            0
        } else if asset.arch == "universal" {
            1
        } else {
            return None;
        };
        Some((kind, arch))
    }
}

/// Linux formats, the one this copy was installed with first.
fn linux_kinds() -> Vec<&'static str> {
    let installed = if std::env::var_os("APPIMAGE").is_some() {
        "appimage"
    } else if std::env::var_os("FLATPAK_ID").is_some() {
        "flatpak"
    } else if std::path::Path::new("/etc/debian_version").exists() {
        "deb"
    } else if std::path::Path::new("/etc/redhat-release").exists()
        || std::path::Path::new("/etc/fedora-release").exists()
    {
        "rpm"
    } else {
        "appimage"
    };
    let mut kinds = vec![installed];
    kinds.extend(
        ["appimage", "deb", "rpm", "flatpak", "tar.gz"]
            .into_iter()
            .filter(|kind| *kind != installed),
    );
    kinds
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r###"{
        "version": "0.2.0", "tag": "v0.2.0", "published_at": "2026-10-26T12:00:00Z",
        "url": "https://github.com/SergioRibera/notinplus/releases/tag/v0.2.0",
        "notes": "## 0.2.0\n### New\n- **sync:** faster",
        "assets": [
            {"platform": "android", "arch": "universal", "kind": "apk", "name": "u.apk", "url": "https://x/u.apk", "size": 90, "sha256": "aa"},
            {"platform": "android", "arch": "aarch64", "kind": "apk", "name": "a.apk", "url": "https://x/a.apk", "size": 40, "sha256": "bb"},
            {"platform": "linux", "arch": "x86_64", "kind": "deb", "name": "n.deb", "url": "https://x/n.deb", "size": 60, "sha256": "cc"},
            {"platform": "linux", "arch": "x86_64", "kind": "appimage", "name": "n.AppImage", "url": "https://x/n.AppImage", "size": 92, "sha256": "dd"},
            {"platform": "linux", "arch": "aarch64", "kind": "appimage", "name": "arm.AppImage", "url": "https://x/arm.AppImage", "size": 92, "sha256": "ee"}
        ]
    }"###;

    fn device(platform: &'static str, arch: &'static str, kinds: &[&'static str]) -> Device {
        Device {
            platform,
            arch,
            kinds: kinds.to_vec(),
        }
    }

    #[test]
    fn reads_the_manifest_the_release_workflow_writes() {
        let release = Release::parse(MANIFEST).expect("manifest");
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.assets.len(), 5);
        assert!(release.notes.starts_with("## 0.2.0"));
    }

    #[test]
    fn only_a_higher_version_is_newer() {
        let release = Release::parse(MANIFEST).expect("manifest");
        assert!(release.is_newer_than("0.1.9"));
        assert!(!release.is_newer_than("0.2.0"));
        assert!(!release.is_newer_than("0.3.0-beta.1"));
        assert!(!release.is_newer_than("garbage"));
    }

    #[test]
    fn an_exact_architecture_beats_the_universal_file() {
        let release = Release::parse(MANIFEST).expect("manifest");
        let phone = device("android", "aarch64", &["apk"]);
        let names: Vec<_> = release
            .assets_for(&phone)
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(names, ["a.apk", "u.apk"]);
        let emulator = device("android", "x86", &["apk"]);
        let names: Vec<_> = release
            .assets_for(&emulator)
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(names, ["u.apk"]);
    }

    #[test]
    fn the_installed_format_comes_first_and_other_machines_are_left_out() {
        let release = Release::parse(MANIFEST).expect("manifest");
        let laptop = device(
            "linux",
            "x86_64",
            &["appimage", "deb", "rpm", "flatpak", "tar.gz"],
        );
        let names: Vec<_> = release
            .assets_for(&laptop)
            .iter()
            .map(|a| a.name.as_str())
            .collect();
        assert_eq!(names, ["n.AppImage", "n.deb"]);
        let windows = device("windows", "x86_64", &["nsis", "msi", "zip"]);
        assert!(release.assets_for(&windows).is_empty());
    }
}
