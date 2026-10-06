//! Shell-side persistence of plugin enablement + per-plugin config
//! values.
//!
//! Each registered plugin owns its live settings via the trait methods
//! `get_config` / `set_config`. The shell saves the aggregate — one
//! file for the whole process — so a relaunch finds every plugin in
//! the state the user left it.
//!
//! Layout:
//!
//! ```text
//! <data_dir>/notinplus/plugin_settings.bin
//! ```
//!
//! On load the shell walks every registered plugin's schema, finds the
//! persisted value for each `(plugin_id, key)` pair, migrates values
//! stored under a `ConfigField::aliases` name into the current key,
//! and feeds the result to `plugin.set_config`. The plugin itself owns
//! the authoritative value from that point on; `collect(&registry)`
//! builds the snapshot the next save round-trips.

use std::fs;
use std::io;
use std::path::PathBuf;

use bincode::config::{self, Configuration};
use notein_plugin::{ConfigField, ConfigValue, PluginRegistry, PluginRole, RegistrySchemaEntry};

use crate::library::bodies;

const CODEC: Configuration = config::standard();
const FILE: &str = "plugin_settings.bin";

/// On-disk shape. `Vec<_>` instead of `HashMap<_, _>` so bincode's
/// derive requirements stay narrow (matches [`crate::prefs::Prefs`]).
#[istmo::message]
#[derive(Clone, Debug, Default)]
pub struct PluginSettings {
    /// Plugins the user has explicitly disabled. Absence = enabled.
    pub disabled: Vec<String>,
    /// One entry per `(plugin_id, key) → value` the user touched. Keys
    /// absent from this list fall back to the schema default.
    pub values: Vec<PluginSettingValue>,
    /// Last schema version observed when the plugin's settings were
    /// written. The loader uses it to drive alias migrations.
    pub schema_versions: Vec<PluginSchemaVersion>,
    /// Home FAB shortcut buttons (the two bubbles above `+`). Empty =
    /// never touched → shell applies defaults. Length 2 = honor
    /// verbatim; `FabShortcutSlot::Empty` is an explicit "no shortcut
    /// here", distinct from "user never configured".
    pub fab_shortcuts: Vec<FabShortcutSlot>,
}

/// A home-FAB shortcut: references one creation entry by its
/// `(plugin_id, entry_id)` pair. `plugin_id` can be a real plugin or
/// the pseudo [`crate::shell_builtins::BUILTIN_PLUGIN_ID`].
#[istmo::message]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FabShortcut {
    pub plugin_id: String,
    pub entry_id: String,
}

/// Slot shape for persisted shortcut picks.
#[istmo::message]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FabShortcutSlot {
    /// User explicitly cleared this slot — render an empty circle.
    Empty,
    /// User picked a specific entry.
    Entry(FabShortcut),
}

#[istmo::message]
#[derive(Clone, Debug)]
pub struct PluginSettingValue {
    pub plugin_id: String,
    pub key: String,
    pub value: ConfigValue,
}

#[istmo::message]
#[derive(Clone, Debug)]
pub struct PluginSchemaVersion {
    pub plugin_id: String,
    pub version: u32,
}

#[must_use]
fn path() -> PathBuf {
    bodies::root_dir().join(FILE)
}

/// Read persisted settings. Missing / corrupt file → defaults so a
/// first-run never surfaces as an error.
#[must_use]
pub fn load() -> PluginSettings {
    let Ok(bytes) = fs::read(path()) else {
        return PluginSettings::default();
    };
    bincode::decode_from_slice::<PluginSettings, _>(&bytes, CODEC)
        .map(|(s, _)| s)
        .unwrap_or_default()
}

/// Atomically overwrite the settings file.
///
/// # Errors
/// Encode / fs failures.
pub fn save(settings: &PluginSettings) -> io::Result<()> {
    let bytes = bincode::encode_to_vec(settings, CODEC)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_atomic(&path(), &bytes)
}

