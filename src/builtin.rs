//! Built-in plugins compiled into the notinplus binary.
//!
//! Each submodule implements one of the plugin traits
//! ([`notein_plugin::CanvasKind`], [`notein_plugin::Source`]) and
//! [`register_all`] wires them into the shared [`PluginRegistry`] at
//! startup. Dynamic (dlopen) plugins land in a later phase; the trait
//! surface is identical.

use std::sync::Arc;

use notein_plugin::PluginRegistry;

pub mod bible_linked;
pub mod pdf_canvas;
pub mod pdf_linked;
pub mod pdf_source;
pub mod stroke_canvas;

pub use bible_linked::BibleLinkedSource;
pub use pdf_canvas::PdfCanvasPlugin;
pub use pdf_linked::PdfLinkedSource;
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
    // Bible plugin: hard-linked for the custom UI render path (the
    // generic `CanvasKind::render(FrameCtxHandle)` trait method cannot
    // return Freya rsx). Register ahead of the dynamic scan so if the
    // user also dropped the shared library in the plugins dir the
    // duplicate id lands as a warning (not a crash).
    if let Err(err) = notein_bible::install_default(reg) {
        log::warn!("register notein-bible: {err}");
    }
}
