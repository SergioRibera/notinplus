//! References that follow from content.
//!
//! Bookmarks carry [`SourceRef`]s and note bodies carry `notein://`
//! links; both name other items. This index keeps those edges
//! queryable without opening every document on disk. It is a cache:
//! every entry is rebuilt from the item it belongs to on each save,
//! so the on-disk file can be deleted at any time and rebuilt by
//! [`rebuild`].
//!
//! State is process-wide because writers (autosave workers, the note
//! page) do not all hold the library lock. The file lives at
//! `<library root>/refs.bincode`.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use bincode::config::{self, Configuration};
use notein_plugin::SourceRef;

use super::bodies;
use super::model::ItemId;
use crate::deep_link::scan_links;
use crate::doc::Doc;

const CODEC: Configuration = config::standard();
const FILE: &str = "refs.bincode";
/// Schema version encoded into the cache file; a mismatch drops it.
const VERSION: u32 = 1;

/// Where a reference was written.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RefOrigin {
    /// A bookmark pinned on a canvas (raw bookmark id bytes).
    Bookmark([u8; 16]),
    /// A text box on a canvas.
    TextBox,
    /// The body of a text note.
    Note,
}

/// One outgoing reference.
#[istmo::message]
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RefEntry {
    pub from: ItemId,
    pub to: ItemId,
    pub origin: RefOrigin,
}

#[istmo::message]
#[derive(Clone, PartialEq, Eq, Debug)]
struct RefsFile {
    version: u32,
    entries: Vec<RefEntry>,
    /// Which item owns each bookmark, so a `SourceRef::Canvas` can be
    /// resolved to an item.
    pins: Vec<([u8; 16], ItemId)>,
}

impl Default for RefsFile {
    fn default() -> Self {
        Self {
            version: VERSION,
            entries: Vec::new(),
            pins: Vec::new(),
        }
    }
}

#[derive(Default)]
struct RefsIndex {
    entries: Vec<RefEntry>,
    pins: HashMap<[u8; 16], ItemId>,
    /// References whose `SourceRef::Canvas` target bookmark was not
    /// known when the source item was recorded. Resolved once the
    /// owning item is recorded.
    pending_pins: Vec<(ItemId, RefOrigin, [u8; 16])>,
}

impl RefsIndex {
    fn load() -> Self {
        let Ok(bytes) = fs::read(path()) else {
            return Self::default();
        };
        let file = bincode::decode_from_slice::<RefsFile, _>(&bytes, CODEC)
            .ok()
            .map(|(f, _)| f)
            .filter(|f| f.version == VERSION)
            .unwrap_or_default();
        Self {
            entries: file.entries,
            pins: file.pins.into_iter().collect(),
            pending_pins: Vec::new(),
        }
    }

    fn save(&self) -> io::Result<()> {
        let file = RefsFile {
            version: VERSION,
            entries: self.entries.clone(),
            pins: self.pins.iter().map(|(k, v)| (*k, *v)).collect(),
        };
        let bytes = bincode::encode_to_vec(&file, CODEC)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let dest = path();
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut tmp = dest.clone();
        tmp.set_extension("bincode.tmp");
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, dest)
    }

    /// Replace everything `from` references.
    fn replace(&mut self, from: ItemId, found: Vec<(RefOrigin, SourceRef)>) {
        self.entries.retain(|e| e.from != from);
        self.pending_pins.retain(|(src, _, _)| *src != from);
        for (origin, source) in found {
            match source {
                SourceRef::Plugin { item, .. } => {
                    let to = ItemId(item);
                    if to != from {
                        self.entries.push(RefEntry { from, to, origin });
                    }
                }
                SourceRef::Canvas { bookmark } => {
                    let key = bookmark.0;
                    match self.pins.get(&key) {
                        Some(&to) if to != from => {
                            self.entries.push(RefEntry { from, to, origin });
                        }
                        Some(_) => {}
                        None => self.pending_pins.push((from, origin, key)),
                    }
                }
                SourceRef::External { .. } => {}
            }
        }
    }

    /// Register which item owns `bookmarks`, resolving any reference
    /// that was waiting for them.
    fn record_pins(&mut self, owner: ItemId, bookmarks: impl IntoIterator<Item = [u8; 16]>) {
        self.pins.retain(|_, o| *o != owner);
        for key in bookmarks {
            self.pins.insert(key, owner);
        }
        let mut resolved = Vec::new();
        self.pending_pins
            .retain(|(from, origin, key)| match self.pins.get(key) {
                Some(&to) if to != *from => {
                    resolved.push(RefEntry {
                        from: *from,
                        to,
                        origin: *origin,
                    });
                    false
                }
                Some(_) => false,
                None => true,
            });
        self.entries.extend(resolved);
    }

    fn forget(&mut self, item: ItemId) {
        self.entries.retain(|e| e.from != item && e.to != item);
        self.pins.retain(|_, o| *o != item);
        self.pending_pins.retain(|(src, _, _)| *src != item);
    }
}

