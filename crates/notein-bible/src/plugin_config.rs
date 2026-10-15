//! Shared configuration between `BibleCanvas` and `BibleSource`.
//!
//! The host persists plugin settings per `(plugin_id, key)` tuple via
//! the generic settings panel. Both trait objects need the same values
//! (which Bible is the default, where to cache downloads, which
//! catalogue URL to hit) so they share an `Arc<RwLock<_>>`.
//!
//! Schema fields:
//!
//! - `default_bible_id` — `Enum` over installed Bibles. Fresh options
//!   are read from the DB on every `config_schema()` call so a newly
//!   installed translation shows up without a plugin reload.
//! - `biblionlabs_manifest_url` — catalogue endpoint. Default points at
//!   fetch.bible but a user with a self-hosted mirror can retarget.
//! - `cache_dir` — where downloaded JSON lives. Blank = platform
//!   default (`~/.cache/notinplus/bible` on Linux).
//! - `install_bible_id` — action key. Writing a value triggers an async
//!   install of that Bible id in the background; the value is not
//!   stored. The settings panel renders it as a plain text field; UI
//!   phases will swap for a dropdown driven off `CatalogueView`.
//! - `font_size` — body text size in logical pixels.
//! - `line_height` — line height multiplier.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use notein_sdk::{
    ConfigEnumOption, ConfigError, ConfigField, ConfigFieldKind, ConfigSchema, ConfigValue, Host,
};

use crate::db::Db;

pub const KEY_DEFAULT_BIBLE: &str = "default_bible_id";
pub const KEY_MANIFEST_URL: &str = "biblionlabs_manifest_url";
pub const KEY_CACHE_DIR: &str = "cache_dir";
/// String field — user-supplied catalogue id (persisted).
pub const KEY_INSTALL_BIBLE_ID: &str = "install_bible_id";
/// Action trigger — fires install for whatever id lives at
/// [`KEY_INSTALL_BIBLE_ID`]. The value is not stored.
pub const KEY_INSTALL_BIBLE: &str = "install_bible";
pub const KEY_FONT_SIZE: &str = "font_size";
pub const KEY_LINE_HEIGHT: &str = "line_height";

pub const DEFAULT_FONT_SIZE: f64 = 22.0;
pub const DEFAULT_LINE_HEIGHT: f64 = 1.65;
pub const DEFAULT_MANIFEST_URL: &str = "https://v1.fetch.bible/manifest.json";

/// Shared handle. Cheap to clone, interior-mutable.
pub type SharedConfig = Arc<RwLock<Inner>>;

#[derive(Debug, Default)]
pub struct Inner {
    pub values: HashMap<String, ConfigValue>,
    /// Host handle received through [`notein_sdk::CanvasKind::install_host`].
    /// Only populated once the shell has registered the plugin.
    pub host: Option<Host>,
}

