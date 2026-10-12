//! Modo B shell for a [`crate::library::ItemKind::DocLinked`] bundle.
//!
//! A DocLinked item owns both a canvas doc and a PDF attachment under
//! the same [`ItemId`] — no second library item, no `source` indirection.
//! [`DocLinkedView`] renders the PDF viewer front-and-center and offers
//! a header toggle to reveal the paired canvas side in a split layout.
//! [`SourcePdfView`] stays available for standalone
//! [`crate::library::ItemKind::SourcePdf`] items (future flows).
//!
//! Phase 5.3 bookmark bridge: tapping a PDF page writes a canvas
//! bookmark whose [`SourceRef::Plugin`] ref points back at the same
//! item id + page, and the chip strip at the bottom scrolls the PDF
//! back to the referenced page.

use std::rc::Rc;
use std::time::Duration;

use freya::icons::lucide as icons;
use freya::prelude::*;
use freya::router::*;

use crate::app::{CanvasMount, canvas_root};
use crate::bookmark::{Bookmark, SourceRef, WorldPoint};
use crate::components::pane_tree_view::{PaneTreeCallbacks, PaneTreeView};
use crate::doc_linked_prefs::{self, DocLinkedPrefs};
use crate::doc_op::DocOp;
use crate::home::open_library;
use crate::ids::BookmarkId;
use crate::library::{
    ItemId, Pane, PaneId, PaneLayout, PaneTab, SplitOrientation, SplitSide,
};
use crate::pdf_viewer::{
    PDF_PLUGIN_ID, PdfAnchor, PdfPaneView, decode_pdf_anchor, encode_pdf_anchor,
    scroll_offset_for_page,
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
        let toast = use_state(|| Option::<String>::None);
        let refresh = use_state(|| 0u64);
        let scroll_ctrl = use_scroll_controller(ScrollConfig::default);
        let page_heights = use_state(Vec::<u32>::new);
        // Per-item sidecar: show-canvas toggle + split-divider ratio.
        // Loaded once at mount; subsequent toggles and resize-commits
        // flow back through `persist`.
        let loaded = use_hook(move || doc_linked_prefs::load(bundle));
        let show_canvas = use_state(move || loaded.show_canvas);
        // Stable ids so the pane tree body callback can discriminate
        // which role (PDF viewer vs. canvas) owns a given leaf. Minted
        // once per mount via `use_hook`; subsequent renders reuse them
        // so the subcomponents keep their state across show/hide
        // toggles and resize.
        let pdf_pane_id = use_hook(PaneId::new_v4);
        let canvas_pane_id = use_hook(PaneId::new_v4);
        let split_ratio = use_state(move || loaded.split_ratio);
        let router = RouterContext::get();

        let persist: Rc<dyn Fn()> = Rc::new(move || {
            let prefs = DocLinkedPrefs {
                show_canvas: *show_canvas.peek(),
                split_ratio: *split_ratio.peek(),
            };
            if let Err(err) = doc_linked_prefs::save(bundle, prefs) {
                log::warn!("save doc_linked prefs id={bundle}: {err}");
            }
        });

        use_hook({
            let mut bookmarks = bookmarks;
            let refresh = refresh;
            move || {
                let _tick = *refresh.read();
                spawn(async move {
                    match load_bookmark_chips(bundle).await {
                        Ok(list) => bookmarks.set(list),
                        Err(err) => log::warn!("load bookmarks: {err}"),
                    }
                });
            }
        });

        let on_tap = page_tap_cb(bundle, toast, refresh);
        let heights_setter = page_heights;
        let chip_cb = chip_tap_cb(scroll_ctrl, page_heights);

        let header = header_bar(bundle, router, show_canvas, Rc::clone(&persist));
        let strip = bookmark_strip(bookmarks, chip_cb);

        let layout = build_pane_layout(
            *show_canvas.read(),
            pdf_pane_id,
            canvas_pane_id,
            *split_ratio.read(),
            bundle,
        );
        let callbacks = PaneTreeCallbacks {
            on_tab_select: Callback::new(|_| {}),
            on_tab_close: Callback::new({
                let persist = Rc::clone(&persist);
                let back_router = router;
                let mut show_canvas = show_canvas;
                move |(pane_id, _idx): (PaneId, usize)| {
                    if pane_id == canvas_pane_id {
                        show_canvas.set(false);
                        persist();
                    } else {
                        let _ = back_router.push(Route::Home);
                    }
                }
            }),
            on_tab_new: Callback::new(|_| {}),
            on_split_resize: Callback::new({
                let persist = Rc::clone(&persist);
                let mut ratio = split_ratio;
                move |(_, new_ratio): (Vec<SplitSide>, f32)| {
                    ratio.set(new_ratio);
                    persist();
                }
            }),
            on_tab_move: Callback::new(|_| {}),
            on_tab_split: Callback::new(|_| {}),
        };
        let label_cb = Callback::<PaneTab, String>::new(move |_: PaneTab| String::new());
        let body_cb =
            Callback::<(PaneId, PaneTab), Element>::new(move |(pane_id, _): (PaneId, PaneTab)| {
                if pane_id == pdf_pane_id {
                    let on_tap = on_tap.clone();
                    PdfPaneView::new(bundle)
                        .on_page_tap(on_tap)
                        .scroll_controller(scroll_ctrl)
                        .on_pages_loaded(Callback::new(move |heights: Vec<u32>| {
                            let mut h = heights_setter;
                            h.set(heights);
                        }))
                        .into_element()
                } else {
                    canvas_root(CanvasMount::Item(bundle)).into_element()
                }
            });

        let overlay = toast_overlay(toast);
        let tree = rect()
            .width(Size::fill())
            .height(Size::flex(1.0))
            .child(PaneTreeView::new(layout, callbacks, label_cb, body_cb));

        rect()
            .expanded()
            .background(Color::from_rgb(15, 15, 18))
            .vertical()
            .content(Content::Flex)
            .child(header)
            .child(tree)
            .child(strip)
            .child(overlay)
    }
}

