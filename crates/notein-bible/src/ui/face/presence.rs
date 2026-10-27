//! Where other people read: the verses they select show in their color
//! with their name, live.

use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;

use freya::prelude::*;
use notein_sdk::services::{Peer, Presence};

use super::Reader;
use super::highlight::Gesture;
use crate::ui::reader::{Other, Verses};

/// This reader's place: chapter, how many verses, the verses, then the
/// book id, all little endian.
fn encode(book: &str, chapter: u16, verses: &Verses) -> Vec<u8> {
    let count = u16::try_from(verses.len()).unwrap_or(u16::MAX);
    let mut out = Vec::with_capacity(4 + verses.len() * 2 + book.len());
    out.extend_from_slice(&chapter.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    for verse in verses.iter().take(usize::from(count)) {
        out.extend_from_slice(&verse.to_le_bytes());
    }
    out.extend_from_slice(book.as_bytes());
    out
}

/// Book, chapter and verses from what [`encode`] made.
fn decode(bytes: &[u8]) -> Option<(String, u16, BTreeSet<u16>)> {
    let word = |at: usize| -> Option<u16> {
        Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
    };
    let chapter = word(0)?;
    let count = usize::from(word(2)?);
    let verses = (0..count).map(|i| word(4 + i * 2)).collect::<Option<_>>()?;
    let book = std::str::from_utf8(bytes.get(4 + count * 2..)?)
        .ok()?
        .to_owned();
    Some((book, chapter, verses))
}

/// Publish this reader's place and return the others in its chapter.
pub(super) fn use_others(reader: &Reader, gesture: Gesture) -> Rc<Vec<Other>> {
    let presence = use_hook({
        let (host, doc) = (Arc::clone(&reader.host), reader.doc);
        move || host.presence(doc)
    });
    let peers = use_state({
        let presence = Arc::clone(&presence);
        move || presence.peers()
    });
    use_hook({
        let presence = Arc::clone(&presence);
        move || follow(presence, peers)
    });
    use_side_effect({
        let (presence, reader) = (Arc::clone(&presence), reader.clone());
        move || {
            let verses = gesture.selected(&reader);
            let st = reader.state.read();
            presence.publish(encode(&st.current_book, st.current_chapter, &verses));
        }
    });
    let st = reader.state.read();
    let others = peers
        .read()
        .iter()
        .filter_map(|peer| {
            let (book, chapter, verses) = decode(&peer.state)?;
            let here = book == st.current_book && chapter == st.current_chapter;
            (here && !verses.is_empty()).then(|| {
                let [r, g, b] = peer.color;
                Other {
                    verses: Rc::new(verses),
                    color: Color::from_rgb(r, g, b),
                    name: peer.name.clone(),
                }
            })
        })
        .collect();
    Rc::new(others)
}

fn follow(presence: Arc<dyn Presence>, mut peers: State<Vec<Peer>>) {
    let changes = presence.changes();
    spawn(async move {
        while changes.recv_async().await.is_ok() {
            peers.set(presence.peers());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn places_round_trip() {
        let verses: Verses = Rc::new([3, 4, 16].into_iter().collect());
        let (book, chapter, back) = decode(&encode("JHN", 3, &verses)).expect("place");
        assert_eq!((book.as_str(), chapter), ("JHN", 3));
        assert_eq!(back, *verses);
        assert!(decode(&[1]).is_none());
    }
}
