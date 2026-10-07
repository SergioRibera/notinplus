//! Modo B shell — mounts the canvas on the left of a horizontal split
//! and a readonly PDF viewer on the right.
//!
//! [`DocLinkedView`] is the route component the home card pushes for a
//! [`ItemKind::DocLinked`] item; [`SourcePdfView`] opens a bare
//! [`ItemKind::SourcePdf`] without a paired canvas. Phase 5 keeps the
//! PDF side intentionally thin — the viewer renders a vertical scroll
//! of page previews using [`freya_pdf::PdfDocument`] directly.

use freya::prelude::*;

use crate::library::ItemId;

/// Canvas + source split for a persisted [`ItemKind::DocLinked`] item.
///
/// Hosted at [`crate::route::Route::DocLinkedView`]. The canvas owns the
/// strokes / bookmarks of the DocLinked item itself; the right pane
/// resolves the paired `source` id and renders its PDF body.
#[derive(Debug, PartialEq)]
pub struct DocLinkedView {
    pub id: u64,
}

impl Component for DocLinkedView {
    fn render(&self) -> impl IntoElement {
        let _item = ItemId(self.id);
        placeholder(&format!(
            "DocLinkedView #{} — próximamente: split canvas + PDF",
            self.id
        ))
    }
}

/// Readonly PDF viewer for a persisted [`ItemKind::SourcePdf`] item.
///
/// Hosted at [`crate::route::Route::SourcePdfView`]. Rendered standalone
/// when the user opens a source item from home; the same component gets
/// embedded in [`DocLinkedView`]'s right pane once Phase 5.2 wires the
/// split layout.
#[derive(Debug, PartialEq)]
pub struct SourcePdfView {
    pub id: u64,
}

impl Component for SourcePdfView {
    fn render(&self) -> impl IntoElement {
        placeholder(&format!(
            "SourcePdfView #{} — próximamente: visor PDF",
            self.id
        ))
    }
}

fn placeholder(text: &str) -> Rect {
    rect()
        .expanded()
        .background(Color::from_rgb(15, 15, 18))
        .center()
        .child(
            label()
                .color(Color::from_rgb(220, 220, 230))
                .font_size(14.0)
                .text(text.to_owned()),
        )
}
