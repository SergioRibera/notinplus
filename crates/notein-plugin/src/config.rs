//! Plugin-declared settings schema.
//!
//! Each plugin exposes a [`ConfigSchema`] via its trait. The shell
//! renders a form from it — one collapsible section per plugin inside
//! the global settings panel — and persists the resulting
//! `(plugin_id, key) → ConfigValue` map alongside the plugin's observed
//! [`ConfigSchema::version`]. On load, if the persisted version is
//! lower than the plugin's current one, the shell migrates fields via
//! their [`ConfigField::aliases`] before falling back to defaults.
//!
//! Keep field keys short and stable; use `aliases` on rename rather
//! than bumping the schema version.

use std::ops::RangeInclusive;

use bincode::{Decode, Encode};
use notein_i18n::tr;
use rkyv::{Archive, Deserialize, Serialize};

use crate::failure::UserMessage;

/// A plugin's full settings schema.
///
/// `version` is monotonic: bump on add/remove/type-change. Pure renames
/// do not require a bump — use [`ConfigField::aliases`].
#[derive(Clone, Debug, PartialEq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct ConfigSchema {
    pub version: u32,
    pub fields: Vec<ConfigField>,
}

impl ConfigSchema {
    /// An empty schema at `version`; chain [`Self::field`] to fill it.
    #[must_use]
    pub const fn new(version: u32) -> Self {
        Self {
            version,
            fields: Vec::new(),
        }
    }

    #[must_use]
    pub const fn empty() -> Self {
        Self::new(0)
    }

    /// The schema with `field` appended.
    #[must_use]
    pub fn field(mut self, field: ConfigField) -> Self {
        self.fields.push(field);
        self
    }
}

impl ConfigField {
    fn of(key: &str, label: &str, kind: ConfigFieldKind, default: ConfigValue) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            help: String::new(),
            kind,
            default,
            aliases: Vec::new(),
        }
    }

    /// An on/off switch.
    #[must_use]
    pub fn boolean(key: &str, label: &str, default: bool) -> Self {
        Self::of(
            key,
            label,
            ConfigFieldKind::Bool,
            ConfigValue::Bool(default),
        )
    }

    /// A whole number inside `range`.
    #[must_use]
    pub fn int(key: &str, label: &str, range: RangeInclusive<i64>, default: i64) -> Self {
        let (min, max) = range.into_inner();
        Self::of(
            key,
            label,
            ConfigFieldKind::Int { min, max },
            ConfigValue::Int(default),
        )
    }

    /// A number inside `range`, moved in `step` increments.
    #[must_use]
    pub fn float(
        key: &str,
        label: &str,
        range: RangeInclusive<f64>,
        step: f64,
        default: f64,
    ) -> Self {
        let (min, max) = range.into_inner();
        Self::of(
            key,
            label,
            ConfigFieldKind::Float { min, max, step },
            ConfigValue::Float(default),
        )
    }

    /// Free text of at most `max_len` characters.
    #[must_use]
    pub fn string(key: &str, label: &str, max_len: u32, default: &str) -> Self {
        Self::of(
            key,
            label,
            ConfigFieldKind::String { max_len },
            ConfigValue::String(default.into()),
        )
    }

    /// One of `options`, stored as its value.
    #[must_use]
    pub fn choice(
        key: &str,
        label: &str,
        options: impl IntoIterator<Item = ConfigEnumOption>,
        default: &str,
    ) -> Self {
        Self::of(
            key,
            label,
            ConfigFieldKind::Enum {
                options: options.into_iter().collect(),
            },
            ConfigValue::String(default.into()),
        )
    }

    /// A color, `0xRRGGBBAA`.
    #[must_use]
    pub fn color(key: &str, label: &str, default: u32) -> Self {
        Self::of(
            key,
            label,
            ConfigFieldKind::Color,
            ConfigValue::Color(default),
        )
    }

    /// A file with one of `extensions` (lowercase, no dot).
    #[must_use]
    pub fn file_path(key: &str, label: &str, extensions: &[&str], default: &str) -> Self {
        Self::of(
            key,
            label,
            ConfigFieldKind::FilePath {
                extensions: extensions.iter().map(|s| (*s).into()).collect(),
            },
            ConfigValue::FilePath(default.into()),
        )
    }

    /// A button that sends [`ConfigValue::Trigger`] when pressed, after
    /// `confirm` is accepted when given.
    #[must_use]
    pub fn action(key: &str, label: &str, button_label: &str, confirm: Option<&str>) -> Self {
        Self::of(
            key,
            label,
            ConfigFieldKind::Action {
                button_label: button_label.into(),
                confirm: confirm.map(Into::into),
            },
            ConfigValue::Trigger,
        )
    }

    /// The field with helper text under its label.
    #[must_use]
    pub fn help(mut self, help: &str) -> Self {
        self.help = help.into();
        self
    }

    /// The field with earlier names; persisted values under any of them
    /// migrate to the current key.
    #[must_use]
    pub fn aliases(mut self, aliases: &[&str]) -> Self {
        self.aliases = aliases.iter().map(|s| (*s).into()).collect();
        self
    }
}

