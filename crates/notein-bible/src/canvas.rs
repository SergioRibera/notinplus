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
    CreationCategory, CreationEntry, CreationError, FrameCtxHandle, InputEvent,
    ItemCreationResult, WorldPoint,
    build::custom_entry,
};

use crate::db::Db;
use crate::state::BibleViewerState;

/// Canvas kind id used by the shell to route open requests.
pub const KIND_ID: &str = "io.notein.bible.viewer";

/// Capability list — the shell reads this to feature-gate stroke tools
/// off. Keep it in sync with the ui/toolbar visibility rules.
const CAPABILITIES: &[&str] = &["bookmarks"];

#[derive(Debug)]
pub struct BibleCanvas {
    db: Db,
    next_instance: std::sync::atomic::AtomicU64,
    instances: RwLock<HashMap<u64, Arc<RwLock<BibleViewerState>>>>,
}

impl BibleCanvas {
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self {
            db,
            next_instance: std::sync::atomic::AtomicU64::new(1),
            instances: RwLock::new(HashMap::new()),
        }
    }

    fn get(&self, instance: u64) -> Option<Arc<RwLock<BibleViewerState>>> {
        self.instances.read().unwrap().get(&instance).cloned()
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
        let (bible_id, book_id, chapter) = decode_open_payload(item_bytes)
            .unwrap_or_else(|| ("spa_rv1960".into(), "GEN".into(), 1));

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
            // Reading-only canvas: pointer events feed selection (8.8),
            // not strokes. Everything else is ignored at this layer.
            _ => {}
        }
    }

    fn create_bookmark(&self, instance: u64, _at: WorldPoint) -> Option<BookmarkId> {
        // Highlights are driven from the UI selection path, not the
        // shell's generic bookmark-at-point call: WorldPoint is not
        // meaningful for reflow-able text.
        let _ = instance;
        None
    }

    fn list_bookmarks(&self, _instance: u64) -> Vec<Bookmark> {
        Vec::new()
    }

    fn update_bookmark(&self, _instance: u64, _bookmark: Bookmark) {}

    fn delete_bookmark(&self, _instance: u64, _id: BookmarkId) {}

    fn navigate_to_bookmark(&self, _instance: u64, _id: BookmarkId) {}

    fn serialize_snapshot(&self, instance: u64) -> Vec<u8> {
        let _ = instance;
        Vec::new()
    }

    fn apply_remote_op(&self, _instance: u64, _op: &[u8]) {}

    fn config_schema(&self) -> ConfigSchema {
        ConfigSchema { version: 1, fields: Vec::new() }
    }

    fn get_config(&self, _key: &str) -> Option<ConfigValue> {
        None
    }

    fn set_config(&self, _key: &str, _value: ConfigValue) -> Result<(), ConfigError> {
        Ok(())
    }

    fn creation_entries(&self) -> Vec<CreationEntry> {
        vec![custom_entry(
            "new-bible-viewer",
            "Nueva Biblia",
            notein_sdk::bytes::Bytes::from_static(b"\xf0\x9f\x93\x96"),
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
        // Default new tab = user's primary Bible at Genesis 1. The
        // toolbar translation picker can retarget without creating a
        // fresh item.
        let bytes = encode_open_payload("spa_rv1960", "GEN", 1);
        Ok(ItemCreationResult {
            item_bytes: bytes,
            suggested_name: "Biblia".into(),
        })
    }
}

fn encode_open_payload(bible_id: &str, book_id: &str, chapter: u16) -> Vec<u8> {
    bincode::encode_to_vec(
        &(bible_id.to_string(), book_id.to_string(), chapter),
        bincode::config::standard(),
    )
    .expect("open payload encode: standard bincode config cannot fail")
}

fn decode_open_payload(bytes: &[u8]) -> Option<(String, String, u16)> {
    if bytes.is_empty() {
        return None;
    }
    bincode::decode_from_slice::<(String, String, u16), _>(bytes, bincode::config::standard())
        .ok()
        .map(|(v, _)| v)
}
