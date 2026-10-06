//! Process-wide [`PluginRegistry`] + lifecycle helpers.
//!
//! The shell holds a single registry so every subsystem (home FAB,
//! settings modal, canvas routing) observes the same plugins. The
//! `Arc<Mutex<_>>` wrapper mirrors `canvas::Board::shared()` — short
//! lock scopes, poisoning silently recovered.
//!
//! A sibling `FabShortcutState` tracks the two shortcut buttons above
//! the home FAB. Shortcuts reference creation entries by their
//! `(plugin_id, entry_id)` pair so they survive plugin reloads and the
//! Phase-2.6 shell-builtin → real-plugin migration.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use notein_plugin::{CreationCategory, CreationEntry, PluginRegistry, PluginRole};

use crate::plugin_settings::{FabShortcut, FabShortcutSlot};
use crate::shell_builtins;

static SHARED: OnceLock<Arc<Mutex<PluginRegistry>>> = OnceLock::new();
static FAB_SHORTCUTS: OnceLock<Arc<Mutex<FabShortcutState>>> = OnceLock::new();

/// Shortcut slots the home FAB renders above the main `+` button.
#[derive(Clone, Debug)]
pub struct FabShortcutState {
    /// Fixed-size array for the two visible slots. `None` = empty
    /// bubble (either default-cleared or user-cleared).
    pub slots: [Option<FabShortcut>; 2],
    /// `true` once the user has touched the shortcuts explicitly;
    /// `false` means `slots` holds the shell defaults. Save-to-disk
    /// only triggers when `touched == true`.
    pub touched: bool,
}

impl Default for FabShortcutState {
    fn default() -> Self {
        Self {
            slots: default_slots(),
            touched: false,
        }
    }
}

fn default_slots() -> [Option<FabShortcut>; 2] {
    [
        Some(FabShortcut {
            plugin_id: shell_builtins::BUILTIN_PLUGIN_ID.to_owned(),
            entry_id: shell_builtins::NEW_EPHEMERAL.to_owned(),
        }),
        Some(FabShortcut {
            plugin_id: shell_builtins::BUILTIN_PLUGIN_ID.to_owned(),
            entry_id: shell_builtins::QUICK_SKETCH.to_owned(),
        }),
    ]
}

/// Process-wide registry handle. Lazily constructed on first call.
#[must_use]
pub fn registry() -> Arc<Mutex<PluginRegistry>> {
    Arc::clone(SHARED.get_or_init(|| Arc::new(Mutex::new(PluginRegistry::new()))))
}

