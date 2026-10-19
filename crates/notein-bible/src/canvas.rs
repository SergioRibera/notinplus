//! `CanvasKind` implementation — Bible viewer as a canvas plugin.
//!
//! The Bible occupies the full pane. It declares `"bookmarks"` only —
//! no strokes, no layers; the shell gates stroke-tool UI off that list.
//! Rendering is handled in `src/ui/`; this module wires the shell
//! callbacks to the viewer state.
//!
//! Reading-only means `on_input` ignores pen pressure and never emits a
//! stroke op. All mutations are either navigation (scroll / chapter
//! change) or bookmark edits.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use notein_sdk::{
    Bookmark, BookmarkId, CanvasError, CanvasKind, ConfigError, ConfigSchema, ConfigValue,
    CreationCategory, CreationEntry, CreationError, FrameCtxHandle, Host, InputEvent,
    ItemCreationResult, Outline, OutlineIcon, OutlineNode, OutlineNodeId, WorldPoint,
    build::custom_entry,
};

use crate::anchor::BibleAnchor;
use crate::db::Db;
use crate::plugin_config::{self, SharedConfig};
use crate::state::BibleSnapshot;
use crate::state::{BibleBookmark, BibleViewerState, u32_to_rgba};

/// Canvas kind id used by the shell to route open requests.
pub const KIND_ID: &str = "io.notein.bible.viewer";

/// Capability list — the shell reads this to feature-gate stroke tools
/// off. Keep it in sync with the ui/toolbar visibility rules.
const CAPABILITIES: &[&str] = &["bookmarks"];

#[derive(Debug)]
pub struct BibleCanvas {
    db: Db,
    config: SharedConfig,
    /// Mirror of the active selection — populated by the UI reader
    /// when the user begins / extends a verse selection. Shared with
    /// `BibleSource` so `current_selection` can satisfy anchor
    /// requests coming from other canvases.
    selection: crate::source::SharedSelection,
    #[cfg(feature = "importer")]
    install_progress: crate::install_background::SharedInstallProgress,
    next_instance: std::sync::atomic::AtomicU64,
    instances: RwLock<HashMap<u64, Arc<RwLock<BibleViewerState>>>>,
}

impl BibleCanvas {
    #[must_use]
    pub fn new(db: Db, config: SharedConfig) -> Self {
        Self::with_selection(db, config, crate::source::new_shared_selection())
    }

    #[must_use]
    pub fn with_selection(
        db: Db,
        config: SharedConfig,
        selection: crate::source::SharedSelection,
    ) -> Self {
        Self {
            db,
            config,
            selection,
            #[cfg(feature = "importer")]
            install_progress: crate::install_background::new_shared_progress(),
            next_instance: std::sync::atomic::AtomicU64::new(1),
            instances: RwLock::new(HashMap::new()),
        }
    }

    #[cfg(feature = "importer")]
    #[must_use]
    pub fn with_install_progress(
        mut self,
        progress: crate::install_background::SharedInstallProgress,
    ) -> Self {
        self.install_progress = progress;
        self
    }

    /// Observable progress of the latest install request. UI can poll
    /// this to drive a progress bar in the install modal.
    #[cfg(feature = "importer")]
    #[must_use]
    pub fn install_progress(&self) -> &crate::install_background::SharedInstallProgress {
        &self.install_progress
    }

    /// Spawn a background install for `bible_id`. Returns immediately;
    /// progress lands in [`Self::install_progress`]. Replaces the old
    /// `set_config(KEY_INSTALL_BIBLE, Trigger)` entry point so the UI
    /// can pass the id straight through without staging it in config
    /// state first.
    #[cfg(feature = "importer")]
    pub fn request_install(&self, bible_id: String) {
        crate::install_background::spawn_install(
            bible_id,
            self.db.clone(),
            self.config.clone(),
            self.install_progress.clone(),
        );
    }

    /// Load the biblionlabs catalogue (fetched from the configured
    /// manifest URL and cached on disk). Blocking HTTP call — invoke
    /// from a background thread, not the UI thread.
    #[cfg(feature = "importer")]
    pub fn load_catalogue(
        &self,
    ) -> Result<
        crate::import::biblionlabs::CatalogueView,
        crate::import::biblionlabs::ImportError,
    > {
        crate::install_background::load_catalogue(&self.config)
    }

