//! Fluent builders for [`ConfigSchema`] and [`CreationEntry`].
//!
//! Nothing here is doing anything a struct literal couldn't do — the
//! point is to collapse the common cases to one line each so a
//! plugin's `config_schema()` / `creation_entries()` reads like a
//! declarative manifest instead of boilerplate.

use notein_plugin::bytes::Bytes;
use notein_plugin::{
    ConfigEnumOption, ConfigField, ConfigFieldKind, ConfigSchema, ConfigValue, CreationCategory,
    CreationEntry, CreationKind,
};

/// Fluent builder for a plugin's [`ConfigSchema`].
///
/// Chain `.bool / .int / .float / …` calls then `.build()`.
#[derive(Debug, Clone)]
pub struct ConfigSchemaBuilder {
    version: u32,
    fields: Vec<ConfigField>,
}

impl ConfigSchemaBuilder {
    #[must_use]
    pub const fn new(version: u32) -> Self {
        Self {
            version,
            fields: Vec::new(),
        }
    }

    #[must_use]
    pub fn bool_(mut self, key: &str, label: &str, help: &str, default: bool) -> Self {
        self.fields.push(ConfigField {
            key: key.into(),
            label: label.into(),
            help: help.into(),
            kind: ConfigFieldKind::Bool,
            default: ConfigValue::Bool(default),
            aliases: Vec::new(),
        });
        self
    }

    #[must_use]
    pub fn int(
        mut self,
        key: &str,
        label: &str,
        help: &str,
        min: i64,
        max: i64,
        default: i64,
    ) -> Self {
        self.fields.push(ConfigField {
            key: key.into(),
            label: label.into(),
            help: help.into(),
            kind: ConfigFieldKind::Int { min, max },
            default: ConfigValue::Int(default),
            aliases: Vec::new(),
        });
        self
    }

    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn float(
        mut self,
        key: &str,
        label: &str,
        help: &str,
        min: f64,
        max: f64,
        step: f64,
        default: f64,
    ) -> Self {
        self.fields.push(ConfigField {
            key: key.into(),
            label: label.into(),
            help: help.into(),
            kind: ConfigFieldKind::Float { min, max, step },
            default: ConfigValue::Float(default),
            aliases: Vec::new(),
        });
        self
    }

    #[must_use]
    pub fn string(
        mut self,
        key: &str,
        label: &str,
        help: &str,
        max_len: u32,
        default: &str,
    ) -> Self {
        self.fields.push(ConfigField {
            key: key.into(),
            label: label.into(),
            help: help.into(),
            kind: ConfigFieldKind::String { max_len },
            default: ConfigValue::String(default.into()),
            aliases: Vec::new(),
        });
        self
    }

    #[must_use]
    pub fn enum_(
        mut self,
        key: &str,
        label: &str,
        help: &str,
        options: &[(&str, &str)],
        default: &str,
    ) -> Self {
        self.fields.push(ConfigField {
            key: key.into(),
            label: label.into(),
            help: help.into(),
            kind: ConfigFieldKind::Enum {
                options: options
                    .iter()
                    .map(|(value, label)| ConfigEnumOption {
                        value: (*value).into(),
                        label: (*label).into(),
                    })
                    .collect(),
            },
            default: ConfigValue::String(default.into()),
            aliases: Vec::new(),
        });
        self
    }

    /// `rgba` packed as `0xRRGGBBAA`.
    #[must_use]
    pub fn color(mut self, key: &str, label: &str, help: &str, default: u32) -> Self {
        self.fields.push(ConfigField {
            key: key.into(),
            label: label.into(),
            help: help.into(),
            kind: ConfigFieldKind::Color,
            default: ConfigValue::Color(default),
            aliases: Vec::new(),
        });
        self
    }

    #[must_use]
    pub fn file_path(
        mut self,
        key: &str,
        label: &str,
        help: &str,
        extensions: &[&str],
        default: &str,
    ) -> Self {
        self.fields.push(ConfigField {
            key: key.into(),
            label: label.into(),
            help: help.into(),
            kind: ConfigFieldKind::FilePath {
                extensions: extensions.iter().map(|s| (*s).into()).collect(),
            },
            default: ConfigValue::FilePath(default.into()),
            aliases: Vec::new(),
        });
        self
    }

    /// Register alternate names for the last pushed field. Loader
    /// uses these to migrate persisted settings after a rename.
    #[must_use]
    pub fn aliases(mut self, aliases: &[&str]) -> Self {
        if let Some(last) = self.fields.last_mut() {
            last.aliases = aliases.iter().map(|s| (*s).into()).collect();
        }
        self
    }

    #[must_use]
    pub fn build(self) -> ConfigSchema {
        ConfigSchema {
            version: self.version,
            fields: self.fields,
        }
    }
}

/// Shorthand for a `Blank` [`CreationEntry`].
#[must_use]
pub fn blank_entry(
    id: &str,
    label: &str,
    icon: Bytes,
    category: CreationCategory,
    prompt_name: bool,
) -> CreationEntry {
    CreationEntry {
        id: id.into(),
        label: label.into(),
        icon,
        category,
        kind: CreationKind::Blank { prompt_name },
    }
}

/// Shorthand for an `Import` [`CreationEntry`].
#[must_use]
pub fn import_entry(
    id: &str,
    label: &str,
    icon: Bytes,
    category: CreationCategory,
    dialog_title: &str,
    extensions: &[&str],
    mime_types: &[&str],
) -> CreationEntry {
    CreationEntry {
        id: id.into(),
        label: label.into(),
        icon,
        category,
        kind: CreationKind::Import {
            dialog_title: dialog_title.into(),
            extensions: extensions.iter().map(|s| (*s).into()).collect(),
            mime_types: mime_types.iter().map(|s| (*s).into()).collect(),
        },
    }
}

/// Shorthand for a `Custom` [`CreationEntry`].
#[must_use]
pub fn custom_entry(
    id: &str,
    label: &str,
    icon: Bytes,
    category: CreationCategory,
) -> CreationEntry {
    CreationEntry {
        id: id.into(),
        label: label.into(),
        icon,
        category,
        kind: CreationKind::Custom,
    }
}
