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

    /// Something changed on this device: the others see it now, the disk
    /// a moment later.
    fn changed_here(&self) {
        crate::sync::doc_changed(self.item);
        self.write_later();
    }

    /// Save the body once typing pauses, and push what is left.
    fn write_later(&self) {
        let (item, doc) = (self.item, Arc::clone(&self.doc));
        crate::deferred::schedule(write_key(item), move || {
            let bytes = open_docs::lock(&doc).snapshot_bytes();
            write(item, bytes);
            crate::sync::doc_changed(item);
        });
    }
}

fn write_key(item: ItemId) -> String {
    format!("body:{item}")
}

fn write(item: ItemId, bytes: Result<Vec<u8>, crate::doc::DocError>) {
    match bytes {
        Ok(bytes) => {
            if let Err(err) = bodies::write_doc_bytes(item, &bytes) {
                log::warn!("body {item}: write failed: {err}");
            }
        }
        Err(err) => log::warn!("body {item}: snapshot failed: {err}"),
    }
}

impl Drop for SharedBody {
    fn drop(&mut self) {
        crate::deferred::run_now(&write_key(self.item));
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
        drop(doc);
        self.changed_here();
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

impl notein_plugin::services::Records for SharedBody {
    fn entries(&self) -> Vec<(String, Vec<u8>)> {
        open_docs::lock(&self.doc).records()
    }

    fn set(&self, key: &str, value: &[u8]) {
        if open_docs::lock(&self.doc).set_record(key, value) {
            self.changed_here();
        }
    }

    fn remove(&self, key: &str) {
        if open_docs::lock(&self.doc).remove_record(key) {
            self.changed_here();
        }
    }

    fn changes(&self) -> flume::Receiver<()> {
        open_docs::watch(self.item)
    }
}
