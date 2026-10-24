//! What a document looks like *while* someone is working on it: ink
//! still under the pen, a pointer moving over the page.
//!
//! Sent as it happens, never stored; the committed change follows through the
//! document itself.

use std::sync::Mutex;

use notein_sync::DeviceId;

use crate::brush::{BrushPreset, InkPoint};
use crate::ids::StrokeId;
use crate::library::ItemId;

/// One ephemeral update of a document.
#[derive(bincode::Encode, bincode::Decode, Clone, PartialEq, Debug)]
pub enum LiveMessage {
    /// Points added to a stroke since the last message. `done` closes
    /// it: the committed stroke arrives through the document.
    Ink {
        stroke: StrokeId,
        preset: BrushPreset,
        color: [u8; 4],
        opacity: f32,
        points: Vec<InkPoint>,
        done: bool,
    },
    /// Where the pen hovers, in world units. `shown: false` hides it.
    Pointer { x: f32, y: f32, shown: bool },
    /// Where the caret sits in a text body, as a character offset, and
    /// whether the author is typing right now.
    Caret { offset: u64, typing: bool },
}

impl LiveMessage {
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::encode_to_vec(self, bincode::config::standard()).unwrap_or_default()
    }

    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        bincode::decode_from_slice(bytes, bincode::config::standard())
            .ok()
            .map(|(message, _)| message)
    }
}

/// A live message as a face receives it.
#[derive(Clone, PartialEq, Debug)]
pub struct LiveEvent {
    pub from: DeviceId,
    pub name: String,
    pub message: LiveMessage,
}

static LISTENERS: Mutex<Vec<(ItemId, flume::Sender<LiveEvent>)>> = Mutex::new(Vec::new());

/// Receive every live message about `item` for as long as the
/// receiver lives.
#[must_use]
pub fn watch(item: ItemId) -> flume::Receiver<LiveEvent> {
    let (tx, rx) = flume::unbounded();
    listeners().push((item, tx));
    rx
}

/// Hand `event` to every face watching `item`.
pub(super) fn dispatch(item: ItemId, event: &LiveEvent) {
    listeners().retain(|(watched, tx)| *watched != item || tx.send(event.clone()).is_ok());
}

fn listeners() -> std::sync::MutexGuard<'static, Vec<(ItemId, flume::Sender<LiveEvent>)>> {
    LISTENERS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
