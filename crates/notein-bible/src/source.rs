//! `Source` implementation — provides anchor resolution + FTS to the
//! shell's bookmark system.
//!
//! Rendering stays in the canvas side (`src/canvas.rs`) — the Bible is
//! not tile-based, so `render_tile` returns `UnsupportedCapability`.
//! Callers wanting to show Bible content open a `BibleCanvas` instance;
//! callers wanting to link to a verse from elsewhere use this `Source`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use notein_sdk::{
    AnchorView, ChipView, ConfigError, ConfigSchema, ConfigValue, CreationCategory, CreationEntry,
    CreationError, ItemCreationResult, SearchHit, Selection, Source, SourceError, Tile,
    TileRequest, build::custom_entry,
};

use crate::anchor::BibleAnchor;
use crate::db::Db;
use crate::plugin_config::{self, SetResult, SharedConfig};

pub const KIND_ID: &str = "io.notein.bible";

#[derive(Debug)]
pub struct BibleSource {
    db: Db,
    config: SharedConfig,
    /// Active search streams. Keyed by stream id; drained by poll +
    /// cleared on end. Streams are eager (we run the FTS5 query in
    /// `search_begin` and buffer the results) because the query cost is
    /// tiny — chunking only matters for live-crawled sources.
    streams: RwLock<HashMap<u64, Vec<SearchHit>>>,
    next_stream: std::sync::atomic::AtomicU64,
    /// `(instance_id → bible_id)` so `Source::open` can accept a
    /// translation and later anchors/searches scope to that one.
    instances: RwLock<HashMap<u64, Arc<String>>>,
    next_instance: std::sync::atomic::AtomicU64,
}

impl BibleSource {
    #[must_use]
    pub fn new(db: Db, config: SharedConfig) -> Self {
        Self {
            db,
            config,
            streams: RwLock::new(HashMap::new()),
            next_stream: std::sync::atomic::AtomicU64::new(1),
            instances: RwLock::new(HashMap::new()),
            next_instance: std::sync::atomic::AtomicU64::new(1),
        }
    }
}

impl Source for BibleSource {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn kind_id(&self) -> &str {
        KIND_ID
    }

    fn kind_label(&self) -> &str {
        "Biblia"
    }

    fn file_extensions(&self) -> &[&'static str] {
        // No file-backed import at the Source level — translations are
        // installed via the biblionlabs pipeline, not drag-and-dropped.
        &[]
    }

    fn open(&self, item_bytes: &[u8]) -> Result<u64, SourceError> {
        // item_bytes = bincoded bible_id String. Empty → default.
        let bible_id: String = if item_bytes.is_empty() {
            let default = plugin_config::effective_default_bible(&self.config, &self.db);
            if default.is_empty() { "spa_rv1960".into() } else { default }
        } else {
            bincode::decode_from_slice::<String, _>(item_bytes, bincode::config::standard())
                .map(|(v, _)| v)
                .map_err(|e| SourceError::OpenFailed(e.to_string()))?
        };
        let id = self
            .next_instance
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.instances
            .write()
            .unwrap()
            .insert(id, Arc::new(bible_id));
        Ok(id)
    }

    fn close(&self, instance: u64) {
        self.instances.write().unwrap().remove(&instance);
    }

    fn title(&self, instance: u64) -> String {
        self.instances
            .read()
            .unwrap()
            .get(&instance)
            .map(|s| (**s).clone())
            .unwrap_or_else(|| "Biblia".into())
    }

    fn content_size(&self, _instance: u64) -> (f32, f32) {
        // No native content coordinate system for a text source. The
        // canvas owns layout; this is only consulted by generic shell
        // zoom code which has no job here.
        (0.0, 0.0)
    }

    fn render_tile(&self, _instance: u64, _req: TileRequest) -> Result<Tile, SourceError> {
        Err(SourceError::RenderFailed(
            "Bible is not tile-based; open via BibleCanvas instead".into(),
        ))
    }

    fn hit_test(&self, _instance: u64, _x: f32, _y: f32) -> Option<Vec<u8>> {
        None
    }

