//! Document bodies the shell merges for plugins: the text of a note,
//! kept in the document every device syncs.

use std::sync::{Arc, Mutex};

use notin_plugin::services::TextBody;

use crate::doc::{BodyView, Doc};
use crate::library::{ItemId, bodies};
use crate::sync::open_docs;

/// The body of one document, published as its live copy while held.
pub struct SharedBody {
    item: ItemId,
    doc: Arc<Mutex<Doc>>,
    /// This handle published the live copy and withdraws it on drop.
    published: bool,
    /// The body as this handle's reader last saw it: its next edit is
    /// recorded against this, so text other devices typed meanwhile stays.
    seen: Mutex<Option<BodyView>>,
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
    #[sismo::function("body::open")]
    pub fn open(item: ItemId) -> Arc<Self> {
        if let Some(doc) = open_docs::get(item) {
            sismo::instant!("body::open_live_copy");
            return Arc::new(Self {
                item,
                doc,
                published: false,
                seen: Mutex::new(None),
            });
        }
        let bytes = {
            sismo::scope!("body::read_bytes");
            bodies::read_doc_bytes(item)
        };
        let doc = {
            sismo::scope!("body::from_snapshot");
            bytes
                .and_then(|bytes| Doc::from_snapshot(&bytes).ok())
                .unwrap_or_default()
        };
        let doc = Arc::new(Mutex::new(doc));
        open_docs::register(item, Arc::clone(&doc));
        Arc::new(Self {
            item,
            doc,
            published: true,
            seen: Mutex::new(None),
        })
    }

    fn seen(&self) -> std::sync::MutexGuard<'_, Option<BodyView>> {
        self.seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Something changed on this device: the others see it now, the disk
    /// a moment later.
    #[sismo::function("body::changed_here")]
    fn changed_here(&self) {
        crate::sync::doc_changed(self.item);
        self.write_later();
    }

    /// Save the body once typing pauses, and push what is left.
    #[sismo::function("body::write_later")]
    fn write_later(&self) {
        let (item, doc) = (self.item, Arc::clone(&self.doc));
        crate::deferred::schedule(write_key(item), move || {
            sismo::scope!("body::deferred_write");
            let bytes = {
                sismo::scope!("body::snapshot");
                open_docs::lock(&doc).snapshot_bytes()
            };
            write(item, bytes);
            crate::sync::doc_changed(item);
        });
    }
}

fn write_key(item: ItemId) -> String {
    format!("body:{item}")
}

#[sismo::function("body::write")]
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
    #[sismo::function("body::drop")]
    fn drop(&mut self) {
        {
            sismo::scope!("body::drop_flush");
            crate::deferred::run_now(&write_key(self.item));
        }
        if self.published {
            sismo::scope!("body::drop_unregister");
            open_docs::unregister(self.item, &self.doc);
        }
    }
}

impl TextBody for SharedBody {
    #[sismo::function("body::text")]
    fn text(&self) -> String {
        let view = open_docs::lock(&self.doc).body_view();
        let text = view.text.clone();
        *self.seen() = Some(view);
        text
    }

    #[sismo::function("body::set")]
    fn set(&self, text: &str) -> bool {
        let mut doc = open_docs::lock(&self.doc);
        let mut seen = self.seen();
        let view = match seen.as_ref() {
            Some(base) => doc.edit_body(base, text),
            None => doc.set_body(text).then(|| doc.body_view()),
        };
        let Some(view) = view else {
            sismo::instant!("body::set_unchanged");
            return false;
        };
        *seen = Some(view);
        drop((seen, doc));
        self.changed_here();
        true
    }

    fn changes(&self) -> flume::Receiver<()> {
        open_docs::watch(self.item)
    }
}

impl notin_plugin::services::Records for SharedBody {
    #[sismo::function("body::records")]
    fn entries(&self) -> Vec<(String, Vec<u8>)> {
        open_docs::lock(&self.doc).records()
    }

    #[sismo::function("body::set_record")]
    fn set(&self, key: &str, value: &[u8]) {
        if open_docs::lock(&self.doc).set_record(key, value) {
            self.changed_here();
        }
    }

    #[sismo::function("body::remove_record")]
    fn remove(&self, key: &str) {
        if open_docs::lock(&self.doc).remove_record(key) {
            self.changed_here();
        }
    }

    fn changes(&self) -> flume::Receiver<()> {
        open_docs::watch(self.item)
    }
}
