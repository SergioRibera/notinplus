//! Bible source backend for linked-doc bundles.
//!
//! Pairs the Bible reader (`io.notein.bible.viewer`) with a free-form
//! stroke canvas inside an [`crate::library::ItemKind::DocLinked`] item.
//! The DocLinked shell owns the split chrome + the pane-swap buttons;
//! this backend only has to render the Bible face.

use freya::prelude::*;

use crate::library::ItemId;
use crate::linked_source::LinkedDocSource;

/// Stable plugin id. Shared with the standalone Bible page so a
/// `source_plugin` string equal to this routes through the registry
/// here instead of the generic placeholder.
pub const KIND_ID: &str = "io.notein.bible.viewer";

#[derive(Debug, Default, Clone, Copy)]
pub struct BibleLinkedSource;

impl LinkedDocSource for BibleLinkedSource {
    fn plugin_id(&self) -> &'static str {
        KIND_ID
    }

    fn render_face(&self, item: ItemId) -> Element {
        crate::bible_face::BibleFace { item }.into_element()
    }
}