    /// Called by the UI when the user updates a selection. Writes
    /// through to the shared selection slot that `BibleSource` reads.
    pub fn publish_selection(&self, bible_id: &str, selection: Option<crate::state::VerseSelection>) {
        let mut guard = self.selection.write().unwrap();
        *guard = selection.map(|s| (bible_id.to_string(), s));
    }

    fn get(&self, instance: u64) -> Option<Arc<RwLock<BibleViewerState>>> {
        self.instances.read().unwrap().get(&instance).cloned()
    }

    /// Expose the shared DB for sibling modules (ui/, source.rs).
    #[must_use]
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// Expose the shared config so UI widgets can observe font / line
    /// height without round-tripping through the trait method.
    #[must_use]
    pub fn config(&self) -> &SharedConfig {
        &self.config
    }

    /// Translation an item's snapshot reads, or the default one for an
    /// item that was never opened.
    fn outline_bible(&self, snapshot: &[u8]) -> String {
        BibleSnapshot::decode(snapshot)
            .map(|snapshot| snapshot.bible_id)
            .filter(|bible_id| !bible_id.is_empty())
            .unwrap_or_else(|| plugin_config::effective_default_bible(&self.config, &self.db))
    }
}

impl CanvasKind for BibleCanvas {
    fn kind_id(&self) -> &str {
        KIND_ID
    }

    fn kind_label(&self) -> &str {
        "Biblia"
    }

    fn capabilities(&self) -> &[&'static str] {
        CAPABILITIES
    }

    fn open(&self, item_bytes: &[u8]) -> Result<u64, CanvasError> {
        // Item bytes = bincoded (bible_id, book_id, chapter). On first
        // open we hydrate the viewer state and fall through to the
        // on-demand loader. An empty slice means "open user's default
        // Bible at Genesis 1".
        let default_bible = plugin_config::effective_default_bible(&self.config, &self.db);
        let (bible_id, book_id, chapter) = decode_open_payload(item_bytes).unwrap_or_else(|| {
            (
                if default_bible.is_empty() {
                    "spa_rv1960".into()
                } else {
                    default_bible
                },
                "GEN".into(),
                1,
            )
        });

        let mut state = BibleViewerState::new(bible_id, book_id, chapter);
        if let Ok(chap) = self.db.load_chapter(&state.bible_id, &state.current_book, state.current_chapter) {
            state.push_chapter(chap);
        }

        let id = self
            .next_instance
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.instances
            .write()
            .unwrap()
            .insert(id, Arc::new(RwLock::new(state)));
        Ok(id)
    }

    fn close(&self, instance: u64) {
        self.instances.write().unwrap().remove(&instance);
    }

    fn render(&self, instance: u64, _ctx: FrameCtxHandle) {
        // UI chapter renderer lands here once wired. Shell delivers a
        // skia-wrapped FrameCtxHandle; the UI module owns the paragraph
        // + drop-cap + toolbar paint path.
        let _ = instance;
    }

    fn on_input(&self, instance: u64, event: InputEvent) {
        let Some(state) = self.get(instance) else { return };
        let mut st = state.write().unwrap();
        match event {
            InputEvent::Scroll { dy, .. } => {
                st.scroll_y = (st.scroll_y - dy).max(0.0);
            }
            InputEvent::KeyDown { code, modifiers: _ } => {
                // Arrow keys drive chapter nav. Codes match the Freya /
                // winit `KeyCode::ArrowLeft` / `ArrowRight` discriminants.
                // Looking up the books list is a DB round-trip but
                // chapter-switch is user-driven, not per-frame, so the
                // cost is irrelevant.
                let books = match self.db.list_books(&st.bible_id) {
                    Ok(b) => b,
                    Err(e) => {
                        log::warn!("bible: list_books failed on nav: {e}");
                        return;
                    }
                };
                let result = match code {
                    // ArrowLeft
                    0x25 | 0x14 => st.go_prev_chapter(&self.db, &books),
                    // ArrowRight
                    0x27 | 0x13 => st.go_next_chapter(&self.db, &books),
                    _ => return,
                };
                if let Err(e) = result {
                    log::warn!("bible: chapter nav failed: {e}");
                }
            }
            // Pointer events are consumed by the UI selection path.
            _ => {}
        }
    }

