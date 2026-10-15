//! Process-wide static catalog of entries the `[[` autocomplete
//! popup can insert.
//!
//! Rationale: computing completions by iterating a plugin's live
//! instance map on every keystroke only surfaces docs that are
//! currently open. Users expect to link to any PDF they imported,
//! whether or not it's got a viewer open right now. The catalog
//! captures the entry when the document is first processed (open /
//! import) and keeps it until the owning item is deleted — so the
//! `[[` popup lists every "linkable thing" without needing a fresh
//! query to each plugin instance.
//!
//! Plugins register from their open path:
//!
//! ```ignore
//! completion_catalog::register(CatalogEntry {
//!     plugin_id: KIND_ID.to_owned(),
//!     item_id,
//!     label: "Paper X".into(),
//!     subtitle: Some("PDF · 24 páginas".into()),
//!     source: SourceRef::Plugin { ... },
//! });
//! ```
//!
//! Keeping it in a single shared `Mutex<Vec<_>>` is deliberate — the
//! typical count is low (dozens, maybe low hundreds) and the lookup
//! is a case-insensitive substring filter. If this starts dominating
//! the frame budget on big libraries, swap for an indexed trie.

use std::sync::{Mutex, OnceLock};

use notein_plugin::{Completion, SourceRef};

use crate::library::ItemId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogEntry {
    /// Who owns the entry — matches the owning plugin's `KIND_ID`.
    pub plugin_id: String,
    /// Library item backing the entry. Multiple entries per item are
    /// legal (e.g. one per PDF page) — the (plugin_id, item_id) pair
    /// is NOT a dedup key.
    pub item_id: ItemId,
    pub label: String,
    pub subtitle: Option<String>,
    pub source: SourceRef,
}

type Catalog = Mutex<Vec<CatalogEntry>>;

fn catalog() -> &'static Catalog {
    static C: OnceLock<Catalog> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Vec::new()))
}

fn lock() -> std::sync::MutexGuard<'static, Vec<CatalogEntry>> {
    match catalog().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Add `entry` to the catalog. Replaces any existing entry with the
/// same `(plugin_id, item_id)` so a re-open refreshes the stored
/// metadata (page count, title, …).
pub fn register(entry: CatalogEntry) {
    let mut g = lock();
    g.retain(|e| !(e.plugin_id == entry.plugin_id && e.item_id == entry.item_id));
    g.push(entry);
}

/// Drop every entry whose `(plugin_id, item_id)` matches. Called by
/// the shell when a library item is deleted or by a plugin when it
/// no longer wants a stale entry visible.
pub fn remove(plugin_id: &str, item_id: ItemId) {
    let mut g = lock();
    g.retain(|e| !(e.plugin_id == plugin_id && e.item_id == item_id));
}

/// Case-insensitive substring filter on `label` + `subtitle`.
/// Returns completions sorted by `label` for stable popup order.
#[must_use]
pub fn query(needle: &str) -> Vec<Completion> {
    let needle = needle.to_lowercase();
    let mut matches: Vec<CatalogEntry> = {
        let g = lock();
        g.iter()
            .filter(|e| {
                if needle.is_empty() {
                    return true;
                }
                let label = e.label.to_lowercase();
                if label.contains(&needle) {
                    return true;
                }
                e.subtitle
                    .as_deref()
                    .is_some_and(|s| s.to_lowercase().contains(&needle))
            })
            .cloned()
            .collect()
    };
    matches.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    matches
        .into_iter()
        .map(|e| Completion {
            label: e.label,
            subtitle: e.subtitle,
            source: e.source,
        })
        .collect()
}

/// Debug: dump every registered entry. Used by `log::debug` traces +
/// a future `/catalog` dev command.
#[must_use]
pub fn snapshot() -> Vec<CatalogEntry> {
    lock().clone()
}
