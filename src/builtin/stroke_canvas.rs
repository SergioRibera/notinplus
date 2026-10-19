//! Default stroke canvas, surfaced as a [`CanvasKind`] plugin.
//!
//! Owns the plugin-facing responsibilities: declaring the three canvas
//! creation entries the home FAB has shipped since v1, exposing the
//! settings schema (empty for now), and tracking opened instance ids.
//!
//! Render, input, and bookmark storage still flow through the legacy
//! [`crate::canvas::Board::shared`] pipeline — those move behind the
//! registry in a later phase. The instance table here keeps a decoded
//! [`Doc`] per `open` call so snapshot serialization and future routing
//! have real state to attach to.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use freya::icons::lucide;
use notein_plugin::{
    Bookmark, BookmarkId, CanvasError, CanvasKind, ConfigError, ConfigSchema, ConfigValue,
    CreationCategory, CreationEntry, CreationError, CreationKind, FrameCtxHandle, InputEvent,
    ItemCreationResult, WorldPoint,
};

use crate::doc::Doc;

/// Stable id this plugin registers under. Referenced by persisted FAB
/// shortcuts and by `DocKind::plugin_id`.
pub const KIND_ID: &str = "io.notein.canvas.stroke";

/// Creation entry ids. Kept identical to the historical shell builtins
/// so persisted shortcuts (modulo the plugin-id migration in
/// [`crate::plugin_settings`]) keep resolving.
pub const NEW_CANVAS: &str = "new-canvas";
pub const NEW_EPHEMERAL: &str = "new-ephemeral";
pub const QUICK_SKETCH: &str = "quick-sketch";

/// Default [`CanvasKind`] implementation. Lifecycle-only for now —
/// `render`, `on_input`, and the bookmark trait methods are stubs while
/// the legacy canvas pipeline keeps serving the actual pane.
#[derive(Debug, Default)]
pub struct StrokeCanvasPlugin {
    instances: Mutex<HashMap<u64, Doc>>,
    next_instance: AtomicU64,
}

impl StrokeCanvasPlugin {
    fn alloc_id(&self) -> u64 {
        self.next_instance
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    fn lock_instances(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Doc>> {
        match self.instances.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }
}

impl CanvasKind for StrokeCanvasPlugin {
    fn kind_id(&self) -> &str {
        KIND_ID
    }

    fn kind_label(&self) -> &str {
        "Lienzo"
    }

    fn capabilities(&self) -> &[&'static str] {
        const CAPS: &[&str] = &["bookmarks", "strokes", "layers"];
        CAPS
    }

    fn open(&self, item_bytes: &[u8]) -> Result<u64, CanvasError> {
        let doc = if item_bytes.is_empty() {
            Doc::default()
        } else {
            Doc::from_snapshot(item_bytes)
                .map_err(|err| CanvasError::OpenFailed(err.to_string()))?
        };
        let id = self.alloc_id();
        self.lock_instances().insert(id, doc);
        Ok(id)
    }

    fn close(&self, instance: u64) {
        self.lock_instances().remove(&instance);
    }

    fn render(&self, _instance: u64, _ctx: FrameCtxHandle) {}

    fn on_input(&self, _instance: u64, _event: InputEvent) {}

    fn create_bookmark(&self, _instance: u64, _at: WorldPoint) -> Option<BookmarkId> {
        None
    }

    fn list_bookmarks(&self, _instance: u64) -> Vec<Bookmark> {
        Vec::new()
    }

    fn update_bookmark(&self, _instance: u64, _bookmark: Bookmark) {}

    fn delete_bookmark(&self, _instance: u64, _id: BookmarkId) {}

    fn navigate_to_bookmark(&self, _instance: u64, _id: BookmarkId) {}

    fn serialize_snapshot(&self, instance: u64) -> Vec<u8> {
        let guard = self.lock_instances();
        let Some(doc) = guard.get(&instance) else {
            return Vec::new();
        };
        doc.snapshot_bytes().unwrap_or_default()
    }

    fn apply_remote_op(&self, _instance: u64, _op: &[u8]) {}

    fn config_schema(&self) -> ConfigSchema {
        ConfigSchema::empty()
    }

    fn get_config(&self, _key: &str) -> Option<ConfigValue> {
        None
    }

    fn set_config(&self, key: &str, _value: ConfigValue) -> Result<(), ConfigError> {
        Err(ConfigError::UnknownKey(key.to_owned()))
    }

    fn creation_entries(&self) -> Vec<CreationEntry> {
        vec![
            CreationEntry {
                id: NEW_CANVAS.to_owned(),
                label: "Lienzo infinito".to_owned(),
                icon: lucide::square_pen(),
                category: CreationCategory::Canvas,
                kind: CreationKind::Blank { prompt_name: true },
            },
            CreationEntry {
                id: NEW_EPHEMERAL.to_owned(),
                label: "Lienzo incógnito".to_owned(),
                icon: lucide::hat_glasses(),
                category: CreationCategory::Canvas,
                kind: CreationKind::Blank { prompt_name: false },
            },
            CreationEntry {
                id: QUICK_SKETCH.to_owned(),
                label: "Borrador rápido".to_owned(),
                icon: lucide::pencil(),
                category: CreationCategory::Canvas,
                kind: CreationKind::Blank { prompt_name: false },
            },
        ]
    }

    fn execute_creation(
        &self,
        entry_id: &str,
        _import_bytes: Option<&[u8]>,
        name_hint: Option<&str>,
    ) -> Result<ItemCreationResult, CreationError> {
        let default_name = match entry_id {
            NEW_CANVAS => "Lienzo infinito",
            NEW_EPHEMERAL => "Lienzo incógnito",
            QUICK_SKETCH => "Borrador rápido",
            other => return Err(CreationError::UnknownEntry(other.to_owned())),
        };
        let suggested_name = name_hint.map_or_else(|| default_name.to_owned(), str::to_owned);
        let item_bytes = Doc::default()
            .snapshot_bytes()
            .map_err(|e| CreationError::BackendFailed(e.to_string()))?;
        Ok(ItemCreationResult {
            item_bytes,
            suggested_name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_entries_match_shell_builtin_ids() {
        let plugin = StrokeCanvasPlugin::default();
        let entries = plugin.creation_entries();
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&NEW_CANVAS));
        assert!(ids.contains(&NEW_EPHEMERAL));
        assert!(ids.contains(&QUICK_SKETCH));
    }

    #[test]
    fn open_empty_bytes_yields_default_doc() {
        let plugin = StrokeCanvasPlugin::default();
        let id = plugin.open(&[]).expect("open blank");
        let bytes = plugin.serialize_snapshot(id);
        assert!(!bytes.is_empty(), "snapshot must round-trip");
        plugin.close(id);
    }

    #[test]
    fn execute_creation_produces_decodable_doc() {
        let plugin = StrokeCanvasPlugin::default();
        let result = plugin
            .execute_creation(NEW_CANVAS, None, None)
            .expect("execute");
        assert_eq!(result.suggested_name, "Lienzo infinito");
        Doc::from_snapshot(&result.item_bytes).expect("decode");
    }

    #[test]
    fn execute_creation_unknown_entry_errors() {
        let plugin = StrokeCanvasPlugin::default();
        let err = plugin
            .execute_creation("nope", None, None)
            .expect_err("unknown");
        assert!(matches!(err, CreationError::UnknownEntry(_)));
    }
}
