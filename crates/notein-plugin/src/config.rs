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

use bincode::{Decode, Encode};
use rkyv::{Archive, Deserialize, Serialize};

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
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            version: 0,
            fields: Vec::new(),
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
}
