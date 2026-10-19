//! Process-wide static catalog of entries the `[` reference picker
//! popup can insert.
//!
//! Rationale: computing completions by iterating a plugin's live
//! instance map on every keystroke only surfaces docs that are
//! currently open. Users expect to link to any PDF they imported,
//! whether or not it's got a viewer open right now. The catalog
//! captures the entry when the document is first processed (open /
//! import) and keeps it until the owning item is deleted — so the
//! `[` popup lists every "linkable thing" without needing a fresh
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

use crate::builtin::pdf_source::KIND_ID as PDF_PLUGIN_ID;
use crate::library::{DocKind, Item, ItemId};
use crate::pdf_viewer::{PdfAnchor, encode_pdf_anchor};

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

/// Drop every entry owned by `plugin_id`. Used when a plugin
/// repopulates its slice from a persisted source — call this first,
/// then `register` each entry fresh.
pub fn clear_plugin(plugin_id: &str) {
    lock().retain(|e| e.plugin_id != plugin_id);
}

/// Rebuild the PDF slice of the catalog from a snapshot of library
/// items. Call this on every library open so a fresh app start sees
/// every PDF the user has imported, not only the ones they've
/// re-opened this session. Idempotent: existing PDF entries are
/// cleared before the rebuild.
///
/// Uses the item's user-facing `name` for the chip label — the plugin
/// cannot reach the library on its own.
pub fn prime_pdf_entries<'a, I: IntoIterator<Item = &'a Item>>(items: I) {
    clear_plugin(PDF_PLUGIN_ID);
    for item in items {
        if !is_pdf_item(&item.kind) {
            continue;
        }
        let anchor = encode_pdf_anchor(PdfAnchor::Page {
            source: item.id,
            page: 0,
        });
        let short_id: String = item.id.to_string().chars().take(8).collect();
        let name = if item.name.trim().is_empty() {
            format!("PDF {short_id}")
        } else {
            item.name.clone()
        };
        register(CatalogEntry {
            plugin_id: PDF_PLUGIN_ID.to_owned(),
            item_id: item.id,
            label: name,
            subtitle: Some(format!("PDF · {short_id}")),
            source: SourceRef::Plugin {
                plugin_id: PDF_PLUGIN_ID.to_owned(),
                item: item.id.0,
                anchor,
                selection: None,
            },
        });
    }
}

/// Refresh the page-count subtitle for the PDF entry pointing at
/// `item`. If the entry already exists (e.g. populated by
/// [`prime_pdf_entries`] on library open), only the subtitle is
/// touched — the human label stays as the user's item name.
/// Otherwise registers a short-id fallback label so the chip is
/// still linkable before the next library prime.
pub fn ensure_pdf_entry(item: ItemId, page_count: usize) {
    let short_id: String = item.to_string().chars().take(8).collect();
    let anchor = encode_pdf_anchor(PdfAnchor::Page {
        source: item,
        page: 0,
    });
    let new_subtitle = Some(format!("PDF · {page_count} páginas · {short_id}"));
    let mut g = lock();
    if let Some(entry) = g
        .iter_mut()
        .find(|e| e.plugin_id == PDF_PLUGIN_ID && e.item_id == item)
    {
        entry.subtitle = new_subtitle;
        entry.source = SourceRef::Plugin {
            plugin_id: PDF_PLUGIN_ID.to_owned(),
            item: item.0,
            anchor,
            selection: None,
        };
        return;
    }
    g.push(CatalogEntry {
        plugin_id: PDF_PLUGIN_ID.to_owned(),
        item_id: item,
        label: format!("PDF {short_id}"),
        subtitle: new_subtitle,
        source: SourceRef::Plugin {
            plugin_id: PDF_PLUGIN_ID.to_owned(),
            item: item.0,
            anchor,
            selection: None,
        },
    });
}

fn is_pdf_item(kind: &DocKind) -> bool {
    kind.is(PDF_PLUGIN_ID)
}
