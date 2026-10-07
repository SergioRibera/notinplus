//! Modo B shell — mounts the canvas on the left of a horizontal split
//! and a readonly PDF viewer on the right.
//!
//! [`DocLinkedView`] is the route component the home card pushes for a
//! [`crate::library::ItemKind::DocLinked`] item; [`SourcePdfView`] opens
//! a bare [`crate::library::ItemKind::SourcePdf`] without a paired
//! canvas. The PDF side renders through [`crate::pdf_viewer::PdfPaneView`].
//!
//! Phase 5.3 adds the bidirectional bookmark bridge: tapping a PDF page
//! writes a canvas bookmark whose [`SourceRef::Plugin`] ref points at
//! the source item + page, and the side bookmark panel chips scroll
//! the PDF pane back to the referenced page.

use std::rc::Rc;
use std::time::Duration;

use freya::prelude::*;
use freya::router::*;

use crate::bookmark::{Bookmark, SourceRef, WorldPoint};
use crate::doc_op::DocOp;
use crate::home::{LibHandle, open_library};
use crate::ids::BookmarkId;
use crate::library::{ItemId, ItemKind};
use crate::pdf_viewer::{
    PDF_PLUGIN_ID, PdfAnchor, PdfPaneView, decode_pdf_anchor, encode_pdf_anchor,
    scroll_offset_for_page,
};
use crate::route::Route;

/// Canvas + source split for a persisted
/// [`crate::library::ItemKind::DocLinked`] item.
#[derive(Debug, PartialEq)]
pub struct DocLinkedView {
    pub id: u64,
}

impl Component for DocLinkedView {
    fn render(&self) -> impl IntoElement {
        let linked = ItemId(self.id);
        let state = use_state(|| LinkedState::Loading);
        let bookmarks = use_state(Vec::<BookmarkChip>::new);
        let toast = use_state(|| Option::<String>::None);
        let refresh = use_state(|| 0u64);
        let scroll_ctrl = use_scroll_controller(ScrollConfig::default);
        let page_heights = use_state(Vec::<u32>::new);
        let router = RouterContext::get();

        use_hook({
            let mut state = state;
            move || {
                spawn(async move {
                    match resolve_linked_source(linked).await {
                        Ok(source) => state.set(LinkedState::Ready { source }),
                        Err(err) => state.set(LinkedState::Error(err)),
                    }
                });
            }
        });

        // Reload bookmarks whenever `refresh` ticks (first mount + every
        // successful page-tap write).
        use_hook({
            let mut bookmarks = bookmarks;
            let refresh = refresh;
            move || {
                let _tick = *refresh.read();
                spawn(async move {
                    match load_bookmark_chips(linked).await {
                        Ok(list) => bookmarks.set(list),
                        Err(err) => log::warn!("load bookmarks: {err}"),
                    }
                });
            }
        });

        let back_router = router;
        let header = header_bar(move || {
            let _ = back_router.push(Route::Home);
        });

        let body: Element = match state.read().clone() {
            LinkedState::Loading => centered("Cargando documento ligado…").into_element(),
            LinkedState::Error(msg) => centered(&msg).into_element(),
            LinkedState::Ready { source } => {
                let on_tap = page_tap_cb(linked, source, toast, refresh);
                let heights_setter = page_heights;
                let viewer = PdfPaneView::new(source)
                    .on_page_tap(on_tap)
                    .scroll_controller(scroll_ctrl)
                    .on_pages_loaded(Callback::new(move |heights: Vec<u32>| {
                        let mut h = heights_setter;
                        h.set(heights);
                    }));
                let chip_cb = chip_tap_cb(scroll_ctrl, page_heights);
                split_body(linked, viewer, bookmarks, chip_cb).into_element()
            }
        };

        let overlay = toast_overlay(toast);

        rect()
            .expanded()
            .background(Color::from_rgb(15, 15, 18))
            .vertical()
            .child(header)
            .child(body)
            .child(overlay)
    }
}

/// Readonly PDF viewer for a persisted
/// [`crate::library::ItemKind::SourcePdf`] item.
#[derive(Debug, PartialEq)]
pub struct SourcePdfView {
    pub id: u64,
}

