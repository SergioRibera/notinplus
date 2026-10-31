//! Third-party plugins on disk: one folder per plugin under the data
//! directory, each with its `plugin.ntp.kdl` manifest and library.

use std::path::{Path, PathBuf};

use notein_i18n::tr;
use notein_plugin::UserMessage;
use notein_plugin::catalog::{Failed, Origin, PluginCatalog};
use notein_plugin::loader::{self, Loaded, LoaderError};

/// Overrides the plugin folder (development).
pub const PLUGIN_DIR_ENV: &str = "NOTINPLUS_PLUGIN_DIR";
const MANIFEST: &str = "plugin.ntp.kdl";

/// Folder holding installed plugins.
#[must_use]
pub fn plugins_dir() -> PathBuf {
    std::env::var_os(PLUGIN_DIR_ENV).map_or_else(
        || istmo::path::data_dir().join("notinplus").join("plugins"),
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
                log::warn!("plugin {}: {err}", manifest.display());
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
            log::warn!("plugin {}: {err}", manifest_path.display());
            catalog.record_failure(Failed {
                name,
                reason: std::sync::Arc::new(err),
            });
        }
    }
}

/// Load a plugin from a folder the user picked, without installing it,
/// so the review sheet can say what it adds.
///
/// # Errors
/// [`LoaderError`] when it is not a compatible plugin.
#[sismo::function("plugins::dynamic_inspect")]
pub fn inspect(folder: &Path) -> Result<Loaded, LoaderError> {
    loader::load(&folder.join(MANIFEST))
}

/// Why installing failed.
#[derive(Debug)]
pub enum InstallError {
    Load(LoaderError),
    Copy(std::io::Error),
    Catalog(notein_plugin::catalog::CatalogError),
    Package(zip::result::ZipError),
    NoManifest,
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Load(err) => write!(f, "load: {err}"),
            Self::Copy(err) => write!(f, "copy: {err}"),
            Self::Catalog(err) => write!(f, "catalog: {err}"),
            Self::Package(err) => write!(f, "package: {err}"),
            Self::NoManifest => write!(f, "package has no {MANIFEST}"),
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
            Self::NoManifest => None,
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
    drop(probe);
    copy_dir(folder, &target).map_err(InstallError::Copy)?;
    let manifest = target.join(MANIFEST);
    let loaded = loader::load(&manifest).map_err(InstallError::Load)?;
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
        let mut archive =
            zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(InstallError::Package)?;
        archive
            .extract(&staged.root)
            .map_err(InstallError::Package)?;
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

impl Drop for Staged {
    fn drop(&mut self) {
        sismo::scope!("drop::plugin_staging");
        if let Err(err) = std::fs::remove_dir_all(&self.root) {
            log::warn!("plugin staging {}: {err}", self.root.display());
        }
    }
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}
