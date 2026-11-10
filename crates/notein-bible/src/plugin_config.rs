//! Plugin settings: the default Bible, the cross-reference set and the
//! original languages, shared behind an `Arc<RwLock<_>>` with
//! every reader and persisted in the translations database.
//!
//! Translations are installed through [`crate::installer`]; the cache dir comes
//! from [`istmo_core::path::data_dir`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};

use notein_i18n::tr;
use notein_sdk::plugin::PluginSettings;
use notein_sdk::{ConfigEnumOption, ConfigError, ConfigField, ConfigSchema, ConfigValue};

use crate::db::Db;

pub const KEY_DEFAULT_BIBLE: &str = "default_bible_id";
/// Toggle for the fetch.bible / "advanced" cross-reference overlay —
/// the inline bookmark icon + hover popup with every xref the TSK
/// doesn't surface.
///
/// Default `true` so first-time users see the enriched set; power users who
/// want a cleaner page turn it off.
pub const KEY_ENABLE_CROSSREF: &str = "enable_crossref";
/// Which of the three fetch.bible cross-reference sets to download; see
/// [`CrossRefLevel`].
pub const KEY_CROSSREF_LEVEL: &str = "crossref_level";
/// Whether the reader shows the Hebrew and Greek behind each verse
/// (see [`crate::originals`]). Off by default: turning it on downloads
/// the packs.
pub const KEY_ORIGINALS: &str = "originals";

/// How deep the cross-reference set goes.
///
/// fetch.bible ranks the OpenBible.info / Treasury of Scripture Knowledge
/// references by votes and splits them in three: `small` keeps the top
/// third, `medium` the top two thirds, `large` every reference. `small`
/// is the default: the page stays readable, the rest is one setting away.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CrossRefLevel {
    #[default]
    Small,
    Medium,
    Large,
}

impl CrossRefLevel {
    pub const ALL: [Self; 3] = [Self::Small, Self::Medium, Self::Large];

    /// The value stored in the settings and the path segment on
    /// fetch.bible.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }

    /// The level stored as `value`, if it is one.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|level| level.as_str() == value)
    }

    /// The name the settings and the progress toasts show.
    pub(crate) fn label(self) -> String {
        match self {
            Self::Small => tr!("config-cross-refs-level-small"),
            Self::Medium => tr!("config-cross-refs-level-medium"),
            Self::Large => tr!("config-cross-refs-level-large"),
        }
    }
}

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

/// The settings saved in `db` on an earlier run, as a shared handle.
/// Values the schema no longer accepts are left out.
#[must_use]
#[cfg_attr(feature = "sismo", sismo::function("bible::config_load"))]
pub fn load_shared(db: &Db) -> SharedConfig {
    let config = new_shared();
    match db.load_settings() {
        Ok(values) => {
            for (key, value) in values {
                if let Err(e) = apply_set(&config, &key, value) {
                    log::warn!("bible: saved setting {key} ignored: {e}");
                }
            }
        }
        Err(e) => log::warn!("bible: settings not loaded: {e}"),
    }
    config
}

/// Default platform cache directory, derived from the shared notinplus
/// data root.
///
/// Not user-configurable — the UI used to expose a `cache_dir` field but all
/// callers now resolve the path directly through this helper so a single
/// on-disk tree carries everything the plugin touches.
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
#[must_use]
pub fn build_schema(db: &Db) -> ConfigSchema {
    let options: Vec<ConfigEnumOption> = db
        .list_bibles()
        .unwrap_or_default()
        .into_iter()
        .map(|b| {
            let label = if b.name_local.is_empty() {
                b.id.clone()
            } else {
                format!("{} ({})", b.name_local, b.abbrev)
            };
            ConfigEnumOption::new(b.id, label)
        })
        .collect();
    let default_bible = options.first().map(|o| o.value.clone()).unwrap_or_default();
    let bible = if options.is_empty() {
        // Nothing installed yet: a text row, the catalog is the next step.
        ConfigField::string(KEY_DEFAULT_BIBLE, &tr!("config-default-bible"), 64, "")
    } else {
        ConfigField::choice(
            KEY_DEFAULT_BIBLE,
            &tr!("config-default-bible"),
            options,
            &default_bible,
        )
    };
    let levels = CrossRefLevel::ALL
        .into_iter()
        .map(|level| ConfigEnumOption::new(level.as_str(), level.label()));

    ConfigSchema::new(1)
        .field(bible.help(&tr!("config-default-bible-help")))
        .field(
            ConfigField::boolean(KEY_ENABLE_CROSSREF, &tr!("config-cross-refs"), true)
                .help(&tr!("config-cross-refs-help")),
        )
        .field(
            ConfigField::choice(
                KEY_CROSSREF_LEVEL,
                &tr!("config-cross-refs-level"),
                levels,
                CrossRefLevel::default().as_str(),
            )
            .help(&tr!("config-cross-refs-level-help")),
        )
        .field(
            ConfigField::boolean(KEY_ORIGINALS, &tr!("config-originals"), false)
                .help(&tr!("config-originals-help")),
        )
}

