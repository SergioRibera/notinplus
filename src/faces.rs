//! Registry of document faces — one renderer per plugin id.
//!
//! Every library item names the plugin that owns its content
//! ([`DocKind::plugin_id`]). The shell never branches on that id
//! itself: it asks this registry for the matching [`Face`] and lets it
//! render the pane body, label the card and describe what the item
//! directory carries. A plugin id with no registered face renders a
//! placeholder so a library produced by a build with more plugins
//! never panics here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use freya::icons::lucide;
use freya::prelude::*;

use crate::app::{CanvasMount, CanvasPane};
use crate::library::{DocKind, ItemId, ViewSlot};

/// Label used for items whose plugin is not installed.
pub const UNKNOWN_LABEL: &str = "Documento del plugin";

/// What the shell needs from a plugin to show one of its items.
pub trait Face: Send + Sync {
    /// Id matching [`DocKind::plugin_id`].
    fn plugin_id(&self) -> &'static str;
    /// Short noun for cards and lists ("Lienzo", "PDF", "Nota").
    fn kind_label(&self) -> &'static str;
    /// Icon shown next to the label.
    fn icon(&self) -> Bytes;
    /// Whether strokes can be drawn directly over the face.
    fn supports_ink(&self) -> bool;
    /// Whether the item directory carries a raw PDF attachment.
    fn carries_pdf(&self) -> bool {
        false
    }
    /// Element mounted in a pane for `item`. `slot` names the pane so
    /// faces with their own board keep per-pane state.
    fn render(&self, item: ItemId, slot: Option<ViewSlot>) -> Element;
}

type Registry = Mutex<HashMap<&'static str, Arc<dyn Face>>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(|| {
        let mut map: HashMap<&'static str, Arc<dyn Face>> = HashMap::new();
        for face in builtin::all() {
            map.insert(face.plugin_id(), face);
        }
        Mutex::new(map)
    })
}

fn lock_registry() -> MutexGuard<'static, HashMap<&'static str, Arc<dyn Face>>> {
    match registry().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Publish a face. Call at bootstrap (or from a dynamic plugin loader)
/// for any plugin outside the shipped builtins.
pub fn register(face: Arc<dyn Face>) {
    lock_registry().insert(face.plugin_id(), face);
}

/// Face registered for `plugin_id`, if any.
#[must_use]
pub fn get(plugin_id: &str) -> Option<Arc<dyn Face>> {
    lock_registry().get(plugin_id).map(Arc::clone)
}

/// Card / list label for `kind`.
#[must_use]
pub fn kind_label(kind: &DocKind) -> &'static str {
    get(&kind.plugin_id).map_or(UNKNOWN_LABEL, |f| f.kind_label())
}

/// Icon for `kind`; a generic file icon when the plugin is unknown.
#[must_use]
pub fn icon(kind: &DocKind) -> Bytes {
    get(&kind.plugin_id).map_or_else(lucide::file, |f| f.icon())
}

/// Whether an item of `kind` owns a raw PDF attachment on disk.
#[must_use]
pub fn carries_pdf(kind: &DocKind) -> bool {
    get(&kind.plugin_id).is_some_and(|f| f.carries_pdf())
}

/// Whether strokes can be drawn directly over an item of `kind`.
#[must_use]
pub fn supports_ink(kind: &DocKind) -> bool {
    get(&kind.plugin_id).is_some_and(|f| f.supports_ink())
}

/// Pane body for `item`. Unknown plugins get a placeholder.
#[must_use]
pub fn render(item: ItemId, kind: &DocKind, slot: Option<ViewSlot>) -> Element {
    get(&kind.plugin_id).map_or_else(
        || missing_plugin(&kind.plugin_id),
        |face| face.render(item, slot),
    )
}

fn missing_plugin(plugin_id: &str) -> Element {
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .center()
        .child(
            label()
                .color(Color::from_rgb(200, 200, 210))
                .font_size(14.0)
                .text(format!("Plugin no instalado: {plugin_id}")),
        )
        .into_element()
}

mod builtin {
    use super::{Arc, Bytes, CanvasMount, CanvasPane, Element, Face, ItemId, ViewSlot, lucide};
    use freya::prelude::IntoElement;

    pub(super) fn all() -> [Arc<dyn Face>; 4] {
        [
            Arc::new(StrokeCanvas),
            Arc::new(Pdf),
            Arc::new(Bible),
            Arc::new(Notes),
        ]
    }

    struct StrokeCanvas;

    impl Face for StrokeCanvas {
        fn plugin_id(&self) -> &'static str {
            crate::builtin::stroke_canvas::KIND_ID
        }

        fn kind_label(&self) -> &'static str {
            "Lienzo"
        }

        fn icon(&self) -> Bytes {
            lucide::pencil()
        }

        fn supports_ink(&self) -> bool {
            true
        }

        fn render(&self, item: ItemId, slot: Option<ViewSlot>) -> Element {
            CanvasPane {
                mount: CanvasMount::Item { id: item, slot },
            }
            .into_element()
        }
    }

    struct Pdf;

    impl Face for Pdf {
        fn plugin_id(&self) -> &'static str {
            crate::builtin::pdf_source::KIND_ID
        }

        fn kind_label(&self) -> &'static str {
            "PDF"
        }

        fn icon(&self) -> Bytes {
            lucide::file_text()
        }

        fn supports_ink(&self) -> bool {
            true
        }

        fn carries_pdf(&self) -> bool {
            true
        }

        fn render(&self, item: ItemId, slot: Option<ViewSlot>) -> Element {
            crate::builtin::pdf_face::PdfFace { item, slot }.into_element()
        }
    }

    struct Bible;

    impl Face for Bible {
        fn plugin_id(&self) -> &'static str {
            notein_bible::canvas::KIND_ID
        }

        fn kind_label(&self) -> &'static str {
            "Biblia"
        }

        fn icon(&self) -> Bytes {
            lucide::book_open()
        }

        fn supports_ink(&self) -> bool {
            false
        }

        fn render(&self, item: ItemId, _slot: Option<ViewSlot>) -> Element {
            crate::bible_face::BibleFace { item }.into_element()
        }
    }

    struct Notes;

    impl Face for Notes {
        fn plugin_id(&self) -> &'static str {
            notein_notes::KIND_ID
        }

        fn kind_label(&self) -> &'static str {
            "Nota"
        }

        fn icon(&self) -> Bytes {
            lucide::notebook_text()
        }

        fn supports_ink(&self) -> bool {
            false
        }

        fn render(&self, item: ItemId, _slot: Option<ViewSlot>) -> Element {
            crate::notes_face::NotesFace { item }.into_element()
        }
    }
}
