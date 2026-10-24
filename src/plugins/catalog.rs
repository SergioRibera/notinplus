//! The process-wide plugin catalog.

use std::sync::{Arc, OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use notein_plugin::catalog::{Failed, Origin, PluginCatalog};
use notein_plugin::erased::ErasedKind;
use notein_plugin::kind::KindInfo;

use super::builtin::{BiblePlugin, CanvasPlugin, NotesPlugin, PdfPlugin};

static CATALOG: OnceLock<RwLock<PluginCatalog>> = OnceLock::new();

fn init() -> RwLock<PluginCatalog> {
    let mut catalog = PluginCatalog::default();
    let builtins: [Arc<dyn notein_plugin::plugin::Plugin>; 3] = [
        Arc::new(CanvasPlugin),
        Arc::new(PdfPlugin),
        Arc::new(NotesPlugin),
    ];
    for plugin in builtins {
        if let Err(err) = catalog.add(plugin, Origin::Builtin) {
            log::error!("builtin plugin: {err}");
        }
    }
    match BiblePlugin::open() {
        Ok(bible) => {
            if let Err(err) = catalog.add(Arc::new(bible), Origin::Builtin) {
                log::error!("builtin plugin: {err}");
            }
        }
        Err(err) => catalog.record_failure(Failed {
            name: "Biblia".to_owned(),
            reason: format!("No se pudo abrir la base de traducciones: {err}"),
        }),
    }
    super::dynamic::load_installed(&mut catalog);
    catalog.install_host(&super::host::handle());
    RwLock::new(catalog)
}

/// Read access to the catalog.
pub fn catalog() -> RwLockReadGuard<'static, PluginCatalog> {
    CATALOG
        .get_or_init(init)
        .read()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Write access (install, enable, disable).
pub fn catalog_mut() -> RwLockWriteGuard<'static, PluginCatalog> {
    CATALOG
        .get_or_init(init)
        .write()
        .unwrap_or_else(PoisonError::into_inner)
}

/// The active kind with id `id`.
#[must_use]
pub fn kind(id: &str) -> Option<Arc<dyn ErasedKind>> {
    catalog().kind(id)
}

/// How kind `id` presents itself, if it is installed and active.
#[must_use]
pub fn kind_info(id: &str) -> Option<KindInfo> {
    kind(id).map(|k| k.info())
}
