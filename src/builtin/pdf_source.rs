//! Built-in PDF source plugin.
//!
//! Lifecycle + creation entry only — the legacy `freya-pdf` pipeline
//! keeps serving actual page rasterisation inside Modo A canvases. The
//! plugin exists so the home FAB `+` menu, the settings panel, and the
//! future `SourceRef::Plugin { plugin_id: "io.notein.source.pdf", .. }`
//! resolver all route through one declarative surface.
//!
//! `render_tile` / `hit_test` / `resolve_anchor` / `search_*` stay as
//! honest stubs — Phase 5 (Modo B) wires them to [`freya_pdf`] when the
//! source pane needs to render independently of the stroke canvas.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use freya::icons::lucide;
use freya_pdf::PdfDocument;
use notein_plugin::{
    AnchorView, ConfigError, ConfigField, ConfigFieldKind, ConfigSchema, ConfigValue,
    CreationCategory, CreationEntry, CreationError, CreationKind, ItemCreationResult, SearchHit,
    Selection, Source, SourceError, Tile, TileRequest,
};

/// Stable id this plugin registers under. Referenced by future
/// `SourceRef::Plugin` anchors.
pub const KIND_ID: &str = "io.notein.source.pdf";

/// Shell-wide creation entry id. Preserved from the old
/// `shell_builtins::IMPORT_PDF` constant so persisted FAB shortcuts
/// keep resolving after the migration.
pub const IMPORT_PDF: &str = "import-pdf";

/// Modo B counterpart to [`IMPORT_PDF`] — imports the PDF as a
/// standalone source item and spawns a sibling canvas item linked to
/// it. Handled by the shell's `flow_import_pdf_linked` flow.
pub const IMPORT_PDF_LINKED: &str = "import-pdf-linked";

/// Default render DPI. Matches the legacy `freya-pdf` setting.
const DEFAULT_RENDER_DPI: f64 = 150.0;
const MIN_RENDER_DPI: f64 = 72.0;
const MAX_RENDER_DPI: f64 = 600.0;

const CONFIG_KEY_RENDER_DPI: &str = "render_dpi";

/// Default [`Source`] implementation for PDF files.
#[derive(Debug, Default)]
pub struct PdfSourcePlugin {
    instances: Mutex<HashMap<u64, PdfInstance>>,
    next_instance: AtomicU64,
    config: Mutex<PdfConfig>,
}

#[derive(Debug)]
struct PdfInstance {
    doc: PdfDocument,
}

#[derive(Debug, Clone, Copy)]
struct PdfConfig {
    render_dpi: f64,
}

impl Default for PdfConfig {
    fn default() -> Self {
        Self {
            render_dpi: DEFAULT_RENDER_DPI,
        }
    }
}

impl PdfSourcePlugin {
    fn alloc_id(&self) -> u64 {
        self.next_instance
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
    }

    fn lock_instances(&self) -> std::sync::MutexGuard<'_, HashMap<u64, PdfInstance>> {
        match self.instances.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    fn lock_config(&self) -> std::sync::MutexGuard<'_, PdfConfig> {
        match self.config.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }
}

impl Source for PdfSourcePlugin {
    fn kind_id(&self) -> &str {
        KIND_ID
    }

    fn kind_label(&self) -> &str {
        "PDF"
    }

    fn file_extensions(&self) -> &[&'static str] {
        const EXTS: &[&str] = &["pdf"];
        EXTS
    }

    fn open(&self, item_bytes: &[u8]) -> Result<u64, SourceError> {
        if item_bytes.is_empty() {
            return Err(SourceError::OpenFailed("empty pdf bytes".to_owned()));
        }
        let doc = PdfDocument::open_bytes(item_bytes.to_vec())
            .map_err(|e| SourceError::OpenFailed(e.to_string()))?;
        let id = self.alloc_id();
        self.lock_instances().insert(id, PdfInstance { doc });
        Ok(id)
    }

    fn close(&self, instance: u64) {
        self.lock_instances().remove(&instance);
    }

    fn title(&self, _instance: u64) -> String {
        // Pdfium exposes a document title metadata field but Modo A
        // owns the display name on the Item itself. Return a stable
        // placeholder; the shell renders the Item title instead.
        "PDF".to_owned()
    }

    fn content_size(&self, instance: u64) -> (f32, f32) {
        let guard = self.lock_instances();
        let Some(inst) = guard.get(&instance) else {
            return (0.0, 0.0);
        };
        if inst.doc.page_count() == 0 {
            return (0.0, 0.0);
        }
        inst.doc.page_size(0).unwrap_or((0.0, 0.0))
    }

    fn render_tile(&self, _instance: u64, _req: TileRequest) -> Result<Tile, SourceError> {
        // Phase 2.7 keeps the legacy `freya-pdf` background renderer in
        // charge of the pane. Phase 5 (Modo B) wires this to
        // `freya_pdf::RenderPool` when the source lives in its own pane.
        Err(SourceError::RenderFailed(
            "pdf render_tile stub; served by legacy pipeline".to_owned(),
        ))
    }

    fn hit_test(&self, _instance: u64, _x: f32, _y: f32) -> Option<Vec<u8>> {
        None
    }

    fn resolve_anchor(&self, _instance: u64, _anchor: &[u8]) -> Option<AnchorView> {
        None
    }

    fn current_selection(&self, _instance: u64) -> Option<Selection> {
        None
    }

    fn search_begin(&self, _instance: u64, _query: &str) -> u64 {
        0
    }

    fn search_poll(&self, _stream_id: u64) -> Vec<SearchHit> {
        Vec::new()
    }

