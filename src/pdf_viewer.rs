//! PDF anchor types shared across the shell.
//!
//! Chip strips, bookmark panels and the canvas long-press handler all
//! encode / decode a page index + source item id against this module;
//! the actual viewer now mounts via [`crate::app::canvas_root`] with
//! [`crate::app::CanvasMount::PdfBacked`] so stroke, highlight and pin
//! input land on the same `Board` as every other canvas.

use bincode::config::{self, Configuration};

use crate::builtin::pdf_source::KIND_ID as PDF_SOURCE_KIND_ID;
use crate::library::ItemId;

const CODEC: Configuration = config::standard();

/// Opaque PDF anchor payload stored in
/// [`crate::bookmark::SourceRef::Plugin::anchor`]. Encodes the backing
/// source item id + page index so chips can resolve back to the right
/// viewer + page without the shell needing to know the plugin's schema.
///
/// Two shapes:
/// - [`PdfAnchor::Page`] — targets a whole page (long-press pins created
///   from the Modo A overlay).
/// - [`PdfAnchor::Range`] — targets a UTF-8 char range inside a page
///   (selection-driven bookmarks from Modo B viewer). `start_char` /
///   `end_char` index into [`freya_pdf::PdfDocument::extract_page_text`].
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PdfAnchor {
    Page {
        source: ItemId,
        page: u32,
    },
    Range {
        source: ItemId,
        page: u32,
        start_char: u32,
        end_char: u32,
    },
}

impl PdfAnchor {
    #[must_use]
    pub const fn source(&self) -> ItemId {
        match self {
            Self::Page { source, .. } | Self::Range { source, .. } => *source,
        }
    }

    #[must_use]
    pub const fn page(&self) -> u32 {
        match self {
            Self::Page { page, .. } | Self::Range { page, .. } => *page,
        }
    }

    /// `Some((start, end))` for [`PdfAnchor::Range`], `None` for whole-page.
    #[must_use]
    pub const fn char_range(&self) -> Option<(u32, u32)> {
        match self {
            Self::Range {
                start_char,
                end_char,
                ..
            } => Some((*start_char, *end_char)),
            Self::Page { .. } => None,
        }
    }
}

/// Bincode the anchor into the `Vec<u8>` payload of a
/// [`crate::bookmark::SourceRef::Plugin`].
#[must_use]
pub fn encode_pdf_anchor(anchor: PdfAnchor) -> Vec<u8> {
    bincode::encode_to_vec(anchor, CODEC).unwrap_or_default()
}

/// Reverse of [`encode_pdf_anchor`]. Returns `None` on decode error so
/// a corrupt / stale payload gracefully renders as an unresolved chip
/// instead of panicking the shell.
#[must_use]
pub fn decode_pdf_anchor(bytes: &[u8]) -> Option<PdfAnchor> {
    bincode::decode_from_slice::<PdfAnchor, _>(bytes, CODEC)
        .ok()
        .map(|(a, _)| a)
}

/// Plugin id matching [`crate::bookmark::SourceRef::Plugin::plugin_id`]
/// for pdf-backed anchors. Re-exported so chip renderers can match
/// without pulling in `builtin::pdf_source`.
pub const PDF_PLUGIN_ID: &str = PDF_SOURCE_KIND_ID;

/// Live text selection inside a PDF source instance.
///
/// Indices are UTF-8 char positions into
/// [`freya_pdf::PdfDocument::extract_page_text`]. A cross-page selection
/// is represented by `start_page != end_page`; most selections stay on a
/// single page. Normalise with [`PdfSelection::normalized`] before use.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PdfSelection {
    pub start_page: u32,
    pub start_char: u32,
    pub end_page: u32,
    pub end_char: u32,
}

impl PdfSelection {
    /// Returns `(start, end)` ordered so start precedes end in document
    /// order. Normalisation lets the viewer drag either direction.
    #[must_use]
    pub const fn normalized(self) -> Self {
        let swap = self.start_page > self.end_page
            || (self.start_page == self.end_page && self.start_char > self.end_char);
        if swap {
            Self {
                start_page: self.end_page,
                start_char: self.end_char,
                end_page: self.start_page,
                end_char: self.start_char,
            }
        } else {
            self
        }
    }

    /// `true` when start == end (i.e. a caret, not a range).
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start_page == self.end_page && self.start_char == self.end_char
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_page_anchor_roundtrips_through_bincode() {
        let a = PdfAnchor::Page {
            source: ItemId::new_v4(),
            page: 7,
        };
        let bytes = encode_pdf_anchor(a);
        let back = decode_pdf_anchor(&bytes).expect("decode");
        assert_eq!(back, a);
        assert_eq!(back.page(), 7);
        assert!(back.char_range().is_none());
    }

    #[test]
    fn pdf_range_anchor_roundtrips_through_bincode() {
        let a = PdfAnchor::Range {
            source: ItemId::new_v4(),
            page: 2,
            start_char: 48,
            end_char: 125,
        };
        let bytes = encode_pdf_anchor(a);
        let back = decode_pdf_anchor(&bytes).expect("decode");
        assert_eq!(back, a);
        assert_eq!(back.char_range(), Some((48, 125)));
    }

    #[test]
    fn decode_pdf_anchor_rejects_empty() {
        assert!(decode_pdf_anchor(&[]).is_none());
    }

    #[test]
    fn pdf_selection_normalizes_reverse_single_page() {
        let s = PdfSelection {
            start_page: 3,
            start_char: 120,
            end_page: 3,
            end_char: 20,
        };
        let n = s.normalized();
        assert_eq!(n.start_char, 20);
        assert_eq!(n.end_char, 120);
    }

    #[test]
    fn pdf_selection_normalizes_reverse_cross_page() {
        let s = PdfSelection {
            start_page: 5,
            start_char: 0,
            end_page: 2,
            end_char: 10,
        };
        let n = s.normalized();
        assert_eq!(n.start_page, 2);
        assert_eq!(n.end_page, 5);
    }

    #[test]
    fn pdf_selection_is_empty_when_caret() {
        let s = PdfSelection {
            start_page: 1,
            start_char: 7,
            end_page: 1,
            end_char: 7,
        };
        assert!(s.is_empty());
    }
}
