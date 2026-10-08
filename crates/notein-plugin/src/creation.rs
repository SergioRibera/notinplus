//! Plugin-declared "what can I create" menu entries.
//!
//! The home FAB `+` aggregates [`CreationEntry`] values from every
//! registered plugin, grouping by [`CreationCategory`]. The shell
//! drives the dialog flow based on the entry's [`CreationKind`] and
//! then calls the plugin's `execute_creation` to produce the new item.

use bytes::Bytes;
use rkyv::{Archive, Deserialize, Serialize};

/// One creation action a plugin exposes to the home menu.
#[derive(Clone, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct CreationEntry {
    /// Machine-readable id passed back to `execute_creation`.
    pub id: String,
    /// Menu label (e.g. `"Importar PDF"`).
    pub label: String,
    /// Emoji or asset reference for the icon.
    pub icon: Bytes,
    pub category: CreationCategory,
    pub kind: CreationKind,
}

/// Visual grouping inside the home menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub enum CreationCategory {
    Canvas,
    Source,
    Other,
}

/// Flow the shell drives when the user picks this entry.
#[derive(Clone, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub enum CreationKind {
    /// Shell shows a file picker; the chosen bytes are passed to
    /// `execute_creation` via `import_bytes`.
    Import {
        dialog_title: String,
        /// Allowed extensions (no leading dot).
        extensions: Vec<String>,
        /// Optional MIME filters (platform-dependent).
        mime_types: Vec<String>,
    },
    /// Shell optionally prompts for a name, then calls the plugin
    /// with no payload — the plugin produces an empty `Doc`.
    Blank { prompt_name: bool },
    /// Plugin fully owns the UX; the shell simply dispatches
    /// `execute_creation` with no payload or name hint.
    Custom,
}

/// Successful result of `execute_creation`.
///
/// The plugin hands back the bincoded bytes of the new item
/// (interpretation is private to the plugin) plus an optional display
/// name the shell can use as the initial title.
#[derive(Clone, Debug, PartialEq, Eq, Archive, Serialize, Deserialize)]
#[rkyv(compare(PartialEq), derive(Debug))]
pub struct ItemCreationResult {
    pub item_bytes: Vec<u8>,
    pub suggested_name: String,
}
