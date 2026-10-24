//! Plugin settings: the default Bible and the catalogue URL, shared
//! behind an `Arc<RwLock<_>>` with every reader. Translations are
//! installed through [`crate::installer`]; the cache dir comes from
//! [`istmo_core::path::data_dir`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use notein_sdk::{
    ConfigEnumOption, ConfigError, ConfigField, ConfigFieldKind, ConfigSchema, ConfigValue,
};

use crate::db::Db;

pub const KEY_DEFAULT_BIBLE: &str = "default_bible_id";
pub const KEY_MANIFEST_URL: &str = "biblionlabs_manifest_url";
pub const KEY_FONT_SIZE: &str = "font_size";
pub const KEY_LINE_HEIGHT: &str = "line_height";
/// Toggle for the fetch.bible / "advanced" cross-reference overlay —
/// the inline bookmark icon + hover popup with every xref the TSK
/// doesn't surface. Default `true` so first-time users see the
/// enriched set; power users who want a cleaner page turn it off.
pub const KEY_ENABLE_CROSSREF: &str = "enable_crossref";

pub const DEFAULT_FONT_SIZE: f64 = 22.0;
pub const DEFAULT_LINE_HEIGHT: f64 = 1.65;
pub const DEFAULT_MANIFEST_URL: &str = "https://v1.fetch.bible/manifest.json";

/// Shared handle. Cheap to clone, interior-mutable.
pub type SharedConfig = Arc<RwLock<Inner>>;

#[derive(Debug, Default)]
pub struct Inner {
    pub values: HashMap<String, ConfigValue>,
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

/// Default platform cache directory, derived from the shared notinplus
/// data root. Not user-configurable — the UI used to expose a cache_dir
/// field but all callers now resolve the path directly through this
/// helper so a single on-disk tree carries everything the plugin touches.
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

    ConfigSchema {
        version: 1,
        fields: vec![
            ConfigField {
                key: KEY_DEFAULT_BIBLE.into(),
                label: "Biblia por defecto".into(),
                help: "Traducción abierta al crear una pestaña nueva.".into(),
                kind: if options.is_empty() {
                    // No installed Bibles yet — surface as a string
                    // field so the user can see something while the
                    // install modal is their actual next step.
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
            ConfigField {
                key: KEY_ENABLE_CROSSREF.into(),
                label: "Referencias cruzadas avanzadas".into(),
                help: "Muestra el ícono + popup con todas las referencias \
                       de fetch.bible. Las notas TSK (letras a/b/c) se ven \
                       siempre sin importar esta opción."
                    .into(),
                kind: ConfigFieldKind::Bool,
                default: ConfigValue::Bool(true),
                aliases: Vec::new(),
            },
        ],
    }
}

pub fn apply_set(
    config: &SharedConfig,
    key: &str,
    value: ConfigValue,
) -> Result<(), ConfigError> {
    match (key, &value) {
        (KEY_FONT_SIZE, ConfigValue::Float(v)) if (14.0..=32.0).contains(v) => {}
        (KEY_LINE_HEIGHT, ConfigValue::Float(v)) if (1.2..=2.2).contains(v) => {}
        (KEY_FONT_SIZE | KEY_LINE_HEIGHT, _) => return Err(ConfigError::TypeMismatch),
        (KEY_ENABLE_CROSSREF, ConfigValue::Bool(_)) => {}
        (KEY_ENABLE_CROSSREF, _) => return Err(ConfigError::TypeMismatch),
        (KEY_DEFAULT_BIBLE | KEY_MANIFEST_URL, ConfigValue::String(_)) => {}
        (KEY_DEFAULT_BIBLE | KEY_MANIFEST_URL, _) => {
            return Err(ConfigError::TypeMismatch);
        }
        _ => return Err(ConfigError::UnknownKey(key.into())),
    }
    config.write().unwrap().values.insert(key.into(), value);
    Ok(())
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

pub fn bool(config: &SharedConfig, key: &str) -> Option<bool> {
    match get_value(config, key)? {
        ConfigValue::Bool(v) => Some(v),
        _ => None,
    }
}

/// Whether the advanced (fetch.bible) cross-ref overlay should render.
/// Defaults to `true` so first-run installs see the enriched set.
pub fn effective_enable_crossref(config: &SharedConfig) -> bool {
    bool(config, KEY_ENABLE_CROSSREF).unwrap_or(true)
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


/// Resolve the effective manifest URL, falling back to the default
/// fetch.bible endpoint.
pub fn effective_manifest_url(config: &SharedConfig) -> String {
    string(config, KEY_MANIFEST_URL)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_MANIFEST_URL.into())
}
