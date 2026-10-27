//! Document bodies the shell merges for plugins: the text of a note,
//! kept in the document every device syncs.

use std::sync::{Arc, Mutex};

use notein_plugin::services::{TextBody, Typing};

use crate::doc::Doc;
use crate::library::{ItemId, bodies};
use crate::sync::live::LiveMessage;
use crate::sync::open_docs;

/// The body of one document, published as its live copy while held.
pub struct SharedBody {
    item: ItemId,
    doc: Arc<Mutex<Doc>>,
    /// This handle published the live copy and withdraws it on drop.
    published: bool,
}

impl std::fmt::Debug for SharedBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedBody")
            .field("item", &self.item)
            .finish_non_exhaustive()
    }
}

impl SharedBody {
    /// The live copy of `item` when a face holds one, else the stored
    /// document, published as the live copy.
    #[must_use]
    pub fn open(item: ItemId) -> Arc<Self> {
        if let Some(doc) = open_docs::get(item) {
            return Arc::new(Self {
                item,
                doc,
                published: false,
            });
        }
        let doc = bodies::read_doc_bytes(item)
            .and_then(|bytes| Doc::from_snapshot(&bytes).ok())
            .unwrap_or_default();
        let doc = Arc::new(Mutex::new(doc));
        open_docs::register(item, Arc::clone(&doc));
        Arc::new(Self {
            item,
            doc,
            published: true,
        })
    }

    fn write(&self, doc: &Doc) {
        match doc.snapshot_bytes() {
            Ok(bytes) => {
                if let Err(err) = bodies::write_doc_bytes(self.item, &bytes) {
                    log::warn!("body {}: write failed: {err}", self.item);
                }
            }
            Err(err) => log::warn!("body {}: snapshot failed: {err}", self.item),
        }
    }
}

impl Drop for SharedBody {
    fn drop(&mut self) {
        if self.published {
            open_docs::unregister(self.item, &self.doc);
        }
    }
}

impl TextBody for SharedBody {
    fn text(&self) -> String {
        open_docs::lock(&self.doc).body()
    }

    fn set(&self, text: &str) -> bool {
        let mut doc = open_docs::lock(&self.doc);
        if !doc.set_body(text) {
            return false;
        }
        self.write(&doc);
        drop(doc);
        crate::sync::doc_changed(self.item);
        crate::sync::live(
            self.item,
            &LiveMessage::Caret {
                offset: 0,
                typing: true,
            },
        );
        true
    }

    fn changes(&self) -> flume::Receiver<()> {
        open_docs::watch(self.item)
    }

    fn typing(&self) -> flume::Receiver<Typing> {
        crate::sync::live::watch_typing(self.item)
    }
}
