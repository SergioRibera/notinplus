//! Third-party plugins on disk: one folder per plugin under the data
//! directory, each with its `plugin.ntp.kdl` manifest and library.

use std::path::{Path, PathBuf};

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
                    reason: err.user_message(),
                });
            }
        }
    }
}

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
                reason: err.to_string(),
            });
        }
    }
}

/// Load a plugin from a folder the user picked, without installing it,
/// so the review sheet can say what it adds.
///
/// # Errors
/// [`LoaderError`] when it is not a compatible plugin.
pub fn inspect(folder: &Path) -> Result<Loaded, LoaderError> {
    loader::load(&folder.join(MANIFEST))
}

/// Why installing failed.
#[derive(Debug)]
pub enum InstallError {
    Load(LoaderError),
    Copy(std::io::Error),
    Catalog(notein_plugin::catalog::CatalogError),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Load(err) => f.write_str(&err.user_message()),
            Self::Copy(err) => write!(f, "No se pudo copiar el plugin: {err}"),
            Self::Catalog(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for InstallError {}

/// Copy the plugin in `folder` into the plugins directory and load it
/// right away. Returns the plugin name.
///
/// # Errors
/// [`InstallError`].
pub fn install(folder: &Path) -> Result<String, InstallError> {
    let probe = inspect(folder).map_err(InstallError::Load)?;
    let target = plugins_dir().join(sanitize(&probe.manifest.id));
    drop(probe);
    copy_dir(folder, &target).map_err(InstallError::Copy)?;
    let manifest = target.join(MANIFEST);
    let loaded = loader::load(&manifest).map_err(InstallError::Load)?;
    let name = loaded.manifest.name.clone();
    let origin = Origin::ThirdParty {
        manifest: Box::new(loaded.manifest.clone()),
        library: loaded.library_path.clone(),
    };
    let mut catalog = super::catalog::catalog_mut();
    catalog
        .add(loaded.plugin, origin)
        .map_err(InstallError::Catalog)?;
    catalog.keep_library(loaded.library);
    if let Some(installed) = catalog.plugin_by_name(&name) {
        installed.install_host(&super::host::handle());
    }
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