/// Store `value` under `key` once it fits that key.
///
/// # Errors
/// The key is unknown, the value has the wrong type, or the
/// cross-reference level is not a known one.
pub fn apply_set(config: &SharedConfig, key: &str, value: ConfigValue) -> Result<(), ConfigError> {
    match (key, &value) {
        (KEY_CROSSREF_LEVEL, ConfigValue::String(v)) if CrossRefLevel::parse(v).is_none() => {
            return Err(ConfigError::OutOfRange);
        }
        (KEY_ENABLE_CROSSREF | KEY_ORIGINALS, ConfigValue::Bool(_))
        | (KEY_DEFAULT_BIBLE | KEY_CROSSREF_LEVEL, ConfigValue::String(_)) => {}
        (KEY_ENABLE_CROSSREF | KEY_DEFAULT_BIBLE | KEY_CROSSREF_LEVEL | KEY_ORIGINALS, _) => {
            return Err(ConfigError::TypeMismatch);
        }
        _ => return Err(ConfigError::UnknownKey(key.into())),
    }
    config
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .values
        .insert(key.into(), value);
    Ok(())
}

pub fn get_value(config: &SharedConfig, key: &str) -> Option<ConfigValue> {
    config
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .values
        .get(key)
        .cloned()
}

pub fn string(config: &SharedConfig, key: &str) -> Option<String> {
    match get_value(config, key)? {
        ConfigValue::String(s) => Some(s),
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

/// Whether the reader offers the original languages; off until the
/// user turns it on.
#[must_use]
pub fn effective_originals(config: &SharedConfig) -> bool {
    bool(config, KEY_ORIGINALS).unwrap_or(false)
}

/// The cross-reference set downloads fetch, [`CrossRefLevel::Small`]
/// unless the user chose a bigger one.
pub fn effective_crossref_level(config: &SharedConfig) -> CrossRefLevel {
    string(config, KEY_CROSSREF_LEVEL)
        .as_deref()
        .and_then(CrossRefLevel::parse)
        .unwrap_or_default()
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

/// The settings form the shell draws for the plugin.
///
/// Every change lands in the shared config and is saved in the database;
/// a new cross-reference level also re-downloads the references of every
/// installed translation.
#[derive(Clone, Debug)]
pub struct Settings {
    db: Db,
    config: SharedConfig,
}

impl Settings {
    #[must_use]
    pub const fn new(db: Db, config: SharedConfig) -> Self {
        Self { db, config }
    }
}

impl PluginSettings for Settings {
    #[cfg_attr(feature = "sismo", sismo::function("bible::settings_schema"))]
    fn schema(&self) -> ConfigSchema {
        build_schema(&self.db)
    }

    fn get(&self, key: &str) -> Option<ConfigValue> {
        get_value(&self.config, key)
    }

    #[cfg_attr(feature = "sismo", sismo::function("bible::settings_set"))]
    fn set(&self, key: &str, value: ConfigValue) -> Result<(), ConfigError> {
        let level_before = effective_crossref_level(&self.config);
        apply_set(&self.config, key, value.clone())?;
        if let Err(e) = self.db.save_setting(key, &value) {
            log::warn!("bible: setting {key} not saved: {e}");
        }
        #[cfg(feature = "importer")]
        if key == KEY_CROSSREF_LEVEL && effective_crossref_level(&self.config) != level_before {
            crate::install_background::spawn_refresh_cross_refs(self.db.clone(), &self.config);
        }
        #[cfg(feature = "importer")]
        if key == KEY_ORIGINALS && effective_originals(&self.config) {
            if let Some(originals) = crate::originals::service() {
                originals.spawn_download();
            }
        }
        #[cfg(not(feature = "importer"))]
        let _ = level_before;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_ref_level_round_trips_through_its_name() {
        for level in CrossRefLevel::ALL {
            assert_eq!(CrossRefLevel::parse(level.as_str()), Some(level));
        }
        assert_eq!(CrossRefLevel::parse("huge"), None);
    }

    #[test]
    fn unknown_level_is_refused_and_the_default_is_small() {
        let config = new_shared();
        assert_eq!(effective_crossref_level(&config), CrossRefLevel::Small);
        assert_eq!(
            apply_set(
                &config,
                KEY_CROSSREF_LEVEL,
                ConfigValue::String("huge".into())
            ),
            Err(ConfigError::OutOfRange)
        );
        apply_set(
            &config,
            KEY_CROSSREF_LEVEL,
            ConfigValue::String("large".into()),
        )
        .unwrap();
        assert_eq!(effective_crossref_level(&config), CrossRefLevel::Large);
    }

    #[test]
    fn settings_survive_a_reload_from_the_database() {
        let db = Db::in_memory().unwrap();
        let settings = Settings::new(db.clone(), new_shared());
        settings
            .set(KEY_ENABLE_CROSSREF, ConfigValue::Bool(false))
            .unwrap();
        settings
            .set(KEY_CROSSREF_LEVEL, ConfigValue::String("medium".into()))
            .unwrap();
        let reloaded = load_shared(&db);
        assert!(!effective_enable_crossref(&reloaded));
        assert_eq!(effective_crossref_level(&reloaded), CrossRefLevel::Medium);
    }
}