    fn create_bookmark(&self, instance: u64, _at: WorldPoint) -> Option<BookmarkId> {
        // The shell may call this on a bookmark-tool click. For Bible,
        // we promote the current selection — WorldPoint is irrelevant
        // for reflow text, verse-coord is authoritative.
        let state = self.get(instance)?;
        state
            .write()
            .unwrap()
            .commit_selection_as_highlight(None, "")
    }

    fn list_bookmarks(&self, instance: u64) -> Vec<Bookmark> {
        let Some(state) = self.get(instance) else { return Vec::new() };
        let st = state.read().unwrap();
        st.highlights
            .iter()
            .map(|h| bible_to_shell(h, &st.bible_id))
            .collect()
    }

    fn update_bookmark(&self, instance: u64, bookmark: Bookmark) {
        let Some(state) = self.get(instance) else { return };
        let mut st = state.write().unwrap();
        if let Some(existing) = st.highlights.iter_mut().find(|h| h.id == bookmark.id) {
            existing.color = bookmark.color.map(crate::state::rgba_to_u32);
            existing.note = bookmark.body;
        }
    }

    fn delete_bookmark(&self, instance: u64, id: BookmarkId) {
        let Some(state) = self.get(instance) else { return };
        state.write().unwrap().remove_highlight(id);
    }

    fn navigate_to_bookmark(&self, instance: u64, id: BookmarkId) {
        let Some(state) = self.get(instance) else { return };
        let target = {
            let st = state.read().unwrap();
            st.highlights
                .iter()
                .find(|h| h.id == id)
                .map(|h| (h.book_id.clone(), h.chapter))
        };
        let Some((book, chap)) = target else { return };
        if let Err(e) = state.write().unwrap().jump_to(&self.db, &book, chap) {
            log::warn!("bible: navigate_to_bookmark failed: {e}");
        }
    }

    fn serialize_snapshot(&self, instance: u64) -> Vec<u8> {
        let Some(state) = self.get(instance) else { return Vec::new() };
        let st = state.read().unwrap();
        crate::state::BibleSnapshot::from_state(&st).encode()
    }

    fn restore(&self, instance: u64, snapshot: &[u8], _at: notein_sdk::OpId) {
        let Some(state) = self.get(instance) else { return };
        let Some(snap) = crate::state::BibleSnapshot::decode(snapshot) else {
            log::warn!("bible: snapshot decode failed; keeping default state");
            return;
        };
        let mut st = state.write().unwrap();
        snap.apply_to(&mut st);
        if let Ok(chap) = self
            .db
            .load_chapter(&st.bible_id, &st.current_book, st.current_chapter)
        {
            st.push_chapter(chap);
        }
    }

    fn apply_remote_op(&self, _instance: u64, _op: &[u8]) {}

    fn install_host(&self, host: Host) {
        // Push every bundled Lora face to the renderer under the same
        // family name — the shell no longer has to know this plugin
        // needs a specific font at startup. Skia groups the eight TTFs
        // by family + resolves weight / italic per request.
        for (_name, bytes) in crate::fonts::all() {
            host.load_font(crate::fonts::FAMILY, bytes.to_vec());
        }
        plugin_config::set_host(&self.config, host);
    }

    /// Books of the translation the item reads, each with its chapters.
    fn outline(&self, snapshot: &[u8]) -> Outline {
        let bible_id = self.outline_bible(snapshot);
        let books = self.db.list_books(&bible_id).unwrap_or_default();
        let nodes = books
            .iter()
            .enumerate()
            .map(|(index, book)| {
                let book_key = (index as u64 + 1) << 16;
                let chapters = self.db.chapter_count(&bible_id, &book.book_id).unwrap_or(0);
                OutlineNode {
                    id: OutlineNodeId(book_key),
                    label: book.name.clone(),
                    subtitle: Some(format!("{chapters} capítulos")),
                    icon: OutlineIcon::Section,
                    children: (1..=chapters)
                        .map(|chapter| {
                            OutlineNode::new(
                                OutlineNodeId(book_key | u64::from(chapter)),
                                format!("{} {chapter}", book.name),
                            )
                        })
                        .collect(),
                }
            })
            .collect();
        Outline { nodes, revision: 0 }
    }