fn path() -> PathBuf {
    bodies::root_dir().join(FILE)
}

fn index() -> MutexGuard<'static, RefsIndex> {
    static R: OnceLock<Mutex<RefsIndex>> = OnceLock::new();
    match R.get_or_init(|| Mutex::new(RefsIndex::load())).lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

fn persist(guard: &RefsIndex) {
    if let Err(err) = guard.save() {
        log::warn!("refs index save: {err}");
    }
}

/// Everything a canvas doc references: bookmark chips, bookmark
/// bodies and text box bodies.
fn doc_refs(doc: &Doc) -> Vec<(RefOrigin, SourceRef)> {
    let mut out = Vec::new();
    for bm in doc.bookmarks() {
        let origin = RefOrigin::Bookmark(bm.id.0);
        out.extend(bm.refs.iter().cloned().map(|r| (origin, r)));
        out.extend(scan_links(&bm.body).map(|r| (origin, r)));
    }
    for layer in &doc.layers {
        for text in &layer.texts {
            out.extend(scan_links(&text.markdown).map(|r| (RefOrigin::TextBox, r)));
        }
    }
    out
}

/// References a canvas doc makes, captured so they can be recorded
/// later without holding on to the doc.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Captured {
    refs: Vec<(RefOrigin, SourceRef)>,
    pins: Vec<[u8; 16]>,
}

/// Collect what `doc` references right now.
#[must_use]
pub fn capture(doc: &Doc) -> Captured {
    Captured {
        refs: doc_refs(doc),
        pins: doc.bookmarks().iter().map(|b| b.id.0).collect(),
    }
}

/// Re-index the references a canvas doc makes. Call after every save.
pub fn record_doc(item: ItemId, doc: &Doc) {
    record_captured(item, capture(doc));
}

/// Re-index from a [`Captured`] set taken earlier with [`capture`].
pub fn record_captured(item: ItemId, captured: Captured) {
    let mut guard = index();
    guard.record_pins(item, captured.pins);
    guard.replace(item, captured.refs);
    persist(&guard);
    drop(guard);
}

/// Re-index the references the content of a plugin document makes (a
/// note's links). Call after every content write.
pub fn record_content(item: ItemId, refs: Vec<SourceRef>) {
    let found: Vec<(RefOrigin, SourceRef)> =
        refs.into_iter().map(|r| (RefOrigin::Note, r)).collect();
    let mut guard = index();
    guard.replace(item, found);
    persist(&guard);
    drop(guard);
}

/// Drop every reference from or to `item`.
pub fn forget(item: ItemId) {
    let mut guard = index();
    guard.forget(item);
    persist(&guard);
    drop(guard);
}

