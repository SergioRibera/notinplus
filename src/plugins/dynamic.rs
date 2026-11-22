//! Third-party plugins on disk: one folder per plugin under the data
//! directory, each with its `plugin.ntp.kdl` manifest and library.

use std::path::{Path, PathBuf};

use notin_i18n::tr;
use notin_plugin::UserMessage;
use notin_plugin::catalog::{Failed, Origin, PluginCatalog};
use notin_plugin::loader::{self, Loaded, LoaderError, Resolved};

/// Overrides the plugin folder (development).
pub const PLUGIN_DIR_ENV: &str = "NOTINPLUS_PLUGIN_DIR";
const MANIFEST: &str = "plugin.ntp.kdl";

/// Folder holding installed plugins.
#[must_use]
pub fn plugins_dir() -> PathBuf {
    std::env::var_os(PLUGIN_DIR_ENV).map_or_else(
        || crate::paths::data_dir().join("notinplus").join("plugins"),
        PathBuf::from,
    )
}

/// Load every installed plugin into `catalog`; failures are recorded
/// so Settings can show them.
#[sismo::function("plugins::dynamic_load_installed")]
pub fn load_installed(catalog: &mut PluginCatalog) {
    let Ok(entries) = std::fs::read_dir(plugins_dir()) else {
        return;
    };
    for dir in entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
        let manifest = dir.join(MANIFEST);
        if !manifest.exists() {
            continue;
        }
        match loader::load(&manifest) {
            Ok(loaded) => register(catalog, loaded, &manifest),
            Err(err) => {
                tracing::warn!("plugin {}: {err}", manifest.display());
                catalog.record_failure(Failed {
                    name: dir
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    reason: std::sync::Arc::new(err),
                });
            }
        }
    }
}

#[sismo::function("plugins::dynamic_register")]
fn register(catalog: &mut PluginCatalog, loaded: Loaded, manifest_path: &Path) {
    let origin = Origin::ThirdParty {
        manifest: Box::new(loaded.manifest.clone()),
        library: loaded.library_path.clone(),
    };
    let name = loaded.manifest.name.clone();
    match catalog.add(loaded.plugin, origin) {
        Ok(()) => catalog.keep_library(loaded.library),
        Err(err) => {
            tracing::warn!("plugin {}: {err}", manifest_path.display());
            catalog.record_failure(Failed {
                name,
                reason: std::sync::Arc::new(err),
            });
        }
    }
}

/// Read the manifest of a plugin the user picked and check it ships a
/// library for this device. None of its code runs: that waits for
/// [`install`].
///
/// # Errors
/// [`LoaderError`] when the manifest is unreadable or names no library
/// for this device.
#[sismo::function("plugins::dynamic_inspect")]
pub fn inspect(folder: &Path) -> Result<Resolved, LoaderError> {
    loader::resolve(&folder.join(MANIFEST))
}

/// Why installing failed.
#[derive(Debug)]
pub enum InstallError {
    Load(LoaderError),
    Copy(std::io::Error),
    Catalog(notin_plugin::catalog::CatalogError),
    Package(zip::result::ZipError),
    NoManifest,
    /// The package unpacks to more files or bytes than a plugin needs.
    TooLarge,
    /// The plugin registered something else than its manifest declares.
    Undeclared(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Load(err) => write!(f, "load: {err}"),
            Self::Copy(err) => write!(f, "copy: {err}"),
            Self::Catalog(err) => write!(f, "catalog: {err}"),
            Self::Package(err) => write!(f, "package: {err}"),
            Self::NoManifest => write!(f, "package has no {MANIFEST}"),
            Self::TooLarge => f.write_str("package unpacks too large"),
            Self::Undeclared(name) => write!(f, "{name} adds something else than it declares"),
        }
    }
}

impl std::error::Error for InstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Load(err) => Some(err),
            Self::Copy(err) => Some(err),
            Self::Catalog(err) => Some(err),
            Self::Package(err) => Some(err),
            Self::NoManifest | Self::TooLarge | Self::Undeclared(_) => None,
        }
    }
}

impl UserMessage for InstallError {
    fn user_message(&self) -> String {
        match self {
            Self::Load(err) => err.user_message(),
            Self::Copy(err) | Self::Package(zip::result::ZipError::Io(err)) => {
                tr!("errors-plugin-copy", "reason" => err.user_message())
            }
            Self::Catalog(err) => err.user_message(),
            Self::Package(_) => tr!("errors-plugin-not-a-package"),
            Self::NoManifest => tr!("errors-plugin-no-manifest", "manifest" => MANIFEST),
            Self::TooLarge => tr!("errors-plugin-too-large"),
            Self::Undeclared(name) => tr!("errors-plugin-undeclared", "name" => name.as_str()),
        }
    }
}

