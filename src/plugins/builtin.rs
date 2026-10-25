//! The plugins that ship with Notein: Lienzo, PDF, Nota and Biblia.

pub mod canvas;
pub mod pdf;

use std::sync::{Arc, OnceLock};

use notein_plugin::info::PluginInfo;
use notein_plugin::plugin::{Plugin, Registrar};

use crate::plugins::faces::{BibleFace, CanvasFace, NoteFace, PdfFace};

static CANVAS: OnceLock<Arc<canvas::CanvasKind>> = OnceLock::new();
static PDF: OnceLock<Arc<pdf::PdfKind>> = OnceLock::new();
static NOTES: OnceLock<Arc<notein_notes::NotesKind>> = OnceLock::new();
static BIBLE: OnceLock<Arc<notein_bible::BibleKind>> = OnceLock::new();

/// The canvas kind, for the shell code that writes its summaries.
#[must_use]
pub fn canvas_kind() -> Arc<canvas::CanvasKind> {
    Arc::clone(CANVAS.get_or_init(Arc::default))
}

/// The PDF kind, for import and text extraction.
#[must_use]
pub fn pdf_kind() -> Arc<pdf::PdfKind> {
    Arc::clone(PDF.get_or_init(Arc::default))
}

/// The note kind and its index.
#[must_use]
pub fn notes_kind() -> Arc<notein_notes::NotesKind> {
    Arc::clone(NOTES.get_or_init(Arc::default))
}

/// The Bible kind, when its database could be opened.
#[must_use]
pub fn bible_kind() -> Option<Arc<notein_bible::BibleKind>> {
    // The kind is made while the catalog is built, which may not have
    // happened yet when a screen first asks.
    super::catalog::ensure();
    BIBLE.get().cloned()
}

/// Lienzo: ink canvases, scratch canvases and notebooks.
#[derive(Debug, Default)]
pub struct CanvasPlugin;

impl Plugin for CanvasPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("io.notein.canvas", "Lienzo")
            .description("Escribí y dibujá a mano en un lienzo infinito.")
    }

    fn register(&self, registrar: &mut Registrar) {
        let kind = registrar.kind(canvas_kind());
        registrar.face(CanvasFace, kind);
    }
}

/// PDF: import a PDF and write on its pages.
#[derive(Debug, Default)]
pub struct PdfPlugin;

impl Plugin for PdfPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("io.notein.pdf", "PDF")
            .description("Importá PDFs, escribí encima y buscá en su texto.")
    }

    fn register(&self, registrar: &mut Registrar) {
        let kind = registrar.kind(pdf_kind());
        registrar.face(PdfFace, kind);
    }
}

/// Nota: markdown notes with references.
#[derive(Debug, Default)]
pub struct NotesPlugin;

impl Plugin for NotesPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("io.notein.note", "Notas")
            .description("Notas de texto con títulos, listas y referencias a todo lo demás.")
    }

    fn register(&self, registrar: &mut Registrar) {
        let kind = registrar.kind(notes_kind());
        registrar.face(NoteFace, kind);
    }
}

/// Biblia: read, highlight and cite verses.
#[derive(Debug)]
pub struct BiblePlugin {
    kind: Arc<notein_bible::BibleKind>,
}

impl BiblePlugin {
    /// Open the translations database.
    ///
    /// # Errors
    /// The database cannot be opened.
    pub fn open() -> Result<Self, notein_bible::db::DbError> {
        let db = notein_bible::Db::open_default()?;
        let kind = Arc::new(notein_bible::BibleKind::new(db, notein_bible::new_shared()));
        let _ = BIBLE.set(Arc::clone(&kind));
        Ok(Self { kind })
    }
}

impl Plugin for BiblePlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("io.notein.bible", "Biblia")
            .description("Leé la Biblia, resaltá y anotá versos, y citalos con /ref.")
    }

    fn register(&self, registrar: &mut Registrar) {
        let kind = registrar.kind(Arc::clone(&self.kind));
        registrar.face(BibleFace, kind);
    }
}