/// References `item` makes.
#[must_use]
pub fn refs_from(item: ItemId) -> Vec<RefEntry> {
    index()
        .entries
        .iter()
        .filter(|e| e.from == item)
        .cloned()
        .collect()
}

/// References pointing at `item`.
#[must_use]
pub fn refs_to(item: ItemId) -> Vec<RefEntry> {
    index()
        .entries
        .iter()
        .filter(|e| e.to == item)
        .cloned()
        .collect()
}

/// Item that owns the bookmark with the given id, if indexed.
#[must_use]
pub fn bookmark_owner(bookmark: [u8; 16]) -> Option<ItemId> {
    index().pins.get(&bookmark).copied()
}

/// Rebuild the whole cache from `docs` (canvas bodies) and `contents`
/// (what plugin documents link to). Everything not mentioned is dropped.
pub fn rebuild<'a>(
    docs: impl IntoIterator<Item = (ItemId, &'a Doc)>,
    contents: impl IntoIterator<Item = (ItemId, Vec<SourceRef>)>,
) {
    let mut guard = index();
    *guard = RefsIndex::default();
    let docs: Vec<(ItemId, &Doc)> = docs.into_iter().collect();
    for (item, doc) in &docs {
        guard.record_pins(*item, doc.bookmarks().iter().map(|b| b.id.0));
    }
    for (item, doc) in &docs {
        guard.replace(*item, doc_refs(doc));
    }
    for (item, refs) in contents {
        guard.replace(
            item,
            refs.into_iter().map(|r| (RefOrigin::Note, r)).collect(),
        );
    }
    persist(&guard);
    drop(guard);
}

#[cfg(test)]
mod tests {
    use super::*;
    use notein_plugin::BookmarkId;

    fn item(seed: u8) -> ItemId {
        ItemId([seed; 16])
    }

    fn plugin_ref(to: ItemId) -> SourceRef {
        SourceRef::Plugin {
            plugin_id: notein_notes::kind::KIND.into(),
            item: to.0,
            anchor: Vec::new(),
            selection: None,
        }
    }

    #[test]
    fn replace_keeps_only_latest_set_for_a_source() {
        let mut idx = RefsIndex::default();
        idx.replace(item(1), vec![(RefOrigin::Note, plugin_ref(item(2)))]);
        idx.replace(item(1), vec![(RefOrigin::Note, plugin_ref(item(3)))]);
        assert_eq!(idx.entries.len(), 1);
        assert_eq!(idx.entries[0].to, item(3));
    }

    #[test]
    fn self_references_are_ignored() {
        let mut idx = RefsIndex::default();
        idx.replace(item(1), vec![(RefOrigin::Note, plugin_ref(item(1)))]);
        assert!(idx.entries.is_empty());
    }

    #[test]
    fn canvas_refs_resolve_through_pins_even_when_recorded_later() {
        let bm = BookmarkId::from_bytes([9; 16]);
        let mut idx = RefsIndex::default();
        idx.replace(
            item(1),
            vec![(RefOrigin::Note, SourceRef::Canvas { bookmark: bm })],
        );
        assert!(idx.entries.is_empty());
        assert_eq!(idx.pending_pins.len(), 1);
        idx.record_pins(item(2), [[9; 16]]);
        assert_eq!(idx.entries.len(), 1);
        assert_eq!(idx.entries[0].to, item(2));
        assert!(idx.pending_pins.is_empty());
    }

    #[test]
    fn forget_drops_both_directions_and_pins() {
        let mut idx = RefsIndex::default();
        idx.record_pins(item(2), [[9; 16]]);
        idx.replace(item(1), vec![(RefOrigin::Note, plugin_ref(item(2)))]);
        idx.replace(item(3), vec![(RefOrigin::Note, plugin_ref(item(1)))]);
        idx.forget(item(2));
        assert_eq!(idx.entries.len(), 1);
        assert_eq!(idx.entries[0].from, item(3));
        assert!(idx.pins.is_empty());
    }
}