    fn search_end(&self, _stream_id: u64) {}

    fn config_schema(&self) -> ConfigSchema {
        ConfigSchema {
            version: 1,
            fields: vec![ConfigField {
                key: CONFIG_KEY_RENDER_DPI.to_owned(),
                label: "Resolución de render".to_owned(),
                help: "DPI de rasterizado de páginas PDF (72–600).".to_owned(),
                kind: ConfigFieldKind::Float {
                    min: MIN_RENDER_DPI,
                    max: MAX_RENDER_DPI,
                    step: 1.0,
                },
                default: ConfigValue::Float(DEFAULT_RENDER_DPI),
                aliases: Vec::new(),
            }],
        }
    }

    fn get_config(&self, key: &str) -> Option<ConfigValue> {
        match key {
            CONFIG_KEY_RENDER_DPI => Some(ConfigValue::Float(self.lock_config().render_dpi)),
            _ => None,
        }
    }

    fn set_config(&self, key: &str, value: ConfigValue) -> Result<(), ConfigError> {
        match (key, value) {
            (CONFIG_KEY_RENDER_DPI, ConfigValue::Float(v)) => {
                if !v.is_finite() || !(MIN_RENDER_DPI..=MAX_RENDER_DPI).contains(&v) {
                    return Err(ConfigError::OutOfRange);
                }
                self.lock_config().render_dpi = v;
                Ok(())
            }
            (CONFIG_KEY_RENDER_DPI, _) => Err(ConfigError::TypeMismatch),
            (other, _) => Err(ConfigError::UnknownKey(other.to_owned())),
        }
    }

    fn creation_entries(&self) -> Vec<CreationEntry> {
        vec![
            CreationEntry {
                id: IMPORT_PDF.to_owned(),
                label: "Importar PDF".to_owned(),
                icon: lucide::file_text(),
                category: CreationCategory::Source,
                kind: CreationKind::Import {
                    dialog_title: "Elegí un PDF".to_owned(),
                    extensions: vec!["pdf".to_owned()],
                    mime_types: vec!["application/pdf".to_owned()],
                },
            },
            CreationEntry {
                id: IMPORT_PDF_LINKED.to_owned(),
                label: "Importar PDF ligado".to_owned(),
                icon: lucide::file_symlink(),
                category: CreationCategory::Source,
                kind: CreationKind::Import {
                    dialog_title: "Elegí un PDF para ligar a un lienzo".to_owned(),
                    extensions: vec!["pdf".to_owned()],
                    mime_types: vec!["application/pdf".to_owned()],
                },
            },
        ]
    }

    fn execute_creation(
        &self,
        entry_id: &str,
        import_bytes: Option<&[u8]>,
        name_hint: Option<&str>,
    ) -> Result<ItemCreationResult, CreationError> {
        if entry_id != IMPORT_PDF && entry_id != IMPORT_PDF_LINKED {
            return Err(CreationError::UnknownEntry(entry_id.to_owned()));
        }
        let bytes = import_bytes.ok_or(CreationError::MissingImportBytes)?;
        if bytes.is_empty() {
            return Err(CreationError::Invalid("empty pdf bytes".to_owned()));
        }
        // Validate the file parses before handing it back to the shell.
        PdfDocument::open_bytes(bytes.to_vec())
            .map_err(|e| CreationError::BackendFailed(e.to_string()))?;
        let suggested_name = name_hint.map_or_else(|| "Documento PDF".to_owned(), str::to_owned);
        Ok(ItemCreationResult {
            item_bytes: bytes.to_vec(),
            suggested_name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_entries_expose_import_pdf() {
        let plugin = PdfSourcePlugin::default();
        let ids: Vec<String> = plugin
            .creation_entries()
            .into_iter()
            .map(|e| e.id)
            .collect();
        assert!(ids.iter().any(|id| id == IMPORT_PDF));
    }

    #[test]
    fn open_rejects_empty_bytes() {
        let plugin = PdfSourcePlugin::default();
        assert!(matches!(plugin.open(&[]), Err(SourceError::OpenFailed(_))));
    }

    #[test]
    fn execute_creation_unknown_entry_errors() {
        let plugin = PdfSourcePlugin::default();
        let err = plugin
            .execute_creation("not-import", None, None)
            .expect_err("should fail");
        assert!(matches!(err, CreationError::UnknownEntry(_)));
    }

    #[test]
    fn execute_creation_requires_bytes() {
        let plugin = PdfSourcePlugin::default();
        let err = plugin
            .execute_creation(IMPORT_PDF, None, None)
            .expect_err("should fail");
        assert!(matches!(err, CreationError::MissingImportBytes));
    }

    #[test]
    fn file_extensions_lists_pdf() {
        let plugin = PdfSourcePlugin::default();
        assert_eq!(plugin.file_extensions(), &["pdf"]);
    }

    #[test]
    fn config_round_trip_render_dpi() {
        let plugin = PdfSourcePlugin::default();
        plugin
            .set_config(CONFIG_KEY_RENDER_DPI, ConfigValue::Float(200.0))
            .expect("set");
        match plugin.get_config(CONFIG_KEY_RENDER_DPI) {
            Some(ConfigValue::Float(v)) => assert!((v - 200.0).abs() < f64::EPSILON),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn config_rejects_out_of_range() {
        let plugin = PdfSourcePlugin::default();
        let err = plugin
            .set_config(CONFIG_KEY_RENDER_DPI, ConfigValue::Float(10.0))
            .expect_err("too low");
        assert!(matches!(err, ConfigError::OutOfRange));
    }
}
