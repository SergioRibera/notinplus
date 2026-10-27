//! Highlights kept as records the shell merges with every device and
//! person, so two people marking verses at once both keep theirs.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use freya::prelude::*;
use notein_sdk::services::Records;

use crate::state::{BibleBookmark, BibleViewerState};

const CODEC: bincode::config::Configuration = bincode::config::standard();

fn key(highlight: &BibleBookmark) -> String {
    use std::fmt::Write as _;
    highlight.id.0.iter().fold(String::new(), |mut key, byte| {
        let _ = write!(key, "{byte:02x}");
        key
    })
}

/// Every highlight in `records`, in reading order.
pub(super) fn highlights(records: &dyn Records) -> Vec<BibleBookmark> {
    let mut out: Vec<BibleBookmark> = records
        .entries()
        .into_iter()
        .filter_map(|(_, bytes)| {
            bincode::decode_from_slice(&bytes, CODEC)
                .ok()
                .map(|(highlight, _)| highlight)
        })
        .collect();
    out.sort_by(|a, b| {
        (&a.book_id, a.chapter, a.start_verse, a.id.0).cmp(&(
            &b.book_id,
            b.chapter,
            b.start_verse,
            b.id.0,
        ))
    });
    out
}

/// Keys this device last wrote or read, so a highlight only gets
/// removed when it was removed here, never because it just arrived.
#[derive(Default)]
pub(super) struct Synced(RefCell<HashSet<String>>);

impl Synced {
    /// Write what changed in `highlights` since the last call.
    pub(super) fn push(&self, records: &dyn Records, highlights: &[BibleBookmark]) {
        let held: BTreeMap<String, Vec<u8>> = records.entries().into_iter().collect();
        let mut now = HashSet::new();
        for highlight in highlights {
            let key = key(highlight);
            let Ok(bytes) = bincode::encode_to_vec(highlight, CODEC) else {
                continue;
            };
            if held.get(&key) != Some(&bytes) {
                records.set(&key, &bytes);
            }
            now.insert(key);
        }
        let mut synced = self.0.borrow_mut();
        for gone in synced.difference(&now) {
            if held.contains_key(gone) {
                records.remove(gone);
            }
        }
        *synced = now;
    }

    /// Note what arrived, so it is not taken for a local removal.
    fn pulled(&self, highlights: &[BibleBookmark]) {
        *self.0.borrow_mut() = highlights.iter().map(key).collect();
    }
}

/// Keep `state`'s highlights merged with `records` both ways.
pub(super) fn use_shared_highlights(records: &Arc<dyn Records>, state: State<BibleViewerState>) {
    let synced = use_hook(|| Rc::new(Synced::default()));
    use_hook({
        let (records, synced) = (Arc::clone(records), Rc::clone(&synced));
        move || {
            let mut state = state;
            let stored = highlights(&*records);
            if stored.is_empty() {
                // A note from before highlights were records: keep them.
                synced.push(&*records, &state.peek().highlights);
            } else {
                synced.pulled(&stored);
                state.write().highlights = stored;
            }
            let changes = records.changes();
            spawn(async move {
                while changes.recv_async().await.is_ok() {
                    let stored = highlights(&*records);
                    if state.peek().highlights != stored {
                        synced.pulled(&stored);
                        state.write().highlights = stored;
                    }
                }
            });
        }
    });
    use_side_effect({
        let (records, synced) = (Arc::clone(records), Rc::clone(&synced));
        move || synced.push(&*records, &state.read().highlights)
    });
}
