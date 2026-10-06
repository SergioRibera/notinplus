//! Built-in PDF-backed canvas plugin (Modo A legacy).
//!
//! Owns the `CanvasKind` surface for items persisted as
//! [`crate::library::ItemKind::PdfCanvas`] — a stroke canvas whose
//! background is a rasterised PDF. Lifecycle-only for now (render +
//! input stay with the legacy pipeline in [`crate::canvas`]); registry
//! presence is what Phase 2.7 needs so future `ItemKind::Canvas { plugin_id
//! = "io.notein.canvas.pdf-bg" }` resolution finds a plugin.
//!
//! Creation of PDF-backed items goes through
//! [`crate::builtin::pdf_source::PdfSourcePlugin`] + the shell's import
//! flow — this plugin exposes no creation entries.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use bincode::config::standard;
use notein_plugin::{
    Bookmark, BookmarkId, CanvasError, CanvasKind, ConfigError, ConfigSchema, ConfigValue,
    CreationEntry, CreationError, FrameCtxHandle, InputEvent, ItemCreationResult, WorldPoint,
};

use crate::doc::Doc;

/// Stable id this plugin registers under. Referenced by
/// `ItemKind::Canvas { plugin_id }` when the schema completes its
/// migration out of the hard-coded `PdfCanvas` variant.
pub const KIND_ID: &str = "io.notein.canvas.pdf-bg";

/// Default [`CanvasKind`] implementation for PDF-backed canvases.
#[derive(Debug, Default)]
pub struct PdfCanvasPlugin {
    instances: Mutex<HashMap<u64, Doc>>,
    next_instance: AtomicU64,
}

impl PdfCanvasPlugin {
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

impl CanvasKind for PdfCanvasPlugin {
    fn kind_id(&self) -> &str {
        KIND_ID
    }

    fn kind_label(&self) -> &str {
        "Lienzo PDF"
    }

    fn capabilities(&self) -> &[&'static str] {
        const CAPS: &[&str] = &["bookmarks", "strokes", "layers", "pdf-background"];
        CAPS
    }

    fn open(&self, item_bytes: &[u8]) -> Result<u64, CanvasError> {
        let doc = if item_bytes.is_empty() {
            Doc::default()
        } else {
            match bincode::decode_from_slice::<Doc, _>(item_bytes, standard()) {
                Ok((doc, _)) => doc,
                Err(err) => return Err(CanvasError::OpenFailed(err.to_string())),
            }
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
        bincode::encode_to_vec(doc, standard()).unwrap_or_default()
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
        Vec::new()
    }

    fn execute_creation(
        &self,
        entry_id: &str,
        _import_bytes: Option<&[u8]>,
        _name_hint: Option<&str>,
    ) -> Result<ItemCreationResult, CreationError> {
        Err(CreationError::UnknownEntry(entry_id.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_creation_entries_exposed() {
        let plugin = PdfCanvasPlugin::default();
        assert!(plugin.creation_entries().is_empty());
    }

    #[test]
    fn open_blank_bytes_yields_default_doc() {
        let plugin = PdfCanvasPlugin::default();
        let id = plugin.open(&[]).expect("open blank");
        let bytes = plugin.serialize_snapshot(id);
        assert!(!bytes.is_empty());
        plugin.close(id);
    }

    #[test]
    fn open_rejects_corrupt_bytes() {
        let plugin = PdfCanvasPlugin::default();
        let err = plugin.open(&[0xff, 0xff]).expect_err("should fail");
        assert!(matches!(err, CanvasError::OpenFailed(_)));
    }

    #[test]
    fn capabilities_include_pdf_background() {
        let plugin = PdfCanvasPlugin::default();
        assert!(plugin.capabilities().contains(&"pdf-background"));
    }

    #[test]
    fn execute_creation_always_rejects() {
        let plugin = PdfCanvasPlugin::default();
        let err = plugin
            .execute_creation("new-pdf", None, None)
            .expect_err("should fail");
        assert!(matches!(err, CreationError::UnknownEntry(_)));
    }
}