impl Inner {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

#[must_use]
pub fn new_shared() -> SharedConfig {
    Arc::new(RwLock::new(Inner::new()))
}

pub fn set_host(config: &SharedConfig, host: Host) {
    config.write().unwrap().host = Some(host);
}

#[must_use]
pub fn host(config: &SharedConfig) -> Option<Host> {
    config.read().unwrap().host.clone()
}

/// Resolve the default platform cache directory. Lives alongside the
/// Bible DB under the shared notinplus data root so a single on-disk
/// tree carries everything the plugin touches.
#[must_use]
pub fn platform_cache_dir() -> PathBuf {
    istmo_core::path::data_dir()
        .join("notinplus")
        .join("bible")
        .join("cache")
}

/// Build the settings schema. Dynamic — reads installed Bibles from
/// `db` so `default_bible_id` always shows the real options available
/// to the user right now.
pub fn build_schema(db: &Db) -> ConfigSchema {
    let bibles = db.list_bibles().unwrap_or_default();
    let options: Vec<ConfigEnumOption> = bibles
        .into_iter()
        .map(|b| ConfigEnumOption {
            value: b.id.clone(),
            label: if b.name_local.is_empty() {
                b.id
            } else {
                format!("{} ({})", b.name_local, b.abbrev)
            },
        })
        .collect();

    let default_bible_default = options
        .first()
        .map(|o| ConfigValue::String(o.value.clone()))
        .unwrap_or_else(|| ConfigValue::String(String::new()));

    let cache_default = platform_cache_dir()
        .to_string_lossy()
        .into_owned();

    ConfigSchema {
        version: 1,
        fields: vec![
            ConfigField {
                key: KEY_DEFAULT_BIBLE.into(),
                label: "Biblia por defecto".into(),
                help: "Traducción abierta al crear una pestaña nueva.".into(),
                kind: if options.is_empty() {
                    // No installed Bibles yet — surface as a string
                    // field so the user can type an id manually (then
                    // the install action downloads it).
                    ConfigFieldKind::String { max_len: 64 }
                } else {
                    ConfigFieldKind::Enum { options }
                },
                default: default_bible_default,
                aliases: Vec::new(),
            },
            ConfigField {
                key: KEY_MANIFEST_URL.into(),
                label: "URL del catálogo".into(),
                help: "Endpoint del manifest biblionlabs / fetch.bible.".into(),
                kind: ConfigFieldKind::String { max_len: 256 },
                default: ConfigValue::String(DEFAULT_MANIFEST_URL.into()),
                aliases: Vec::new(),
            },
            ConfigField {
                key: KEY_CACHE_DIR.into(),
                label: "Directorio de caché".into(),
                help: "Donde se guardan los JSON descargados.".into(),
                kind: ConfigFieldKind::String { max_len: 512 },
                default: ConfigValue::String(cache_default),
                aliases: Vec::new(),
            },
            ConfigField {
                key: KEY_INSTALL_BIBLE_ID.into(),
                label: "Biblia a instalar (id del catálogo)".into(),
                help: "Id del catálogo biblionlabs (ej. spa_rv1960).".into(),
                kind: ConfigFieldKind::String { max_len: 64 },
                default: ConfigValue::String("spa_rv1960".into()),
                aliases: Vec::new(),
            },
            ConfigField {
                key: KEY_INSTALL_BIBLE.into(),
                label: "Instalar".into(),
                help: "Descarga la Biblia indicada arriba en segundo plano.".into(),
                kind: ConfigFieldKind::Action {
                    button_label: "Instalar ahora".into(),
                    confirm: None,
                },
                default: ConfigValue::Trigger,
                aliases: Vec::new(),
            },
            ConfigField {
                key: KEY_FONT_SIZE.into(),
                label: "Tamaño del texto".into(),
                help: "Tamaño en píxeles del cuerpo del versículo.".into(),
                kind: ConfigFieldKind::Float { min: 14.0, max: 32.0, step: 1.0 },
                default: ConfigValue::Float(DEFAULT_FONT_SIZE),
                aliases: Vec::new(),
            },
            ConfigField {
                key: KEY_LINE_HEIGHT.into(),
                label: "Interlineado".into(),
                help: "Multiplicador de altura de línea del párrafo.".into(),
                kind: ConfigFieldKind::Float { min: 1.2, max: 2.2, step: 0.05 },
                default: ConfigValue::Float(DEFAULT_LINE_HEIGHT),
                aliases: Vec::new(),
            },
        ],
    }
}

/// Validate `value` against `key`. Writing to [`KEY_INSTALL_BIBLE`] is
/// an action — the caller sees `Ok(InstallTrigger::Install(id))` and
/// should spawn an installer, but the value is NOT retained.
#[derive(Debug)]
pub enum SetResult {
    Stored,
    InstallRequested(String),
}

pub fn apply_set(
    config: &SharedConfig,
    key: &str,
    value: ConfigValue,
) -> Result<SetResult, ConfigError> {
    match (key, &value) {
        (KEY_FONT_SIZE, ConfigValue::Float(v)) if (14.0..=32.0).contains(v) => {}
        (KEY_LINE_HEIGHT, ConfigValue::Float(v)) if (1.2..=2.2).contains(v) => {}
        (KEY_FONT_SIZE | KEY_LINE_HEIGHT, _) => return Err(ConfigError::TypeMismatch),
        (
            KEY_DEFAULT_BIBLE | KEY_MANIFEST_URL | KEY_CACHE_DIR | KEY_INSTALL_BIBLE_ID,
            ConfigValue::String(_),
        ) => {}
        (KEY_DEFAULT_BIBLE | KEY_MANIFEST_URL | KEY_CACHE_DIR | KEY_INSTALL_BIBLE_ID, _) => {
            return Err(ConfigError::TypeMismatch);
        }
        (KEY_INSTALL_BIBLE, ConfigValue::Trigger) => {
            let id = string(config, KEY_INSTALL_BIBLE_ID).unwrap_or_default();
            if id.trim().is_empty() {
                return Err(ConfigError::Rejected(
                    "no bible id configured (set install_bible_id first)".into(),
                ));
            }
            return Ok(SetResult::InstallRequested(id));
        }
        (KEY_INSTALL_BIBLE, _) => return Err(ConfigError::TypeMismatch),
        _ => return Err(ConfigError::UnknownKey(key.into())),
    }
    config.write().unwrap().values.insert(key.into(), value);
    Ok(SetResult::Stored)
}

pub fn get_value(config: &SharedConfig, key: &str) -> Option<ConfigValue> {
    config.read().unwrap().values.get(key).cloned()
}

pub fn string(config: &SharedConfig, key: &str) -> Option<String> {
    match get_value(config, key)? {
        ConfigValue::String(s) => Some(s),
        _ => None,
    }
}

pub fn float(config: &SharedConfig, key: &str) -> Option<f64> {
    match get_value(config, key)? {
        ConfigValue::Float(v) => Some(v),
        _ => None,
    }
}

/// Resolve the effective default Bible id — the stored value, or the
/// first installed Bible, or an empty string when nothing is installed.
pub fn effective_default_bible(config: &SharedConfig, db: &Db) -> String {
    if let Some(s) = string(config, KEY_DEFAULT_BIBLE) {
        if !s.is_empty() {
            return s;
        }
    }
    db.list_bibles()
        .ok()
        .and_then(|v| v.into_iter().next())
        .map(|b| b.id)
        .unwrap_or_default()
}

/// Resolve the effective cache dir, falling back to platform default.
pub fn effective_cache_dir(config: &SharedConfig) -> PathBuf {
    string(config, KEY_CACHE_DIR)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(platform_cache_dir)
}

/// Resolve the effective manifest URL, falling back to the default
/// fetch.bible endpoint.
pub fn effective_manifest_url(config: &SharedConfig) -> String {
    string(config, KEY_MANIFEST_URL)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_MANIFEST_URL.into())
}
