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

use std::rc::Rc;

use freya::icons::lucide as icons;
use freya::prelude::*;
use freya::router::*;

use crate::app::{CanvasMount, canvas_root};
use crate::bookmark::{Bookmark, SourceRef};
use crate::home::open_library;
use crate::library::ItemId;
use crate::pdf_viewer::{
    PDF_PLUGIN_ID, PdfAnchor, PdfPaneView, decode_pdf_anchor,
};
use crate::route::Route;

/// Canvas + PDF bundle route backed by a
/// [`crate::library::ItemKind::DocLinked`] item.
#[derive(Debug, PartialEq)]
pub struct DocLinkedView {
    pub id: ItemId,
}

impl Component for DocLinkedView {
    fn render(&self) -> impl IntoElement {
        let bundle = self.id;
        let bookmarks = use_state(Vec::<BookmarkChip>::new);
        let router = RouterContext::get();

        use_hook({
            let mut bookmarks = bookmarks;
            move || {
                spawn(async move {
                    match load_bookmark_chips(bundle).await {
                        Ok(list) => bookmarks.set(list),
                        Err(err) => log::warn!("load bookmarks: {err}"),
                    }
                });
            }
        });

        let back_router = router;
        let header = plain_header(move || {
            let _ = back_router.push(Route::Home);
        });
        let chip_cb = chip_tap_cb();
        let strip = bookmark_strip(bookmarks, chip_cb);

        let body = rect()
            .width(Size::fill())
            .height(Size::flex(1.0))
            .child(canvas_root(CanvasMount::PdfBacked(bundle)));

        rect()
            .expanded()
            .background(Color::from_rgb(15, 15, 18))
            .vertical()
            .content(Content::Flex)
            .child(header)
            .child(body)
            .child(strip)
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
        let router = RouterContext::get();
        let back_router = router;
        let header = plain_header(move || {
            let _ = back_router.push(Route::Home);
        });
        rect()
            .expanded()
            .background(Color::from_rgb(15, 15, 18))
            .vertical()
            .child(header)
            .child(PdfPaneView::new(self.id))
    }
}

#[derive(Clone, Debug)]
struct BookmarkChip {
    body: String,
    page: Option<u32>,
}

async fn load_bookmark_chips(bundle: ItemId) -> Result<Vec<BookmarkChip>, String> {
    use crate::library::LibraryError;
    let handle = open_library()
        .await
        .map_err(|e| format!("open library: {e}"))?;
    let lib = handle.lock().await;
    let doc = match lib.load_doc(bundle).await {
        Ok(d) => d,
        Err(LibraryError::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(err) => return Err(format!("load doc: {err}")),
    };
    let chips = doc
        .bookmarks()
        .iter()
        .map(|bm| {
            let page = pdf_ref(bm).map(|a| a.page);
            BookmarkChip {
                body: bm.body.clone(),
                page,
            }
        })
        .collect();
    Ok(chips)
}

fn pdf_ref(bm: &Bookmark) -> Option<PdfAnchor> {
    bm.refs.iter().find_map(|r| match r {
        SourceRef::Plugin {
            plugin_id, anchor, ..
        } if plugin_id == PDF_PLUGIN_ID => decode_pdf_anchor(anchor),
        _ => None,
    })
}

/// Chip strip pan handler. Receives the target page index. For now a
/// no-op — the Board viewport pan wiring lands next.
fn chip_tap_cb() -> Callback<usize, ()> {
    Callback::new(|_page: usize| {})
}

fn bookmark_strip(
    bookmarks: State<Vec<BookmarkChip>>,
    chip_cb: Callback<usize, ()>,
) -> Rect {
    let entries = bookmarks.read().clone();
    let mut strip = rect()
        .horizontal()
        .width(Size::fill())
        .height(Size::px(44.0))
        .padding((0.0, 10.0))
        .background(Color::from_rgb(18, 18, 22))
        .cross_align(Alignment::Center)
        .spacing(8.0)
        .child(
            label()
                .color(Color::from_rgb(160, 160, 170))
                .font_size(11.0)
                .text("Marcadores:"),
        );
    if entries.is_empty() {
        strip = strip.child(
            label()
                .color(Color::from_rgb(120, 120, 130))
                .font_size(11.0)
                .text("Tocá una página para agregar"),
        );
        return strip;
    }
    for chip in entries {
        strip = strip.child(bookmark_chip(chip, chip_cb.clone()));
    }
    strip
}

fn bookmark_chip(chip: BookmarkChip, chip_cb: Callback<usize, ()>) -> Rect {
    let (text, page) = match chip.page {
        Some(p) => (format!("Página {}", p + 1), Some(p as usize)),
        None => (chip.body.clone(), None),
    };
    let bg = if page.is_some() {
        Color::from_rgb(55, 90, 180)
    } else {
        Color::from_rgb(40, 40, 48)
    };
    let text_color = Color::from_rgb(240, 245, 255);
    let mut node = rect()
        .horizontal()
        .height(Size::px(28.0))
        .padding((0.0, 10.0))
        .spacing(6.0)
        .cross_align(Alignment::Center)
        .with_corner_radius(14.0)
        .background(bg);
    if page.is_some() {
        node = node.child(
            SvgViewer::new(icons::bookmark())
                .color(text_color)
                .width(Size::px(12.0))
                .height(Size::px(12.0)),
        );
    }
    node = node.child(label().color(text_color).font_size(11.0).text(text));
    if let Some(p) = page {
        node = node.on_press(move |_| chip_cb.call(p));
    }
    node
}

fn plain_header(on_back: impl Fn() + 'static) -> Rect {
    let on_back = Rc::new(on_back);
    let back_cb = Rc::clone(&on_back);
    rect()
        .horizontal()
        .width(Size::fill())
        .height(Size::px(44.0))
        .padding((0.0, 12.0))
        .background(Color::from_rgb(18, 18, 22))
        .cross_align(Alignment::Center)
        .spacing(10.0)
        .child(
            rect()
                .height(Size::px(32.0))
                .padding((0.0, 12.0))
                .center()
                .with_corner_radius(8.0)
                .background(Color::from_rgb(28, 28, 32))
                .on_press(move |_| back_cb())
                .child(
                    label()
                        .color(Color::from_rgb(220, 220, 230))
                        .font_size(13.0)
                        .text("← Home"),
                ),
        )
}

