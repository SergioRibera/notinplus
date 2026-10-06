//! Process-wide [`PluginRegistry`] + lifecycle helpers.
//!
//! The shell holds a single registry so every subsystem (home FAB,
//! settings modal, canvas routing) observes the same plugins. The
//! `Arc<Mutex<_>>` wrapper mirrors `canvas::Board::shared()` — short
//! lock scopes, poisoning silently recovered.
//!
//! Phase 2.4 stops here: built-in plugins land in Phase 2.6 / 2.7 and
//! will register through [`registry()`] at startup.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use notein_plugin::PluginRegistry;

static SHARED: OnceLock<Arc<Mutex<PluginRegistry>>> = OnceLock::new();

/// Process-wide registry handle. Lazily constructed on first call.
#[must_use]
pub fn registry() -> Arc<Mutex<PluginRegistry>> {
    Arc::clone(SHARED.get_or_init(|| Arc::new(Mutex::new(PluginRegistry::new()))))
}

/// Mirror of [`crate::canvas::lock`] — short critical sections, no
/// panic-prone work inside the guard, so poisoning is forward-progress
/// recoverable.
pub fn lock(reg: &Arc<Mutex<PluginRegistry>>) -> MutexGuard<'_, PluginRegistry> {
    match reg.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Load persisted plugin settings and apply them to the global
/// registry. Idempotent — safe to call multiple times (e.g. after
/// registering a new plugin at runtime).
pub fn hydrate_from_disk() {
    let settings = crate::plugin_settings::load();
    let reg = registry();
    let mut guard = lock(&reg);
    crate::plugin_settings::apply(&settings, &mut guard);
}

/// Snapshot the global registry and persist. Called from the settings
/// modal whenever the user toggles enablement or edits a field.
pub fn persist_to_disk() {
    let reg = registry();
    let guard = lock(&reg);
    let snapshot = crate::plugin_settings::collect(&guard);
    drop(guard);
    if let Err(err) = crate::plugin_settings::save(&snapshot) {
        log::warn!("save plugin_settings failed: {err}");
    }
}