/// Hydrate `registry` from `settings`:
///
/// * flip the enabled bit for every id in `settings.disabled`,
/// * for every registered plugin, walk its current schema and feed the
///   persisted value (resolved through aliases) into
///   `plugin.set_config` so the plugin's in-memory state matches disk.
///
/// Per-field errors are logged and skipped — one bad field never
/// prevents the rest from loading.
pub fn apply(settings: &PluginSettings, registry: &mut PluginRegistry) {
    for id in &settings.disabled {
        registry.set_enabled(id, false);
    }
    let schemas = registry.all_schemas();
    for schema_entry in &schemas {
        apply_schema_entry(registry, schema_entry, &settings.values);
    }
}

fn apply_schema_entry(
    registry: &PluginRegistry,
    schema_entry: &RegistrySchemaEntry,
    values: &[PluginSettingValue],
) {
    let plugin_id = schema_entry.plugin_id.as_str();
    for field in &schema_entry.schema.fields {
        let Some(value) = find_persisted(values, plugin_id, field) else {
            continue;
        };
        let outcome = match schema_entry.plugin_role {
            PluginRole::Source => registry
                .get_source(plugin_id)
                .map(|p| p.set_config(&field.key, value.clone())),
            PluginRole::Canvas => registry
                .get_canvas(plugin_id)
                .map(|p| p.set_config(&field.key, value.clone())),
        };
        if let Some(Err(err)) = outcome {
            log::warn!(
                "plugin {plugin_id} rejected persisted value for `{}`: {err}",
                field.key
            );
        }
    }
}

/// Build the snapshot that mirrors the current registry state.
#[must_use]
pub fn collect(registry: &PluginRegistry) -> PluginSettings {
    let mut out = PluginSettings {
        disabled: registry.disabled_ids().map(ToOwned::to_owned).collect(),
        values: Vec::new(),
        schema_versions: Vec::new(),
        fab_shortcuts: Vec::new(),
    };
    for schema_entry in registry.all_schemas() {
        out.schema_versions.push(PluginSchemaVersion {
            plugin_id: schema_entry.plugin_id.clone(),
            version: schema_entry.schema.version,
        });
        collect_schema_entry(registry, &schema_entry, &mut out.values);
    }
    out
}

fn collect_schema_entry(
    registry: &PluginRegistry,
    schema_entry: &RegistrySchemaEntry,
    values: &mut Vec<PluginSettingValue>,
) {
    let plugin_id = schema_entry.plugin_id.as_str();
    for field in &schema_entry.schema.fields {
        let current = match schema_entry.plugin_role {
            PluginRole::Source => registry
                .get_source(plugin_id)
                .and_then(|p| p.get_config(&field.key)),
            PluginRole::Canvas => registry
                .get_canvas(plugin_id)
                .and_then(|p| p.get_config(&field.key)),
        };
        let Some(value) = current else {
            continue;
        };
        if value == field.default {
            continue;
        }
        values.push(PluginSettingValue {
            plugin_id: plugin_id.to_owned(),
            key: field.key.clone(),
            value,
        });
    }
}

fn find_persisted<'a>(
    values: &'a [PluginSettingValue],
    plugin_id: &str,
    field: &ConfigField,
) -> Option<&'a ConfigValue> {
    if let Some(v) = values
        .iter()
        .find(|v| v.plugin_id == plugin_id && v.key == field.key)
    {
        return Some(&v.value);
    }
    for alias in &field.aliases {
        if let Some(v) = values
            .iter()
            .find(|v| v.plugin_id == plugin_id && &v.key == alias)
        {
            log::info!(
                "migrating persisted plugin setting {plugin_id}/{alias} → {}",
                field.key
            );
            return Some(&v.value);
        }
    }
    None
}

fn write_atomic(dest: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = dest.to_path_buf();
    let file_name = tmp
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    let mut with_ext = file_name;
    with_ext.push(".tmp");
    tmp.set_file_name(with_ext);
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, dest)
}
