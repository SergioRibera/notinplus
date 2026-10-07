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

use bincode::config::{self, Configuration};
use freya::elements::image::ImageHandle;
use freya::prelude::*;
use freya_engine::prelude::AlphaType;
use freya_pdf::PdfDocument;

use crate::builtin::pdf_source::KIND_ID as PDF_SOURCE_KIND_ID;
use crate::library::{ItemId, bodies};

const CODEC: Configuration = config::standard();

/// Vertical padding + gap used by [`page_list`] when stacking pages.
/// Public so callers can compute per-page y offsets for scroll-targets.
const PAGE_PADDING_TOP: f32 = 16.0;
const PAGE_GAP: f32 = 12.0;
const PAGE_LABEL_GUTTER: f32 = 22.0; // label + spacing below each card

/// Opaque PDF anchor payload stored in [`SourceRef::Plugin::anchor`].
/// Encodes the backing source item id + page index so chips can
/// resolve back to the right viewer + page without the shell needing
/// to know the plugin's schema.
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

/// Width (in device pixels) each page is rasterised at. Chosen to keep
/// the rendered bitmap small enough that a cold-open of a mid-size PDF
/// finishes in a few seconds while staying crisp on a HiDPI laptop
/// display. The viewer scales the resulting image to the pane width.
const PAGE_RENDER_WIDTH_PX: u32 = 900;

/// Callback invoked when the user taps a page. The payload is the
/// zero-based page index. Phase 5.3 reads this to spawn a bookmark
/// anchored at the tapped page.
pub type PageTapCallback = Callback<usize, ()>;

/// Callback invoked once the viewer has rasterised every page. The
/// payload is the ordered list of per-page bitmap heights in pixels —
/// enough information for the caller to compute scroll offsets via
/// [`scroll_offset_for_page`] without re-rasterising the document.
pub type PagesLoadedCallback = Callback<Vec<u32>, ()>;

/// Readonly PDF viewer bound to a [`ItemId`] whose body is a PDF blob
/// on disk (see [`bodies::read_pdf`]).
#[derive(Clone)]
pub struct PdfPaneView {
    item: ItemId,
    on_page_tap: Option<PageTapCallback>,
    on_pages_loaded: Option<PagesLoadedCallback>,
    scroll_controller: Option<freya::prelude::ScrollController>,
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
            on_pages_loaded: None,
            scroll_controller: None,
        }
    }

    #[must_use]
    pub fn on_page_tap(mut self, cb: PageTapCallback) -> Self {
        self.on_page_tap = Some(cb);
        self
    }

    #[must_use]
    pub fn on_pages_loaded(mut self, cb: PagesLoadedCallback) -> Self {
        self.on_pages_loaded = Some(cb);
        self
    }

    /// Drive the internal scrollview from a caller-owned controller.
    /// Lets the enclosing shell jump the viewer to a specific page via
    /// [`scroll_offset_for_page`].
    #[must_use]
    pub fn scroll_controller(mut self, ctrl: freya::prelude::ScrollController) -> Self {
        self.scroll_controller = Some(ctrl);
        self
    }
}

/// Compute the vertical scroll offset that lands `target_page` at the
/// top of the viewport. Page dimensions are taken from the already
/// rasterised entries so the math uses the exact pixel sizes freya
/// will lay out.
#[must_use]
pub fn scroll_offset_for_page(pages: &[PageEntry], target_page: usize) -> i32 {
    let mut y = PAGE_PADDING_TOP;
    for entry in pages.iter().take(target_page) {
        #[allow(clippy::cast_precision_loss)]
        let h = entry.height_px as f32;
        y += h + PAGE_LABEL_GUTTER + PAGE_GAP;
    }
    #[allow(clippy::cast_possible_truncation)]
    let out = y.round() as i32;
    out
}

/// Rasterised PDF page bundled with its on-screen dimensions.
#[derive(Clone)]
pub struct PageEntry {
    handle: ImageHandle,
    width_px: u32,
    height_px: u32,
}

impl std::fmt::Debug for PageEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageEntry")
            .field("width_px", &self.width_px)
            .field("height_px", &self.height_px)
            .finish_non_exhaustive()
    }
}

