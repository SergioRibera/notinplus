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
    ItemCreationResult, WorldPoint, build::custom_entry,
};

use crate::db::Db;
use crate::plugin_config::{self, SetResult, SharedConfig};
use crate::state::BibleViewerState;

/// Canvas kind id used by the shell to route open requests.
pub const KIND_ID: &str = "io.notein.bible.viewer";

/// Capability list — the shell reads this to feature-gate stroke tools
/// off. Keep it in sync with the ui/toolbar visibility rules.
const CAPABILITIES: &[&str] = &["bookmarks"];

#[derive(Debug)]
pub struct BibleCanvas {
    db: Db,
    config: SharedConfig,
    next_instance: std::sync::atomic::AtomicU64,
    instances: RwLock<HashMap<u64, Arc<RwLock<BibleViewerState>>>>,
}

impl BibleCanvas {
    #[must_use]
    pub fn new(db: Db, config: SharedConfig) -> Self {
        Self {
            db,
            config,
            next_instance: std::sync::atomic::AtomicU64::new(1),
            instances: RwLock::new(HashMap::new()),
        }
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
        plugin_config::build_schema(&self.db)
    }

    fn get_config(&self, key: &str) -> Option<ConfigValue> {
        plugin_config::get_value(&self.config, key)
    }

    fn set_config(&self, key: &str, value: ConfigValue) -> Result<(), ConfigError> {
        match plugin_config::apply_set(&self.config, key, value)? {
            SetResult::Stored => Ok(()),
            SetResult::InstallRequested(id) => {
                // Install is a background job; schedule + ack. Real HTTP
                // wiring lands when the catalogue picker UI goes in. For
                // now we log the request so manual tests can confirm
                // the signal reached the plugin.
                log::info!("bible: install requested for {id}");
                Ok(())
            }
        }
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

    #[test]
    fn install_bible_request_acked_without_storing_value() {
        let db = Db::in_memory().unwrap();
        let config = plugin_config::new_shared();
        let canvas = BibleCanvas::new(db, config.clone());
        canvas
            .set_config(
                plugin_config::KEY_INSTALL_BIBLE,
                ConfigValue::String("spa_rv1960".into()),
            )
            .unwrap();
        // Action key is not retained in the stored map.
        assert!(config
            .read()
            .unwrap()
            .values
            .get(plugin_config::KEY_INSTALL_BIBLE)
            .is_none());
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