    /// Chapter 1 for a book row, the chapter itself for a chapter row.
    /// Verse `0` asks the reader to jump without flashing a verse.
    fn outline_anchor(&self, snapshot: &[u8], node: OutlineNodeId) -> Option<Vec<u8>> {
        let bible_id = self.outline_bible(snapshot);
        let index = usize::try_from(node.0 >> 16).ok()?.checked_sub(1)?;
        let chapter = u16::try_from(node.0 & 0xFFFF).ok()?.max(1);
        let book = self.db.list_books(&bible_id).ok()?.into_iter().nth(index)?;
        Some(BibleAnchor::single_verse(bible_id, book.book_id, chapter, 0).encode())
    }

    fn config_schema(&self) -> ConfigSchema {
        plugin_config::build_schema(&self.db)
    }

    fn get_config(&self, key: &str) -> Option<ConfigValue> {
        plugin_config::get_value(&self.config, key)
    }

    fn set_config(&self, key: &str, value: ConfigValue) -> Result<(), ConfigError> {
        plugin_config::apply_set(&self.config, key, value)
    }

    fn creation_entries(&self) -> Vec<CreationEntry> {
        vec![custom_entry(
            "new-bible-viewer",
            "Nueva Biblia",
            freya_icons::lucide::book_open(),
            CreationCategory::Canvas,
        )]
    }

    fn execute_creation(
        &self,
        entry_id: &str,
        _import_bytes: Option<&[u8]>,
        _name_hint: Option<&str>,
    ) -> Result<ItemCreationResult, CreationError> {
        if entry_id != "new-bible-viewer" {
            return Err(CreationError::UnknownEntry(entry_id.into()));
        }
        let default_bible = plugin_config::effective_default_bible(&self.config, &self.db);
        let bible_id = if default_bible.is_empty() {
            "spa_rv1960".into()
        } else {
            default_bible
        };
        let bytes = encode_open_payload(&bible_id, "GEN", 1);
        Ok(ItemCreationResult {
            item_bytes: bytes,
            suggested_name: "Biblia".into(),
        })
    }
}

/// Map an internal `BibleBookmark` to the shell's `Bookmark` shape.
/// `anchor` carries no meaningful pixel coord — set to `(0, 0)` and
/// kept stable so the shell's generic bookmark panel still has a sort
/// key. The real anchor lives in the plugin-private highlights list.
fn bible_to_shell(h: &BibleBookmark, _bible_id: &str) -> Bookmark {
    Bookmark {
        id: h.id,
        anchor: WorldPoint { x: 0.0, y: f32::from(h.chapter) * 1000.0 + f32::from(h.start_verse) },
        stuck_to: None,
        body: h.note.clone(),
        refs: Vec::new(),
        color: h.color.map(u32_to_rgba),
        created_at: 0,
        updated_at: 0,
        lamport: 0,
    }
}

/// Convenience for callers that want the anchor payload for a given
/// highlight — fed into `SourceRef::Plugin { anchor }` when a user
/// links from a different canvas to this highlight.
#[must_use]
pub fn highlight_anchor(h: &BibleBookmark, bible_id: &str) -> Vec<u8> {
    BibleAnchor {
        bible_id: bible_id.into(),
        book_id: h.book_id.clone(),
        start_chapter: h.chapter,
        start_verse: h.start_verse,
        end_chapter: h.chapter,
        end_verse: h.end_verse,
        start_char: None,
        end_char: None,
    }
    .encode()
}

pub fn encode_open_payload(bible_id: &str, book_id: &str, chapter: u16) -> Vec<u8> {
    bincode::encode_to_vec(
        &(bible_id.to_string(), book_id.to_string(), chapter),
        bincode::config::standard(),
    )
    .expect("open payload encode: standard bincode config cannot fail")
}

