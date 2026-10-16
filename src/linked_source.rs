//! Linked-doc source registry.
//!
//! A [`DocLinked`](crate::library::ItemKind::DocLinked) bundle pairs an
//! arbitrary "source" face (PDF today, Bible / EPUB / whatever
//! tomorrow) with a free-form infinite-canvas notes face. This module
//! owns the registry of source backends: each entry is a
//! [`LinkedDocSource`] implementation addressed by its plugin id.
//!
//! The [`crate::doc_linked`] shell looks up the bundle's declared
//! `source_plugin`, fetches the backend from here, and defers face
//! rendering to [`LinkedDocSource::render_face`]. The backend is free
//! to return any element — no shell-side composition, no required
//! shape.
//!
//! # Patterns
//!
//! The plugin decides whether its face accepts strokes and how.
//!
//! * **Backdrop plugin** (PDF today — see
//!   [`crate::builtin::pdf_linked::PdfFace`]). The plugin provides a
//!   [`freya_canvas_bg::CanvasBackground`] and installs it onto a
//!   shell-owned stroke [`crate::canvas::Board`] via
//!   `set_background`. Mount the board with
//!   [`crate::app::CanvasMount::Item`]; the shell handles strokes
//!   persistence (`doc_path(id)`), autosave, and pan/zoom. Publish an
//!   anchor-ref closure through
//!   [`crate::app::AnchorSourceCtx`] so long-press pins resolve back
//!   to a plugin-specific [`crate::bookmark::SourceRef`].
//!
//! * **Freya-content plugin** (planned Bible reader). The plugin
//!   returns an arbitrary Freya element rendering its own UI
//!   (scrollable text, custom layout, …). To accept strokes on top,
//!   stack a [`crate::app::CanvasPane`] (or a bare
//!   [`crate::app::canvas_body`]) over the content element inside an
//!   absolutely-positioned overlay rect — the stroke board carries
//!   its own state and persists to `doc_path(id)` the same way the
//!   PDF face does. Opt out of strokes by simply not including the
//!   overlay. The plugin is in charge of coordinate alignment between
//!   its underlying UI and the stroke board's world coordinates.
//!
//! Builtins register themselves lazily on first [`get`] access via
//! [`register_builtins`]. External plugins can call [`register`]
//! directly.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use freya::prelude::Element;

use crate::library::ItemId;

/// Shape every source backend implements. The registry is keyed on
/// [`plugin_id`](LinkedDocSource::plugin_id); call sites render the
/// face via [`render_face`](LinkedDocSource::render_face).
pub trait LinkedDocSource: Send + Sync {
    /// Stable plugin id matching
    /// [`crate::library::model::ItemKind::DocLinked::source_plugin`].
    fn plugin_id(&self) -> &'static str;

    /// Render the source-face element for `item`. Returns a Freya
    /// [`Element`] mounted in-place by the DocLinked shell — the
    /// backend owns everything inside its box (chrome, backing store,
    /// autosave, pointer input).
    fn render_face(&self, item: ItemId) -> Element;
}

type Registry = Mutex<HashMap<&'static str, Arc<dyn LinkedDocSource>>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| {
        let reg: Registry = Mutex::new(HashMap::new());
        {
            let mut g = lock_registry(&reg);
            register_builtins(&mut g);
        }
        reg
    })
}

fn lock_registry(r: &Registry) -> MutexGuard<'_, HashMap<&'static str, Arc<dyn LinkedDocSource>>> {
    match r.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

fn register_builtins(g: &mut HashMap<&'static str, Arc<dyn LinkedDocSource>>) {
    let pdf: Arc<dyn LinkedDocSource> = Arc::new(crate::builtin::pdf_linked::PdfLinkedSource);
    g.insert(pdf.plugin_id(), pdf);
    let bible: Arc<dyn LinkedDocSource> =
        Arc::new(crate::builtin::bible_linked::BibleLinkedSource);
    g.insert(bible.plugin_id(), bible);
}

/// Publish a backend. Call this at app bootstrap (or from a dynamic
/// plugin loader) for any source outside the shipped builtins.
pub fn register(source: Arc<dyn LinkedDocSource>) {
    lock_registry(registry()).insert(source.plugin_id(), source);
}

/// Look up a backend by plugin id. Returns `None` when nothing is
/// registered — the shell renders a stub placeholder in that case so
/// a corrupt / stale `source_plugin` never panics.
#[must_use]
pub fn get(plugin_id: &str) -> Option<Arc<dyn LinkedDocSource>> {
    lock_registry(registry()).get(plugin_id).map(Arc::clone)
}
