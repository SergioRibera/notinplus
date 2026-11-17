//! Plugin-private anchor payload.
//!
//! `BibleAnchor` is the shape every `SourceRef::Plugin { plugin_id =
//! "io.notein.bible", anchor }` carries. Only this crate decodes it.
//! The on-wire form is the wire codec's; the shell is pure transport.
//!
//! Keep the layout additive: downstream chips, deeplinks and shared
//! bundles may embed older encodings. Future variants should go in
//! `BibleAnchor::V*` enums rather than mutating `V1` fields.

use wincode::{SchemaRead, SchemaWrite};

/// Reference to a single verse or a verse range inside a specific
/// translation.
///
/// `end_verse == start_verse` + `end_chapter == start_chapter` for a
/// single-verse pin. Cross-chapter ranges are allowed but conventionally
/// avoided — the UI collapses "Juan 3:16" and "Juan 3:16-17" the same
/// way, but a range spanning chapters pays a layout cost on scroll.
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct BibleAnchor {
    /// Translation id, e.g. `"spa_rv1960"`. Resolver falls back to the
    /// user's default Bible when the exact id is missing (chip shows a
    /// `⚠` marker in that case).
    pub bible_id: String,
    /// OSIS book code, e.g. `"EPH"` for Ephesians. Fixed small set per
    /// canon so the field is cheap even when embedded in every chip.
    pub book_id: String,
    pub start_chapter: u16,
    pub start_verse: u16,
    pub end_chapter: u16,
    pub end_verse: u16,
    /// Optional character offsets inside the start/end verses. `None`
    /// means "whole verse". Used for highlight-style bookmarks that
    /// span a sub-sentence; chip rendering ignores them.
    pub start_char: Option<u32>,
    pub end_char: Option<u32>,
}

impl BibleAnchor {
    #[must_use]
    pub fn single_verse(
        bible_id: impl Into<String>,
        book_id: impl Into<String>,
        chapter: u16,
        verse: u16,
    ) -> Self {
        Self {
            bible_id: bible_id.into(),
            book_id: book_id.into(),
            start_chapter: chapter,
            start_verse: verse,
            end_chapter: chapter,
            end_verse: verse,
            start_char: None,
            end_char: None,
        }
    }

    /// Bincode-encode for storage inside `SourceRef::Plugin::anchor`.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        notin_sdk::Payload::to_bytes(self)
    }

    /// Decode a payload previously produced by [`Self::encode`]. Returns
    /// `None` on any decode error — callers surface as "broken anchor"
    /// (chip ⚠) rather than propagating a hard failure.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        notin_sdk::decode_bounded(bytes, 64 << 10).ok()
    }
}

/// What a reference points at inside the original Hebrew or Greek of its
/// verses, beyond the verses themselves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, SchemaWrite, SchemaRead)]
pub enum OriginalRef {
    /// One word of the start verse, by its position in the original text.
    Word { position: u16 },
    /// The dictionary entry of the word at `position` of the start verse:
    /// the term itself, wherever it appears. The verse is only where it
    /// was taken from.
    Term { position: u16 },
    /// The verses read word by word in the original language.
    Interlinear,
}

/// The anchor of the Bible kind: verses, and possibly a place in their
/// original text.
///
/// Written as a [`BibleAnchor`] followed, for original text, by a tag and
/// the [`OriginalRef`]. Decoders that only know [`BibleAnchor`] stop before
/// that tail (trailing bytes are not an error), so older readers still see
/// the verses; anchors written before the tail existed decode with
/// `original: None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BibleRef {
    pub passage: BibleAnchor,
    pub original: Option<OriginalRef>,
}

/// Tag before the [`OriginalRef`] tail. Other values are tails written by
/// a newer version: read as no tail.
const ORIGINAL_TAIL: u8 = 1;

impl From<BibleAnchor> for BibleRef {
    fn from(passage: BibleAnchor) -> Self {
        Self {
            passage,
            original: None,
        }
    }
}