    fn resolve_anchor(&self, _instance: u64, anchor: &[u8]) -> Option<AnchorView> {
        let BibleAnchor {
            bible_id,
            book_id,
            start_chapter,
            start_verse,
            end_chapter,
            end_verse,
            ..
        } = BibleAnchor::decode(anchor)?;

        let chapter = self
            .db
            .load_chapter(&bible_id, &book_id, start_chapter)
            .ok()?;
        let snippet = chapter
            .verses
            .iter()
            .find(|v| v.verse == start_verse)
            .map(|v| v.text.clone())
            .unwrap_or_default();

        let title = if start_chapter == end_chapter && start_verse == end_verse {
            format!("{} {start_chapter}:{start_verse}", chapter.book_name)
        } else {
            format!(
                "{} {start_chapter}:{start_verse}-{end_chapter}:{end_verse}",
                chapter.book_name
            )
        };

        Some(AnchorView { title, snippet })
    }

    fn current_selection(&self, _instance: u64) -> Option<Selection> {
        // Selection emission lives on the canvas side; when the host
        // asks the Source for a selection to seed a chip, the UI
        // mirrors its current verse selection into this spot via a
        // shared selection registry.
        None
    }

    fn search_begin(&self, instance: u64, query: &str) -> u64 {
        let bible_id = self
            .instances
            .read()
            .unwrap()
            .get(&instance)
            .cloned();
        let bid = bible_id.as_deref().map(String::as_str);
        let hits = self.db.search(bid, query, 100).unwrap_or_default();

        let shell_hits: Vec<SearchHit> = hits
            .into_iter()
            .map(|h| {
                let anchor = BibleAnchor::single_verse(
                    &h.bible_id,
                    &h.book_id,
                    h.chapter,
                    h.verse,
                );
                // FTS5 BM25 ascends (lower = better). The shell expects
                // "higher = better"; negate so the sort flips naturally
                // in whatever UI picks a top-N view.
                SearchHit {
                    anchor: anchor.encode(),
                    snippet: h.snippet,
                    score: -(h.score as f32),
                }
            })
            .collect();

        let id = self
            .next_stream
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.streams.write().unwrap().insert(id, shell_hits);
        id
    }

    fn search_poll(&self, stream_id: u64) -> Vec<SearchHit> {
        self.streams
            .write()
            .unwrap()
            .get_mut(&stream_id)
            .map(std::mem::take)
            .unwrap_or_default()
    }

    fn search_end(&self, stream_id: u64) {
        self.streams.write().unwrap().remove(&stream_id);
    }

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
                log::info!("bible: install requested for {id}");
                Ok(())
            }
        }
    }

    fn creation_entries(&self) -> Vec<CreationEntry> {
        vec![custom_entry(
            "install-bible",
            "Instalar Biblia",
            notein_sdk::bytes::Bytes::from_static(b"\xf0\x9f\x93\xa5"),
            CreationCategory::Source,
        )]
    }

    fn execute_creation(
        &self,
        entry_id: &str,
        _import_bytes: Option<&[u8]>,
        _name_hint: Option<&str>,
    ) -> Result<ItemCreationResult, CreationError> {
        // Install flow is handled by the UI (translation picker chooses
        // from biblionlabs manifest, importer runs in a background
        // worker). The Source itself does not synthesise items.
        Err(CreationError::Invalid(format!(
            "install flow for {entry_id} is driven by the UI, not Source::execute_creation",
        )))
    }

    fn render_chip(&self, anchor: &[u8]) -> Option<ChipView> {
        let BibleAnchor {
            book_id,
            start_chapter,
            start_verse,
            end_chapter,
            end_verse,
            ..
        } = BibleAnchor::decode(anchor)?;
        let label = if start_chapter == end_chapter && start_verse == end_verse {
            format!("{book_id} {start_chapter}:{start_verse}")
        } else {
            format!("{book_id} {start_chapter}:{start_verse}-{end_chapter}:{end_verse}")
        };
        Some(ChipView {
            label,
            icon: Some("📖".into()),
            tint: None,
            tooltip: None,
        })
    }
}
