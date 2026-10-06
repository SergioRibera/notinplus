//! Dynamic plugin discovery.
//!
//! On startup the shell scans a well-known directory for every
//! `plugin.ntp.kdl` manifest, loads the matching shared library via
//! [`notein_plugin::load_from_manifest`], and registers the resulting
//! trait objects into the shared [`PluginRegistry`].
//!
//! Scan root resolution:
//! - `NOTINPLUS_PLUGIN_DIR` env var overrides everything (useful for
//!   dev + tests).
//! - Otherwise `istmo::path::data_dir()/notinplus/plugins/` — the same
//!   data_dir the library carves its items out of, so plugins travel
//!   with the install.
//!
//! Layout the scanner expects:
//!
//! ```text
//! <scan_root>/
//! ├── <plugin-id-1>/
//! │   ├── plugin.ntp.kdl
//! │   └── libplugin.so
//! └── <plugin-id-2>/
//!     ├── plugin.ntp.kdl
//!     └── libplugin.so
//! ```
//!
//! Each subdirectory is loaded independently; a failure in one is
//! logged (level `warn`) and does not abort the scan — a single bad
//! `.so` must not brick the shell.

use std::path::{Path, PathBuf};

use notein_plugin::{LoaderError, PluginRegistry};

/// Env var that overrides the default scan root.
pub const PLUGIN_DIR_ENV: &str = "NOTINPLUS_PLUGIN_DIR";

/// One outcome per subdir seen during a scan.
#[derive(Debug)]
pub struct ScanReport {
    pub scanned_root: PathBuf,
    pub loaded: Vec<PathBuf>,
    pub failed: Vec<ScanFailure>,
}

#[derive(Debug)]
pub struct ScanFailure {
    pub manifest_path: PathBuf,
    pub error: ScanFailureKind,
}

#[derive(Debug)]
pub enum ScanFailureKind {
    Load(LoaderError),
    Register(notein_plugin::RegistryError),
}

impl std::fmt::Display for ScanFailureKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Load(e) => write!(f, "{e}"),
            Self::Register(e) => write!(f, "{e}"),
        }
    }
}

/// Resolve the scan root.
#[must_use]
pub fn scan_root() -> PathBuf {
    if let Ok(override_dir) = std::env::var(PLUGIN_DIR_ENV) {
        return PathBuf::from(override_dir);
    }
    istmo::path::data_dir().join("notinplus").join("plugins")
}

/// Load every plugin in `root` into `registry`. Missing root → empty
/// report (first-run installs may never have populated it).
pub fn scan_and_register(registry: &mut PluginRegistry, root: &Path) -> ScanReport {
    let mut report = ScanReport {
        scanned_root: root.to_path_buf(),
        loaded: Vec::new(),
        failed: Vec::new(),
    };

    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return report,
        Err(err) => {
            log::warn!("plugin scan: open {}: {}", root.display(), err);
            return report;
        }
    };

    for entry in entries.flatten() {
        let subdir = entry.path();
        if !subdir.is_dir() {
            continue;
        }
        let manifest_path = subdir.join("plugin.ntp.kdl");
        if !manifest_path.exists() {
            continue;
        }
        match notein_plugin::load_from_manifest(&manifest_path) {
            Ok(loaded) => {
                let path = loaded.manifest_path.clone();
                if let Err(err) = registry.register_dynamic(loaded) {
                    log::warn!("plugin scan: register {}: {err}", path.display());
                    report.failed.push(ScanFailure {
                        manifest_path: path,
                        error: ScanFailureKind::Register(err),
                    });
                } else {
                    log::info!("plugin scan: loaded {}", path.display());
                    report.loaded.push(path);
                }
            }
            Err(err) => {
                log::warn!("plugin scan: load {}: {err}", manifest_path.display());
                report.failed.push(ScanFailure {
                    manifest_path,
                    error: ScanFailureKind::Load(err),
                });
            }
        }
    }

    report
}

/// Convenience wrapper used at startup — scans [`scan_root`] into the
/// supplied registry.
pub fn scan_default(registry: &mut PluginRegistry) -> ScanReport {
    let root = scan_root();
    scan_and_register(registry, &root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_missing_root_returns_empty_report() {
        let mut reg = PluginRegistry::new();
        let fake = PathBuf::from("/tmp/notinplus-nonexistent-plugin-dir-xyz-123");
        let report = scan_and_register(&mut reg, &fake);
        assert!(report.loaded.is_empty());
        assert!(report.failed.is_empty());
    }

    #[test]
    fn scan_skips_subdirs_without_manifest() {
        let tmp = tempdir();
        std::fs::create_dir_all(tmp.join("no-manifest-here")).unwrap();
        std::fs::write(tmp.join("stray-file.txt"), b"ignored").unwrap();

        let mut reg = PluginRegistry::new();
        let report = scan_and_register(&mut reg, &tmp);
        assert!(report.loaded.is_empty());
        assert!(report.failed.is_empty());

        cleanup(&tmp);
    }

    #[test]
    fn scan_reports_manifest_without_library() {
        let tmp = tempdir();
        let plugin_dir = tmp.join("broken");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.ntp.kdl"),
            br#"
id      "org.user.broken"
name    "Broken"
version "0.1.0"
library "libmissing.so"
"#,
        )
        .unwrap();

        let mut reg = PluginRegistry::new();
        let report = scan_and_register(&mut reg, &tmp);
        assert_eq!(report.loaded.len(), 0);
        assert_eq!(report.failed.len(), 1);
        assert!(matches!(
            report.failed[0].error,
            ScanFailureKind::Load(LoaderError::Dlopen { .. })
        ));

        cleanup(&tmp);
    }

    fn tempdir() -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("notinplus-plugin-scan-{unique}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn cleanup(path: &Path) {
        let _ = std::fs::remove_dir_all(path);
    }
}