impl ConfigEnumOption {
    #[must_use]
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

/// One typed setting inside a schema.
#[derive(Clone, Debug, PartialEq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct ConfigField {
    /// Stable, machine-readable key (e.g. `"render_dpi"`).
    pub key: String,
    /// Human label for the form row.
    pub label: String,
    /// Optional tooltip / helper text.
    pub help: String,
    pub kind: ConfigFieldKind,
    pub default: ConfigValue,
    /// Previous names of this field. Loader migrates persisted values
    /// found under any alias into the current `key`.
    pub aliases: Vec<String>,
}

/// The type and constraints of a [`ConfigField`].
#[derive(Clone, Debug, PartialEq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub enum ConfigFieldKind {
    Bool,
    Int {
        min: i64,
        max: i64,
    },
    Float {
        min: f64,
        max: f64,
        step: f64,
    },
    String {
        max_len: u32,
    },
    Enum {
        options: Vec<ConfigEnumOption>,
    },
    /// 8-bit per channel RGBA packed as `0xRRGGBBAA`.
    Color,
    FilePath {
        /// Lowercase file extensions without the leading dot.
        extensions: Vec<String>,
    },
    /// Button-shaped action. The host renders a button labelled
    /// `button_label` in the settings form; clicking it dispatches
    /// `set_config(key, ConfigValue::Trigger)` to the plugin. No value
    /// is stored. Use for "Install", "Reset", "Refresh" style actions.
    Action {
        button_label: String,
        /// Optional confirmation prompt. When set, the host pops a
        /// confirm dialog before firing the trigger.
        confirm: Option<String>,
    },
}

/// One option inside [`ConfigFieldKind::Enum`].
#[derive(Clone, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct ConfigEnumOption {
    /// Stored value (matches against [`ConfigValue::String`]).
    pub value: String,
    /// Display label.
    pub label: String,
}

/// A concrete setting value — union of every variant declared in
/// [`ConfigFieldKind`].
#[derive(Clone, Debug, PartialEq, Encode, Decode, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub enum ConfigValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    /// RGBA packed as `0xRRGGBBAA`.
    Color(u32),
    FilePath(String),
    /// Button-style trigger. Carries no payload. Dispatched to the
    /// plugin on button press when the field kind is
    /// [`ConfigFieldKind::Action`].
    Trigger,
}

/// Error setting a config value.
#[derive(Clone, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub enum ConfigError {
    UnknownKey(String),
    TypeMismatch,
    OutOfRange,
    Rejected(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownKey(k) => write!(f, "unknown setting {k}"),
            Self::TypeMismatch => f.write_str("value of the wrong type"),
            Self::OutOfRange => f.write_str("value out of range"),
            Self::Rejected(msg) => write!(f, "value rejected: {msg}"),
        }
    }
}

impl UserMessage for ConfigError {
    fn user_message(&self) -> String {
        match self {
            Self::UnknownKey(k) => tr!("plugin-config-unknown-key", "key" => k.as_str()),
            Self::TypeMismatch => tr!("plugin-config-type-mismatch"),
            Self::OutOfRange => tr!("plugin-config-out-of-range"),
            Self::Rejected(msg) => tr!("plugin-config-rejected", "reason" => msg.as_str()),
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::{ConfigEnumOption, ConfigField, ConfigFieldKind, ConfigSchema, ConfigValue};

    #[test]
    fn fields_chain_into_a_schema() {
        let schema = ConfigSchema::new(2)
            .field(ConfigField::float("size", "Tamaño", 10.0..=20.0, 0.5, 12.0).help("px"))
            .field(
                ConfigField::choice("mode", "Modo", [ConfigEnumOption::new("a", "A")], "a")
                    .aliases(&["old_mode"]),
            );
        assert_eq!(schema.version, 2);
        let [size, mode] = schema.fields.as_slice() else {
            panic!("two fields");
        };
        assert_eq!(size.help, "px");
        assert_eq!(
            size.kind,
            ConfigFieldKind::Float {
                min: 10.0,
                max: 20.0,
                step: 0.5
            }
        );
        assert_eq!(mode.default, ConfigValue::String("a".into()));
        assert_eq!(mode.aliases, ["old_mode"]);
    }
}
