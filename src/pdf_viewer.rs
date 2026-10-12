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
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PdfAnchor {
    pub source: ItemId,
    pub page: u32,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_anchor_roundtrips_through_bincode() {
        let a = PdfAnchor {
            source: ItemId::new_v4(),
            page: 7,
        };
        let bytes = encode_pdf_anchor(a);
        let back = decode_pdf_anchor(&bytes).expect("decode");
        assert_eq!(back, a);
    }

    #[test]
    fn decode_pdf_anchor_rejects_empty() {
        assert!(decode_pdf_anchor(&[]).is_none());
    }
}