pub fn decode_open_payload(bytes: &[u8]) -> Option<(String, String, u16)> {
    if bytes.is_empty() {
        return None;
    }
    bincode::decode_from_slice::<(String, String, u16), _>(bytes, bincode::config::standard())
        .ok()
        .map(|(v, _)| v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn seeded_db() -> Db {
        let db = Db::in_memory().unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO bibles (id, abbrev, name_local, language)
                 VALUES ('spa_rv1960', 'RVR1960', 'Reina Valera 1960', 'spa')",
                [],
            )?;
            conn.execute(
                "INSERT INTO books (bible_id, book_id, name, abbrev, order_num, testament)
                 VALUES ('spa_rv1960', 'EPH', 'Efesios', 'Ef', 49, 'NT')",
                [],
            )?;
            for v in 1..=3 {
                conn.execute(
                    "INSERT INTO verses (bible_id, book_id, chapter, verse, text, paragraph_break)
                     VALUES ('spa_rv1960', 'EPH', 6, ?1, 'placeholder', 0)",
                    params![v as i64],
                )?;
            }
            Ok(())
        })
        .unwrap();
        db
    }

    #[test]
    fn outline_lists_books_and_anchors_chapters() {
        let canvas = BibleCanvas::new(seeded_db(), plugin_config::new_shared());
        let snapshot = BibleSnapshot {
            version: BibleSnapshot::VERSION,
            bible_id: "spa_rv1960".into(),
            current_book: "EPH".into(),
            current_chapter: 6,
            highlights: Vec::new(),
        }
        .encode();
        let outline = canvas.outline(&snapshot);
        assert_eq!(outline.nodes.len(), 1);
        let book = &outline.nodes[0];
        assert_eq!(book.label, "Efesios");
        let chapter = book.children.last().expect("chapters");
        assert_eq!(chapter.label, "Efesios 6");

        let anchor = canvas
            .outline_anchor(&snapshot, chapter.id)
            .and_then(|bytes| BibleAnchor::decode(&bytes))
            .expect("chapter anchor");
        assert_eq!((anchor.book_id.as_str(), anchor.start_chapter, anchor.start_verse), ("EPH", 6, 0));
        let first = canvas
            .outline_anchor(&snapshot, book.id)
            .and_then(|bytes| BibleAnchor::decode(&bytes))
            .expect("book anchor");
        assert_eq!(first.start_chapter, 1);
    }

    #[test]
    fn open_from_payload_loads_chapter() {
        let db = seeded_db();
        let canvas = BibleCanvas::new(db, plugin_config::new_shared());
        let bytes = encode_open_payload("spa_rv1960", "EPH", 6);
        let inst = canvas.open(&bytes).unwrap();
        assert_eq!(canvas.title(inst), "Efesios 6");
        canvas.close(inst);
        assert_eq!(canvas.title(inst), "Biblia");
    }

    #[test]
    fn open_empty_payload_uses_configured_default_bible() {
        let db = seeded_db();
        let config = plugin_config::new_shared();
        config
            .write()
            .unwrap()
            .values
            .insert(
                plugin_config::KEY_DEFAULT_BIBLE.into(),
                ConfigValue::String("spa_rv1960".into()),
            );
        let canvas = BibleCanvas::new(db, config);
        let inst = canvas.open(&[]).unwrap();
        // Genesis is seeded empty here, but the Bible id resolution
        // flows from the config, not the hard-coded fallback.
        let st = canvas.get(inst).unwrap();
        assert_eq!(st.read().unwrap().bible_id, "spa_rv1960");
    }

    #[test]
    fn set_config_rejects_out_of_range() {
        let canvas = BibleCanvas::new(Db::in_memory().unwrap(), plugin_config::new_shared());
        assert!(canvas
            .set_config(
                plugin_config::KEY_FONT_SIZE,
                ConfigValue::Float(plugin_config::DEFAULT_FONT_SIZE),
            )
            .is_ok());
        assert!(matches!(
            canvas.set_config(plugin_config::KEY_FONT_SIZE, ConfigValue::Float(100.0)),
            Err(ConfigError::TypeMismatch)
        ));
        assert!(matches!(
            canvas.set_config("unknown_key", ConfigValue::Bool(true)),
            Err(ConfigError::UnknownKey(_))
        ));
    }

    // Trait method surface for consumers inside this crate's tests.
    impl BibleCanvas {
        fn title(&self, instance: u64) -> String {
            let Some(st) = self.get(instance) else { return "Biblia".into() };
            let st = st.read().unwrap();
            if let Some(chap) = st.current_chapter_data() {
                return format!("{} {}", chap.book_name, chap.chapter);
            }
            "Biblia".into()
        }
    }
}