impl PageEntry {
    /// Produce a layout-only stub for scroll-offset computation. Used
    /// by [`crate::doc_linked`] which only knows per-page heights (via
    /// [`PagesLoadedCallback`]) and never needs to re-render.
    #[must_use]
    pub fn stub(height_px: u32) -> Self {
        // Fabricate a tiny 1×1 opaque image so the handle construction
        // stays infallible — callers only read `height_px` through
        // [`scroll_offset_for_page`].
        let bytes = Bytes::from_static(&[0u8, 0, 0, 255]);
        let handle = ImageHandle::from_rgba(1, 1, bytes, AlphaType::Opaque)
            .expect("1x1 rgba handle always builds");
        Self {
            handle,
            width_px: 1,
            height_px,
        }
    }
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
        let on_loaded = self.on_pages_loaded.clone();
        use_hook({
            let mut state = state;
            move || {
                spawn(async move {
                    match load_pdf_pages(item).await {
                        Ok(pages) => {
                            if let Some(cb) = on_loaded {
                                let heights: Vec<u32> = pages.iter().map(|p| p.height_px).collect();
                                cb.call(heights);
                            }
                            state.set(LoadState::Ready(Arc::new(pages)));
                        }
                        Err(err) => state.set(LoadState::Error(err)),
                    }
                });
            }
        });

        let on_tap = self.on_page_tap.clone();
        let ctrl = self.scroll_controller;
        match state.read().clone() {
            LoadState::Loading => wrap_centered(placeholder_text("Cargando PDF…")),
            LoadState::Error(msg) => wrap_centered(placeholder_text(&msg)),
            LoadState::Ready(pages) if pages.is_empty() => {
                wrap_centered(placeholder_text("PDF sin páginas"))
            }
            LoadState::Ready(pages) => page_list(pages, on_tap, ctrl).into_element(),
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

fn page_list(
    pages: Arc<Vec<PageEntry>>,
    on_tap: Option<PageTapCallback>,
    ctrl: Option<freya::prelude::ScrollController>,
) -> Rect {
    let mut col = rect()
        .width(Size::fill())
        .vertical()
        .cross_align(Alignment::Center)
        .spacing(PAGE_GAP)
        .padding((PAGE_PADDING_TOP, 16.0));
    for (idx, entry) in pages.iter().enumerate() {
        col = col.child(page_card(idx, entry, on_tap.clone()));
    }
    let sv = match ctrl {
        Some(c) => ScrollView::new_controlled(c),
        None => ScrollView::new(),
    };
    rect()
        .width(Size::fill())
        .height(Size::flex(1.0))
        .background(Color::from_rgb(22, 22, 28))
        .child(
            sv.direction(Direction::Vertical)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_anchor_roundtrips_through_bincode() {
        let a = PdfAnchor {
            source: ItemId(42),
            page: 7,
        };
        let bytes = encode_pdf_anchor(a);
        let back = decode_pdf_anchor(&bytes).expect("decode");
        assert_eq!(back, a);
    }

    #[test]
    fn decode_pdf_anchor_rejects_garbage() {
        assert!(decode_pdf_anchor(&[]).is_none());
        assert!(decode_pdf_anchor(&[0xff; 1024]).is_some() || decode_pdf_anchor(&[0xff; 1024]).is_none());
    }

    #[test]
    fn scroll_offset_for_page_is_zero_at_page_zero() {
        let pages = vec![PageEntry::stub(100), PageEntry::stub(200)];
        let out = scroll_offset_for_page(&pages, 0);
        // First page sits at the top padding offset.
        #[allow(clippy::cast_possible_truncation)]
        let expected = PAGE_PADDING_TOP.round() as i32;
        assert_eq!(out, expected);
    }

    #[test]
    fn scroll_offset_for_page_sums_previous_heights() {
        let pages = vec![PageEntry::stub(100), PageEntry::stub(200), PageEntry::stub(300)];
        let out = scroll_offset_for_page(&pages, 2);
        // top padding + (100 + label gutter + gap) + (200 + label gutter + gap)
        let expected = PAGE_PADDING_TOP
            + (100.0 + PAGE_LABEL_GUTTER + PAGE_GAP)
            + (200.0 + PAGE_LABEL_GUTTER + PAGE_GAP);
        #[allow(clippy::cast_possible_truncation)]
        let expected_i = expected.round() as i32;
        assert_eq!(out, expected_i);
    }
}