/// Build the pane-tree layout for the current show/hide toggle state.
/// `show_canvas = false` → single PDF leaf; `true` → horizontal split
/// (PDF on the first side, canvas on the second). Both leaves carry a
/// synthetic [`PaneTab`] pointing at `bundle` so the tree's selection /
/// tab-count invariants hold even though we never persist this layout.
fn build_pane_layout(
    show_canvas: bool,
    pdf_pane_id: PaneId,
    canvas_pane_id: PaneId,
    ratio: f32,
    bundle: ItemId,
) -> PaneLayout {
    let pdf = PaneLayout::Leaf(Pane {
        id: pdf_pane_id,
        tabs: vec![PaneTab { item: bundle }],
        active_tab: 0,
    });
    if !show_canvas {
        return pdf;
    }
    let canvas = PaneLayout::Leaf(Pane {
        id: canvas_pane_id,
        tabs: vec![PaneTab { item: bundle }],
        active_tab: 0,
    });
    PaneLayout::Split {
        orientation: SplitOrientation::Horizontal,
        ratio,
        first: Box::new(pdf),
        second: Box::new(canvas),
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

fn page_tap_cb(
    bundle: ItemId,
    toast: State<Option<String>>,
    refresh: State<u64>,
) -> Callback<usize, ()> {
    Callback::new(move |page: usize| {
        let mut toast = toast;
        let mut refresh = refresh;
        spawn(async move {
            match create_pdf_bookmark(bundle, page).await {
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
            freya::prelude::timer(Duration::from_millis(2500)).await;
            toast.set(None);
        });
    })
}

fn chip_tap_cb(ctrl: ScrollController, page_heights: State<Vec<u32>>) -> Callback<usize, ()> {
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

async fn create_pdf_bookmark(bundle: ItemId, page: usize) -> Result<(), String> {
    use crate::library::LibraryError;
    let handle = open_library()
        .await
        .map_err(|e| format!("open library: {e}"))?;
    let mut lib = handle.lock().await;
    let mut doc = match lib.load_doc(bundle).await {
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
        item: bundle.0,
        anchor: encode_pdf_anchor(PdfAnchor {
            source: bundle,
            page: page_u32,
        }),
        selection: Some(format!("Página {}", page + 1)),
    }];
    doc.emit(DocOp::AddBookmark { bookmark: bm });
    lib.save_doc(bundle, &mut doc)
        .await
        .map_err(|err| format!("save doc: {err}"))?;
    Ok(())
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

fn header_bar(
    bundle: ItemId,
    router: RouterContext,
    mut show_canvas: State<bool>,
    persist: Rc<dyn Fn()>,
) -> Rect {
    let open_canvas_router = router;
    let open_full = move || {
        let _ = open_canvas_router.push(Route::CanvasView { id: bundle });
    };
    let open_full = Rc::new(open_full);
    let open_full_cb = Rc::clone(&open_full);

    let back_router = router;
    let on_back = Rc::new(move || {
        let _ = back_router.push(Route::Home);
    });
    let back_cb = Rc::clone(&on_back);

    let currently_showing = *show_canvas.read();
    let toggle_label = if currently_showing {
        "Ocultar lienzo"
    } else {
        "Mostrar lienzo"
    };
    let toggle_bg = if currently_showing {
        Color::from_rgb(55, 90, 180)
    } else {
        Color::from_rgb(28, 28, 32)
    };

    rect()
        .horizontal()
        .width(Size::fill())
        .height(Size::px(44.0))
        .padding((0.0, 12.0))
        .content(Content::Flex)
        .background(Color::from_rgb(18, 18, 22))
        .cross_align(Alignment::Center)
        .spacing(8.0)
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
        .child(rect().width(Size::flex(1.0)).height(Size::px(1.0)))
        .child(
            rect()
                .height(Size::px(32.0))
                .padding((0.0, 12.0))
                .center()
                .with_corner_radius(8.0)
                .background(toggle_bg)
                .on_press(move |_| {
                    let next = !*show_canvas.peek();
                    show_canvas.set(next);
                    persist();
                })
                .child(
                    label()
                        .color(Color::from_rgb(240, 245, 255))
                        .font_size(13.0)
                        .text(toggle_label.to_owned()),
                ),
        )
        .child(
            rect()
                .height(Size::px(32.0))
                .padding((0.0, 12.0))
                .center()
                .with_corner_radius(8.0)
                .background(Color::from_rgb(28, 28, 32))
                .on_press(move |_| open_full_cb())
                .child(
                    label()
                        .color(Color::from_rgb(220, 220, 230))
                        .font_size(13.0)
                        .text("Abrir lienzo"),
                ),
        )
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