/// Copy the plugin in `folder` into the plugins directory and load it
/// right away. Returns the plugin name.
///
/// # Errors
/// [`InstallError`].
#[sismo::function("plugins::dynamic_install")]
pub fn install(folder: &Path) -> Result<String, InstallError> {
    let probe = inspect(folder).map_err(InstallError::Load)?;
    let target = plugins_dir().join(sanitize(&probe.manifest.id));
    copy_dir(folder, &target).map_err(InstallError::Copy)?;
    let manifest = target.join(MANIFEST);
    let discard = |err: InstallError| {
        if let Err(io) = std::fs::remove_dir_all(&target) {
            tracing::warn!("plugin {}: {io}", target.display());
        }
        err
    };
    // The first time any of the plugin's code runs.
    let loaded = loader::load(&manifest).map_err(|err| discard(InstallError::Load(err)))?;
    // A plugin that adds something else than its manifest showed in the
    // review is not kept.
    if let Some(declared) = &loaded.manifest.declared
        && let Some(mismatch) = declared.mismatch(&loaded.declared())
    {
        tracing::warn!(
            "plugin {}: registers {:?}, declares but lacks {:?}",
            loaded.manifest.id,
            mismatch.undeclared,
            mismatch.missing
        );
        let name = loaded.manifest.name.clone();
        drop(loaded);
        return Err(discard(InstallError::Undeclared(name)));
    }
    let name = loaded.manifest.name.clone();
    let id = loaded.plugin.info().id;
    let origin = Origin::ThirdParty {
        manifest: Box::new(loaded.manifest.clone()),
        library: loaded.library_path.clone(),
    };
    let mut catalog = super::catalog::catalog_mut();
    catalog
        .add(loaded.plugin, origin)
        .map_err(InstallError::Catalog)?;
    catalog.keep_library(loaded.library);
    if let Some(installed) = catalog.plugin(&id) {
        installed.install_host(&super::host::handle());
    }
    drop(catalog);
    Ok(name)
}

fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[sismo::function("plugins::dynamic_copy_dir")]
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

/// A plugin package unpacked into a temporary folder, removed on drop.
#[derive(Debug)]
pub struct Staged {
    root: PathBuf,
    plugin: PathBuf,
}

impl Staged {
    /// Unpack a `.ntp` package (a zip holding the plugin folder).
    ///
    /// # Errors
    /// [`InstallError::Package`] when it is not a readable package, or
    /// [`InstallError::Copy`] when the files cannot be written.
    #[sismo::function("plugins::dynamic_unpack")]
    pub fn unpack(bytes: &[u8]) -> Result<Self, InstallError> {
        let root = plugins_dir()
            .join(".staging")
            .join(format!("{:x}", rand_suffix()));
        std::fs::create_dir_all(&root).map_err(InstallError::Copy)?;
        let staged = Self {
            plugin: root.clone(),
            root,
        };
        extract_capped(bytes, &staged.root, MAX_UNPACKED)?;
        if staged.root.join(MANIFEST).exists() {
            return Ok(staged);
        }
        let inner = std::fs::read_dir(&staged.root)
            .map_err(InstallError::Copy)?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.join(MANIFEST).exists())
            .ok_or(InstallError::NoManifest)?;
        Ok(Self {
            plugin: inner,
            root: staged.root.clone(),
        })
    }

    /// Folder holding the plugin manifest.
    #[must_use]
    pub fn folder(&self) -> &Path {
        &self.plugin
    }
}

/// Most files a package may hold.
const MAX_ENTRIES: usize = 4096;
/// Most bytes a package may unpack to: one library per platform fits
/// well below it, a zip bomb does not.
const MAX_UNPACKED: u64 = 512 * 1024 * 1024;

/// Unpack `bytes` into `root`: regular files and folders only, every
/// path inside `root`, at most `max_bytes` whatever the archive claims.
#[sismo::function("plugins::dynamic_extract_capped")]
fn extract_capped(bytes: &[u8], root: &Path, max_bytes: u64) -> Result<(), InstallError> {
    use std::io::Read as _;

    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(InstallError::Package)?;
    if archive.len() > MAX_ENTRIES {
        return Err(InstallError::TooLarge);
    }
    let mut left = max_bytes;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(InstallError::Package)?;
        let Some(relative) = entry.enclosed_name() else {
            return Err(InstallError::Package(
                zip::result::ZipError::InvalidArchive("path outside the package".into()),
            ));
        };
        let dest = root.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&dest).map_err(InstallError::Copy)?;
            continue;
        }
        if entry.is_symlink() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(InstallError::Copy)?;
        }
        let mut out = std::fs::File::create(&dest).map_err(InstallError::Copy)?;
        // One byte over what is left tells a lying header from a fit.
        let written = std::io::copy(&mut (&mut entry).take(left + 1), &mut out)
            .map_err(InstallError::Copy)?;
        left = left.checked_sub(written).ok_or(InstallError::TooLarge)?;
    }
    Ok(())
}

impl Drop for Staged {
    fn drop(&mut self) {
        sismo::scope!("drop::plugin_staging");
        if let Err(err) = std::fs::remove_dir_all(&self.root) {
            tracing::warn!("plugin staging {}: {err}", self.root.display());
        }
    }
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    fn package(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, body) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "notinplus-unpack-{name}-{}-{:x}",
            std::process::id(),
            rand_suffix()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn unpacks_within_the_cap() {
        let root = scratch("fits");
        let bytes = package(&[
            ("plugin/plugin.ntp.kdl", b"id \"a\""),
            ("plugin/lib.so", &[0; 64]),
        ]);
        extract_capped(&bytes, &root, 1024).unwrap();
        assert!(root.join("plugin/plugin.ntp.kdl").is_file());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_more_than_the_cap() {
        let root = scratch("bomb");
        let bytes = package(&[("a.bin", &[0; 600]), ("b.bin", &[0; 600])]);
        assert!(matches!(
            extract_capped(&bytes, &root, 1000),
            Err(InstallError::TooLarge)
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_paths_outside_the_package() {
        let root = scratch("escape");
        let bytes = package(&[("../escaped.txt", b"x")]);
        assert!(extract_capped(&bytes, &root, 1024).is_err());
        assert!(!root.parent().unwrap().join("escaped.txt").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