/// Process-wide FAB shortcut handle. Lazily constructed on first call.
#[must_use]
pub fn fab_shortcuts_handle() -> Arc<Mutex<FabShortcutState>> {
    Arc::clone(FAB_SHORTCUTS.get_or_init(|| Arc::new(Mutex::new(FabShortcutState::default()))))
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

fn lock_shortcuts(h: &Arc<Mutex<FabShortcutState>>) -> MutexGuard<'_, FabShortcutState> {
    match h.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Snapshot of the two FAB shortcut slots. Caller gets `None` for
/// slots the user has cleared.
#[must_use]
pub fn fab_shortcut_slots() -> [Option<FabShortcut>; 2] {
    let h = fab_shortcuts_handle();
    let g = lock_shortcuts(&h);
    g.slots.clone()
}

/// Update one shortcut slot and persist. `index` must be `0` or `1`.
/// `slot` carries `None` to clear or `Some(FabShortcut)` to pick.
pub fn set_fab_shortcut(index: usize, slot: Option<FabShortcut>) {
    if index > 1 {
        return;
    }
    {
        let h = fab_shortcuts_handle();
        let mut g = lock_shortcuts(&h);
        g.slots[index] = slot;
        g.touched = true;
    }
    persist_to_disk();
}

/// Load persisted plugin settings and apply them to the global
/// registry + shortcut state. Idempotent — safe to call multiple
/// times (e.g. after registering a new plugin at runtime).
pub fn hydrate_from_disk() {
    let settings = crate::plugin_settings::load();
    let reg = registry();
    let mut guard = lock(&reg);
    crate::plugin_settings::apply(&settings, &mut guard);
    drop(guard);
    hydrate_shortcuts(&settings.fab_shortcuts);
}

fn hydrate_shortcuts(persisted: &[FabShortcutSlot]) {
    let h = fab_shortcuts_handle();
    let mut g = lock_shortcuts(&h);
    if persisted.len() != 2 {
        g.slots = default_slots();
        g.touched = false;
        return;
    }
    g.slots = [
        slot_to_option(&persisted[0]),
        slot_to_option(&persisted[1]),
    ];
    g.touched = true;
}

fn slot_to_option(s: &FabShortcutSlot) -> Option<FabShortcut> {
    match s {
        FabShortcutSlot::Empty => None,
        FabShortcutSlot::Entry(e) => Some(e.clone()),
    }
}

/// Snapshot the global registry + shortcut state and persist. Called
/// from the settings modal whenever the user toggles enablement or
/// edits a field.
pub fn persist_to_disk() {
    let reg = registry();
    let guard = lock(&reg);
    let mut snapshot = crate::plugin_settings::collect(&guard);
    drop(guard);
    snapshot.fab_shortcuts = snapshot_shortcuts();
    if let Err(err) = crate::plugin_settings::save(&snapshot) {
        log::warn!("save plugin_settings failed: {err}");
    }
}

fn snapshot_shortcuts() -> Vec<FabShortcutSlot> {
    let h = fab_shortcuts_handle();
    let g = lock_shortcuts(&h);
    if !g.touched {
        return Vec::new();
    }
    g.slots
        .iter()
        .map(|s| match s {
            None => FabShortcutSlot::Empty,
            Some(e) => FabShortcutSlot::Entry(e.clone()),
        })
        .collect()
}

/// Unified entry metadata for the FAB `+` menu + shortcut picker.
/// Merges [`shell_builtins::entries`] with the registry's
/// [`PluginRegistry::list_creation_entries`] into one list so UI code
/// stays blind to the shell/plugin split.
#[derive(Clone, Debug)]
pub struct AvailableEntry {
    pub plugin_id: String,
    pub plugin_label: String,
    pub entry: CreationEntry,
    pub source: EntrySource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntrySource {
    ShellBuiltin,
    Plugin(PluginRole),
}

/// Every creation entry currently available to the user. Shell
/// builtins come first; plugin entries follow in registry order.
#[must_use]
pub fn available_entries() -> Vec<AvailableEntry> {
    let mut out = Vec::new();
    for entry in shell_builtins::entries() {
        out.push(AvailableEntry {
            plugin_id: shell_builtins::BUILTIN_PLUGIN_ID.to_owned(),
            plugin_label: "notinplus".to_owned(),
            entry,
            source: EntrySource::ShellBuiltin,
        });
    }
    let reg = registry();
    let g = lock(&reg);
    for e in g.list_creation_entries() {
        out.push(AvailableEntry {
            plugin_id: e.plugin_id,
            plugin_label: e.plugin_label,
            entry: e.entry,
            source: EntrySource::Plugin(e.plugin_role),
        });
    }
    out
}

/// Resolve a persisted [`FabShortcut`] to the live entry metadata.
/// `None` if the referenced plugin / entry no longer exists (plugin
/// uninstalled, entry removed, etc.).
#[must_use]
pub fn resolve_shortcut(sc: &FabShortcut) -> Option<AvailableEntry> {
    available_entries()
        .into_iter()
        .find(|e| e.plugin_id == sc.plugin_id && e.entry.id == sc.entry_id)
}

/// Group `entries` by [`CreationCategory`] in menu order
/// (Canvas / Source / Other).
#[must_use]
pub fn group_by_category(entries: Vec<AvailableEntry>) -> Vec<(CreationCategory, Vec<AvailableEntry>)> {
    let mut canvases = Vec::new();
    let mut sources = Vec::new();
    let mut other = Vec::new();
    for e in entries {
        match e.entry.category {
            CreationCategory::Canvas => canvases.push(e),
            CreationCategory::Source => sources.push(e),
            CreationCategory::Other => other.push(e),
        }
    }
    let mut out = Vec::new();
    if !canvases.is_empty() {
        out.push((CreationCategory::Canvas, canvases));
    }
    if !sources.is_empty() {
        out.push((CreationCategory::Source, sources));
    }
    if !other.is_empty() {
        out.push((CreationCategory::Other, other));
    }
    out
}
