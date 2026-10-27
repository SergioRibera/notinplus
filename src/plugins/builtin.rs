//! The plugins that ship with Notein and live in the shell: Lienzo and
//! PDF. Nota and Biblia ship from their own crates.

pub mod canvas;
pub mod pdf;

use std::sync::{Arc, OnceLock};

use notein_plugin::info::PluginInfo;
use notein_plugin::plugin::{Plugin, Registrar};

use crate::plugins::faces::{CanvasFace, PdfFace};

static CANVAS: OnceLock<Arc<canvas::CanvasKind>> = OnceLock::new();
static PDF: OnceLock<Arc<pdf::PdfKind>> = OnceLock::new();

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
