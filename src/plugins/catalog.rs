//! The process-wide plugin catalog.

use std::sync::{Arc, OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use notein_i18n::{Lang, tr};
use notein_plugin::UserMessage;
use notein_plugin::catalog::{Failed, Origin, PluginCatalog};
use notein_plugin::erased::ErasedKind;
use notein_plugin::kind::KindInfo;

use super::builtin::{CanvasPlugin, PdfPlugin};

static CATALOG: OnceLock<RwLock<PluginCatalog>> = OnceLock::new();

fn init() -> RwLock<PluginCatalog> {
    let mut catalog = PluginCatalog::default();
    let builtins: [Arc<dyn notein_plugin::plugin::Plugin>; 3] = [
        Arc::new(CanvasPlugin),
        Arc::new(PdfPlugin),
        Arc::new(notein_notes::NotesPlugin::default()),
    ];
    for plugin in builtins {
        if let Err(err) = catalog.add(plugin, Origin::Builtin) {
            log::error!("builtin plugin: {err}");
        }
    }
    match notein_bible::BiblePlugin::open() {
        Ok(bible) => {
            if let Err(err) = catalog.add(Arc::new(bible), Origin::Builtin) {
                log::error!("builtin plugin: {err}");
            }
        }
        Err(err) => {
            log::error!("bible plugin: {err}");
            catalog.record_failure(Failed {
                name: tr!("plugin-host-bible-name"),
                reason: Arc::new(BibleUnavailable(err)),
            });
        }
    }
    super::dynamic::load_installed(&mut catalog);
    for id in <crate::settings::Settings as crate::persist::Persisted>::load()
        .plugins
        .disabled
    {
        catalog.set_enabled(&id, false);
    }
    catalog.install_host(&super::host::handle());
    RwLock::new(catalog)
}

/// The Bible's database could not be opened, so the builtin is missing.
#[derive(Debug)]
struct BibleUnavailable(notein_bible::db::DbError);

impl UserMessage for BibleUnavailable {
    fn user_message(&self) -> String {
        tr!("errors-plugin-bible-unavailable", "reason" => self.0.user_message())
    }
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

/// Switch every loaded plugin to `lang`. A no-op before the catalog
/// is first used: plugins pick up the current language as they load.
pub fn set_lang(lang: Lang) {
    if let Some(catalog) = CATALOG.get() {
        catalog
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .set_lang(lang);
    }
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
