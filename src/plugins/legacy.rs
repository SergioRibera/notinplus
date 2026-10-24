//! The source-plugin registry the PDF text engine and reference chips
//! still go through.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use notein_plugin::PluginRegistry;

static SHARED: OnceLock<Arc<Mutex<PluginRegistry>>> = OnceLock::new();

/// Process-wide source registry, built on first use.
#[must_use]
pub fn registry() -> Arc<Mutex<PluginRegistry>> {
    Arc::clone(SHARED.get_or_init(|| {
        let mut reg = PluginRegistry::new();
        crate::builtin::register_all(&mut reg);
        let report = crate::dynamic_plugins::scan_default(&mut reg);
        log::info!(
            "dynamic plugin scan: {} loaded, {} failed, root={}",
            report.loaded.len(),
            report.failed.len(),
            report.scanned_root.display(),
        );
        crate::host_api::install_host_in_registry(&reg);
        Arc::new(Mutex::new(reg))
    }))
}

/// Lock the registry, recovering from poisoning: every critical section
/// is short and panic-free.
pub fn lock(reg: &Arc<Mutex<PluginRegistry>>) -> MutexGuard<'_, PluginRegistry> {
    reg.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
