//! Readonly PDF viewer rendered as a scrollable column of pages.
//!
//! Phase 5.2 ships this as the right pane of a [`crate::doc_linked::DocLinkedView`]
//! split and as the standalone body of a [`crate::doc_linked::SourcePdfView`].
//! The view opens the pdf via [`freya_pdf::PdfDocument`], rasterises each
//! page off the UI thread into an [`ImageHandle`], and lays the handles
//! out top-to-bottom inside a [`ScrollView`]. A caller-supplied
//! [`PageTapCallback`] lets the enclosing shell react to taps on
//! individual pages (Phase 5.3 wires this to cross-pane bookmarks).
//!
//! The viewer does NOT implement text selection, zoom, or lazy page
//! rasterisation — those are explicit follow-ups.

use std::sync::Arc;

use freya::elements::image::ImageHandle;
use freya::prelude::*;
use freya_engine::prelude::AlphaType;
use freya_pdf::PdfDocument;

use crate::library::{ItemId, bodies};

/// Width (in device pixels) each page is rasterised at. Chosen to keep
/// the rendered bitmap small enough that a cold-open of a mid-size PDF
/// finishes in a few seconds while staying crisp on a HiDPI laptop
/// display. The viewer scales the resulting image to the pane width.
const PAGE_RENDER_WIDTH_PX: u32 = 900;

/// Callback invoked when the user taps a page. The payload is the
/// zero-based page index. Phase 5.3 reads this to spawn a bookmark
/// anchored at the tapped page.
pub type PageTapCallback = Callback<usize, ()>;

/// Readonly PDF viewer bound to a [`ItemId`] whose body is a PDF blob
/// on disk (see [`bodies::read_pdf`]).
#[derive(Clone)]
pub struct PdfPaneView {
    item: ItemId,
    on_page_tap: Option<PageTapCallback>,
}

impl std::fmt::Debug for PdfPaneView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PdfPaneView")
            .field("item", &self.item)
            .finish_non_exhaustive()
    }
}

impl PartialEq for PdfPaneView {
    fn eq(&self, other: &Self) -> bool {
        self.item == other.item
    }
}

impl PdfPaneView {
    #[must_use]
    pub const fn new(item: ItemId) -> Self {
        Self {
            item,
            on_page_tap: None,
        }
    }

    #[must_use]
    pub fn on_page_tap(mut self, cb: PageTapCallback) -> Self {
        self.on_page_tap = Some(cb);
        self
    }
}

#[derive(Clone)]
struct PageEntry {
    handle: ImageHandle,
    width_px: u32,
    height_px: u32,
}

#[derive(Clone)]
enum LoadState {
    Loading,
    Ready(Arc<Vec<PageEntry>>),
    Error(String),
}

impl Component for PdfPaneView {
    fn render(&self) -> impl IntoElement {
        let item = self.item;
        let state = use_state(|| LoadState::Loading);
        use_hook({
            let mut state = state;
            move || {
                spawn(async move {
                    match load_pdf_pages(item).await {
                        Ok(pages) => state.set(LoadState::Ready(Arc::new(pages))),
                        Err(err) => state.set(LoadState::Error(err)),
                    }
                });
            }
        });

        let on_tap = self.on_page_tap.clone();
        match state.read().clone() {
            LoadState::Loading => wrap_centered(placeholder_text("Cargando PDF…")),
            LoadState::Error(msg) => wrap_centered(placeholder_text(&msg)),
            LoadState::Ready(pages) if pages.is_empty() => {
                wrap_centered(placeholder_text("PDF sin páginas"))
            }
            LoadState::Ready(pages) => page_list(pages, on_tap).into_element(),
        }
    }
}

async fn load_pdf_pages(item: ItemId) -> Result<Vec<PageEntry>, String> {
    let bytes = bodies::read_pdf(item).map_err(|e| format!("read pdf: {e}"))?;
    let doc = PdfDocument::open_bytes(bytes).map_err(|e| format!("open pdf: {e:?}"))?;
    let total = doc.page_count();
    let mut pages = Vec::with_capacity(total);
    for idx in 0..total {
        let (nat_w, nat_h) = doc.page_size(idx).unwrap_or((612.0, 792.0));
        let aspect = if nat_h > 0.0 { nat_w / nat_h } else { 1.0 };
        let target_h = {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let h = (PAGE_RENDER_WIDTH_PX as f32 / aspect.max(0.01)).round() as u32;
            h.max(1)
        };
        match doc.render_page_rgba(idx, PAGE_RENDER_WIDTH_PX, target_h) {
            Ok((rgba, w, h)) => {
                let bytes = Bytes::from(rgba);
                let Some(handle) = ImageHandle::from_rgba(w, h, bytes, AlphaType::Opaque) else {
                    log::warn!("pdf page {idx}: ImageHandle::from_rgba refused ({w}x{h})");
                    continue;
                };
                pages.push(PageEntry {
                    handle,
                    width_px: w,
                    height_px: h,
                });
            }
            Err(err) => log::warn!("pdf page {idx} render: {err}"),
        }
    }
    Ok(pages)
}

fn page_list(pages: Arc<Vec<PageEntry>>, on_tap: Option<PageTapCallback>) -> Rect {
    let mut col = rect()
        .width(Size::fill())
        .vertical()
        .cross_align(Alignment::Center)
        .spacing(12.0)
        .padding((16.0, 16.0));
    for (idx, entry) in pages.iter().enumerate() {
        col = col.child(page_card(idx, entry, on_tap.clone()));
    }

    rect()
        .width(Size::fill())
        .height(Size::flex(1.0))
        .background(Color::from_rgb(22, 22, 28))
        .child(
            ScrollView::new()
                .direction(Direction::Vertical)
                .show_scrollbar(true)
                .child(col),
        )
}

fn page_card(idx: usize, entry: &PageEntry, on_tap: Option<PageTapCallback>) -> Rect {
    #[allow(clippy::cast_precision_loss)]
    let w = entry.width_px as f32;
    #[allow(clippy::cast_precision_loss)]
    let h = entry.height_px as f32;
    let img = image(entry.handle.clone())
        .width(Size::px(w))
        .height(Size::px(h))
        .aspect_ratio(AspectRatio::Min);
    let mut card = rect()
        .width(Size::px(w))
        .height(Size::px(h))
        .background(Color::from_rgb(255, 255, 255))
        .with_corner_radius(6.0)
        .child(img);
    if let Some(cb) = on_tap {
        card = card.on_press(move |_| cb.call(idx));
    }
    rect()
        .vertical()
        .cross_align(Alignment::Center)
        .spacing(4.0)
        .child(card)
        .child(
            label()
                .color(Color::from_rgb(180, 180, 190))
                .font_size(11.0)
                .text(format!("Página {}", idx + 1)),
        )
}

fn placeholder_text(text: &str) -> Rect {
    rect().center().child(
        label()
            .color(Color::from_rgb(200, 200, 210))
            .font_size(13.0)
            .text(text.to_owned()),
    )
}

fn wrap_centered(child: Rect) -> Element {
    rect()
        .width(Size::fill())
        .height(Size::flex(1.0))
        .center()
        .background(Color::from_rgb(22, 22, 28))
        .child(child)
        .into_element()
}
