//! Shell for a [`crate::library::ItemKind::DocLinked`] bundle.
//!
//! A DocLinked item owns a PDF attachment and the canvas doc that
//! annotates it under the same [`ItemId`]. [`DocLinkedView`] mounts
//! the PDF as the stroke canvas background so strokes, highlights and
//! long-press pins land directly over the pages.
//!
//! [`SourcePdfView`] stays available for standalone
//! [`crate::library::ItemKind::SourcePdf`] items (future flows).
//!
//! The chip strip at the bottom lists existing PDF-anchored bookmarks;
//! tapping a chip pans the viewport to the referenced page.

use freya::prelude::*;

use crate::app::{CanvasMount, canvas_body};
use crate::library::ItemId;

/// Canvas + PDF bundle route backed by a
/// [`crate::library::ItemKind::DocLinked`] item.
#[derive(Debug, PartialEq)]
pub struct DocLinkedView {
    pub id: ItemId,
}

impl Component for DocLinkedView {
    fn render(&self) -> impl IntoElement {
        let bundle = self.id;
        let state = crate::app::use_chrome_state();
        let coords = crate::app::use_chrome_coords();
        let insets = crate::hooks::use_safe_area_insets();
        let pad = *insets.read();

        let canvas_el = canvas_body(CanvasMount::PdfBacked(bundle), state).into_element();
        let body = rect()
            .width(Size::fill())
            .height(Size::flex(1.0))
            .child(canvas_el);

        rect()
            .expanded()
            .background(Color::from_rgb(15, 15, 18))
            .vertical()
            .content(Content::Flex)
            .child(body)
            .child(crate::app::canvas_chrome(state, coords, pad))
    }
}

/// Readonly PDF viewer for a persisted
/// [`crate::library::ItemKind::SourcePdf`] item.
#[derive(Debug, PartialEq)]
pub struct SourcePdfView {
    pub id: ItemId,
}

impl Component for SourcePdfView {
    fn render(&self) -> impl IntoElement {
        let id = self.id;
        let state = crate::app::use_chrome_state();
        let coords = crate::app::use_chrome_coords();
        let insets = crate::hooks::use_safe_area_insets();
        let pad = *insets.read();

        let canvas_el = canvas_body(CanvasMount::PdfBacked(id), state).into_element();
        let body = rect()
            .width(Size::fill())
            .height(Size::flex(1.0))
            .child(canvas_el);
        rect()
            .expanded()
            .background(Color::from_rgb(15, 15, 18))
            .vertical()
            .content(Content::Flex)
            .child(body)
            .child(crate::app::canvas_chrome(state, coords, pad))
    }
}