impl Component for SourcePdfView {
    fn render(&self) -> impl IntoElement {
        let router = RouterContext::get();
        let back_router = router;
        let header = header_bar(move || {
            let _ = back_router.push(Route::Home);
        });
        rect()
            .expanded()
            .background(Color::from_rgb(15, 15, 18))
            .vertical()
            .child(header)
            .child(PdfPaneView::new(ItemId(self.id)))
    }
}

#[derive(Clone)]
enum LinkedState {
    Loading,
    Ready { source: ItemId },
    Error(String),
}

#[derive(Clone, Debug)]
struct BookmarkChip {
    body: String,
    page: Option<u32>,
}

async fn resolve_linked_source(linked: ItemId) -> Result<ItemId, String> {
    let handle: LibHandle = open_library()
        .await
        .map_err(|e| format!("open library: {e}"))?;
    let lib = handle.lock().await;
    let Some(item) = lib.index().item(linked) else {
        return Err(format!("item {} no existe", linked.0));
    };
    match item.kind {
        ItemKind::DocLinked { source } => Ok(source),
        other => Err(format!(
            "item {} no es DocLinked (kind={other:?})",
            linked.0
        )),
    }
}

async fn load_bookmark_chips(canvas: ItemId) -> Result<Vec<BookmarkChip>, String> {
    use crate::library::LibraryError;
    let handle = open_library()
        .await
        .map_err(|e| format!("open library: {e}"))?;
    let lib = handle.lock().await;
    let doc = match lib.load_doc(canvas).await {
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

fn page_tap_cb(
    canvas: ItemId,
    source: ItemId,
    toast: State<Option<String>>,
    refresh: State<u64>,
) -> Callback<usize, ()> {
    Callback::new(move |page: usize| {
        let mut toast = toast;
        let mut refresh = refresh;
        spawn(async move {
            match create_pdf_bookmark(canvas, source, page).await {
                Ok(()) => {
                    toast.set(Some(format!("Marcador creado en página {}", page + 1)));
                    let next = refresh.peek().saturating_add(1);
                    refresh.set(next);
                }
                Err(err) => {
                    log::error!("create pdf bookmark: {err}");
                    toast.set(Some(format!("No se pudo crear marcador: {err}")));
                }
            }
            // Auto-clear toast after a short delay.
            freya::prelude::timer(Duration::from_millis(2500)).await;
            toast.set(None);
        });
    })
}

fn chip_tap_cb(
    ctrl: ScrollController,
    page_heights: State<Vec<u32>>,
) -> Callback<usize, ()> {
    Callback::new(move |page: usize| {
        let heights = page_heights.read().clone();
        let entries: Vec<_> = heights
            .into_iter()
            .map(crate::pdf_viewer::PageEntry::stub)
            .collect();
        let offset = scroll_offset_for_page(&entries, page);
        let mut ctrl = ctrl;
        ctrl.scroll_to_y(offset);
    })
}

async fn create_pdf_bookmark(
    canvas: ItemId,
    source: ItemId,
    page: usize,
) -> Result<(), String> {
    use crate::library::LibraryError;
    let handle = open_library()
        .await
        .map_err(|e| format!("open library: {e}"))?;
    let mut lib = handle.lock().await;
    let mut doc = match lib.load_doc(canvas).await {
        Ok(d) => d,
        Err(LibraryError::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound => {
            crate::doc::Doc::default()
        }
        Err(err) => return Err(format!("load doc: {err}")),
    };
    let id = BookmarkId::new_v4();
    let now = crate::bookmark_ui::now_ms();
    #[allow(clippy::cast_precision_loss)]
    let world = WorldPoint {
        x: 120.0,
        y: 120.0 + (page as f32) * 60.0,
    };
    let mut bm = Bookmark::new(id, world, now);
    #[allow(clippy::cast_possible_truncation)]
    let page_u32 = page as u32;
    bm.body = format!("Página {} del PDF", page + 1);
    bm.refs = vec![SourceRef::Plugin {
        plugin_id: PDF_PLUGIN_ID.to_owned(),
        anchor: encode_pdf_anchor(PdfAnchor {
            source,
            page: page_u32,
        }),
        selection: Some(format!("Página {}", page + 1)),
    }];
    doc.emit(DocOp::AddBookmark { bookmark: bm });
    lib.save_doc(canvas, &mut doc)
        .await
        .map_err(|err| format!("save doc: {err}"))?;
    Ok(())
}

fn split_body(
    canvas: ItemId,
    viewer: PdfPaneView,
    bookmarks: State<Vec<BookmarkChip>>,
    chip_cb: Callback<usize, ()>,
) -> Rect {
    rect()
        .width(Size::fill())
        .height(Size::flex(1.0))
        .horizontal()
        .child(canvas_placeholder(canvas))
        .child(divider())
        .child(
            rect()
                .width(Size::flex(1.0))
                .height(Size::fill())
                .vertical()
                .child(
                    rect()
                        .width(Size::fill())
                        .height(Size::flex(1.0))
                        .child(viewer),
                )
                .child(bookmark_strip(bookmarks, chip_cb)),
        )
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
        Some(p) => (format!("📑 Página {}", p + 1), Some(p as usize)),
        None => (chip.body.clone(), None),
    };
    let bg = if page.is_some() {
        Color::from_rgb(55, 90, 180)
    } else {
        Color::from_rgb(40, 40, 48)
    };
    let mut node = rect()
        .height(Size::px(28.0))
        .padding((0.0, 10.0))
        .center()
        .with_corner_radius(14.0)
        .background(bg)
        .child(
            label()
                .color(Color::from_rgb(240, 245, 255))
                .font_size(11.0)
                .text(text),
        );
    if let Some(p) = page {
        node = node.on_press(move |_| chip_cb.call(p));
    }
    node
}

fn canvas_placeholder(canvas: ItemId) -> Rect {
    let router = RouterContext::get();
    let open_router = router;
    rect()
        .width(Size::flex(1.0))
        .height(Size::fill())
        .background(Color::from_rgb(26, 26, 32))
        .vertical()
        .center()
        .spacing(14.0)
        .child(
            label()
                .color(Color::from_rgb(230, 230, 240))
                .font_size(14.0)
                .text(format!("Lienzo del documento ligado #{}", canvas.0)),
        )
        .child(
            rect()
                .height(Size::px(36.0))
                .padding((0.0, 20.0))
                .center()
                .with_corner_radius(18.0)
                .background(Color::from_rgb(55, 90, 180))
                .on_press(move |_| {
                    let _ = open_router.push(Route::CanvasView { id: canvas.0 });
                })
                .child(
                    label()
                        .color(Color::from_rgb(240, 245, 255))
                        .font_size(13.0)
                        .text("Abrir lienzo"),
                ),
        )
}

fn divider() -> Rect {
    rect()
        .width(Size::px(1.0))
        .height(Size::fill())
        .background(Color::from_rgb(40, 40, 48))
}

fn header_bar(on_back: impl Fn() + 'static) -> Rect {
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

fn toast_overlay(toast: State<Option<String>>) -> Rect {
    let msg = toast.read().clone();
    match msg {
        None => rect().width(Size::px(0.0)).height(Size::px(0.0)),
        Some(text) => rect()
            .position(
                Position::new_global()
                    .bottom(24.0)
                    .left(0.0)
                    .right(0.0),
            )
            .layer(Layer::Overlay)
            .width(Size::fill())
            .center()
            .child(
                rect()
                    .height(Size::px(36.0))
                    .padding((0.0, 18.0))
                    .background(Color::from_rgb(32, 120, 72))
                    .with_corner_radius(18.0)
                    .center()
                    .child(
                        label()
                            .color(Color::from_rgb(240, 245, 255))
                            .font_size(13.0)
                            .text(text),
                    ),
            ),
    }
}

fn centered(text: &str) -> Rect {
    rect().expanded().center().child(
        label()
            .color(Color::from_rgb(200, 200, 210))
            .font_size(14.0)
            .text(text.to_owned()),
    )
}