impl BibleRef {
    #[must_use]
    pub const fn original(passage: BibleAnchor, original: OriginalRef) -> Self {
        Self {
            passage,
            original: Some(original),
        }
    }
}

// SAFETY: `TYPE_META` stays `Dynamic`, and `size_of` counts exactly what
// `write` writes: the passage, then the tag and the tail when present.
unsafe impl<C: wincode::config::Config> SchemaWrite<C> for BibleRef {
    type Src = Self;

    fn size_of(src: &Self) -> wincode::WriteResult<usize> {
        let mut size = <BibleAnchor as SchemaWrite<C>>::size_of(&src.passage)?;
        if let Some(original) = &src.original {
            size += <u8 as SchemaWrite<C>>::size_of(&ORIGINAL_TAIL)?
                + <OriginalRef as SchemaWrite<C>>::size_of(original)?;
        }
        Ok(size)
    }

    fn write(mut writer: impl wincode::io::Writer, src: &Self) -> wincode::WriteResult<()> {
        <BibleAnchor as SchemaWrite<C>>::write(writer.by_ref(), &src.passage)?;
        if let Some(original) = &src.original {
            <u8 as SchemaWrite<C>>::write(writer.by_ref(), &ORIGINAL_TAIL)?;
            <OriginalRef as SchemaWrite<C>>::write(writer.by_ref(), original)?;
        }
        Ok(())
    }
}

// SAFETY: `TYPE_META` stays `Dynamic`, and `dst` is written once, right
// before returning `Ok`.
unsafe impl<'de, C: wincode::config::Config> SchemaRead<'de, C> for BibleRef {
    type Dst = Self;

    fn read(
        mut reader: impl wincode::io::Reader<'de>,
        dst: &mut core::mem::MaybeUninit<Self>,
    ) -> wincode::ReadResult<()> {
        let passage = <BibleAnchor as SchemaRead<'de, C>>::get(reader.by_ref())?;
        let original = match <u8 as SchemaRead<'de, C>>::get(reader.by_ref()) {
            Ok(ORIGINAL_TAIL) => Some(<OriginalRef as SchemaRead<'de, C>>::get(reader.by_ref())?),
            // The bytes ended (an anchor of verses only), or a tail from a
            // newer version: its verses still read.
            Ok(_) | Err(wincode::ReadError::Io(wincode::io::ReadError::ReadSizeLimit(_))) => None,
            Err(err) => return Err(err),
        };
        dst.write(Self { passage, original });
        Ok(())
    }
}

#[cfg(test)]
mod forged {
    use super::BibleAnchor;

    #[test]
    fn a_forged_length_is_no_anchor() {
        let forged = [253, 255, 255, 255, 255, 255, 255, 255, 255];
        assert!(BibleAnchor::decode(&forged).is_none());
    }
}

#[cfg(test)]
mod original_tail {
    use notin_sdk::Payload;

    use super::{BibleAnchor, BibleRef, OriginalRef};

    fn verse() -> BibleAnchor {
        BibleAnchor::single_verse("spa_rv1960", "MAT", 5, 3)
    }

    #[test]
    fn a_word_reference_round_trips() {
        let word = BibleRef::original(verse(), OriginalRef::Word { position: 0 });
        assert_eq!(BibleRef::from_bytes(&word.to_bytes()), Ok(word));
    }

    #[test]
    fn anchors_written_before_the_tail_read_as_verses() {
        let old = verse().to_bytes();
        assert_eq!(BibleRef::from_bytes(&old), Ok(BibleRef::from(verse())));
    }

    #[test]
    fn readers_of_verses_skip_the_tail() {
        let word = BibleRef::original(verse(), OriginalRef::Term { position: 4 });
        assert_eq!(BibleAnchor::decode(&word.to_bytes()), Some(verse()));
    }

    #[test]
    fn an_unknown_tail_still_reads_its_verses() {
        let mut bytes = verse().to_bytes();
        bytes.extend([9, 1, 2, 3]);
        assert_eq!(BibleRef::from_bytes(&bytes), Ok(BibleRef::from(verse())));
    }
}
