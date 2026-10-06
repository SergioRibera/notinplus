//! Built-in plugins compiled into the notinplus binary.
//!
//! Each submodule implements one of the plugin traits
//! ([`notein_plugin::CanvasKind`], [`notein_plugin::Source`]) and
//! [`register_all`] wires them into the shared [`PluginRegistry`] at
//! startup. Dynamic (dlopen) plugins land in a later phase; the trait
//! surface is identical.

use std::sync::Arc;

use notein_plugin::PluginRegistry;

pub mod pdf_canvas;
pub mod pdf_source;
pub mod stroke_canvas;

pub use pdf_canvas::PdfCanvasPlugin;
pub use pdf_source::PdfSourcePlugin;
pub use stroke_canvas::StrokeCanvasPlugin;

/// Register every in-process builtin into `reg`. Idempotent: duplicate
/// kind-ids are logged and skipped so a second call never panics.
pub fn register_all(reg: &mut PluginRegistry) {
    if let Err(err) = reg.register_canvas(Arc::new(StrokeCanvasPlugin::default())) {
        log::warn!("register StrokeCanvasPlugin: {err}");
    }
    if let Err(err) = reg.register_canvas(Arc::new(PdfCanvasPlugin::default())) {
        log::warn!("register PdfCanvasPlugin: {err}");
    }
    if let Err(err) = reg.register_source(Arc::new(PdfSourcePlugin::default())) {
        log::warn!("register PdfSourcePlugin: {err}");
    }
}
