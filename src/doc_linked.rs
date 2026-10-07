//! Modo B shell — mounts the canvas on the left of a horizontal split
//! and a readonly PDF viewer on the right.
//!
//! [`DocLinkedView`] is the route component the home card pushes for a
//! [`crate::library::ItemKind::DocLinked`] item; [`SourcePdfView`] opens
//! a bare [`crate::library::ItemKind::SourcePdf`] without a paired
//! canvas. The PDF side renders through [`crate::pdf_viewer::PdfPaneView`].

use std::rc::Rc;

use freya::prelude::*;
use freya::router::*;

use crate::home::{LibHandle, open_library};
use crate::library::{ItemId, ItemKind};
use crate::pdf_viewer::PdfPaneView;
use crate::route::Route;

/// Canvas + source split for a persisted
/// [`crate::library::ItemKind::DocLinked`] item.
///
/// Hosted at [`Route::DocLinkedView`]. The left pane renders the
/// DocLinked's own canvas (strokes + bookmarks); the right pane
/// resolves the paired `source` id and renders its PDF body.
#[derive(Debug, PartialEq)]
pub struct DocLinkedView {
    pub id: u64,
}

impl Component for DocLinkedView {
    fn render(&self) -> impl IntoElement {
        let linked = ItemId(self.id);
        let state = use_state(|| LinkedState::Loading);
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

        let back_router = router;
        let header = header_bar(move || {
            let _ = back_router.push(Route::Home);
        });

        let body: Element = match state.read().clone() {
            LinkedState::Loading => centered("Cargando documento ligado…").into_element(),
            LinkedState::Error(msg) => centered(&msg).into_element(),
            LinkedState::Ready { source } => split_body(linked, source).into_element(),
        };

        rect()
            .expanded()
            .background(Color::from_rgb(15, 15, 18))
            .vertical()
            .child(header)
            .child(body)
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

async fn resolve_linked_source(linked: ItemId) -> Result<ItemId, String> {
    let handle: LibHandle = open_library().await.map_err(|e| format!("open library: {e}"))?;
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

fn split_body(_canvas: ItemId, source: ItemId) -> Rect {
    rect()
        .width(Size::fill())
        .height(Size::flex(1.0))
        .horizontal()
        .child(canvas_placeholder(_canvas))
        .child(divider())
        .child(
            rect()
                .width(Size::flex(1.0))
                .height(Size::flex(1.0))
                .child(PdfPaneView::new(source)),
        )
}

fn canvas_placeholder(canvas: ItemId) -> Rect {
    // Phase 5.2 ships the right-pane PDF viewer first; the left-pane
    // canvas lands as a bounded `canvas_root()` once Board gains a
    // non-singleton flavour (follow-up noted in SOURCES_PLAN §5). For
    // now, offer an "abrir lienzo" shortcut so the strokes side stays
    // one tap away.
    let router = RouterContext::get();
    let open_router = router;
    rect()
        .width(Size::flex(1.0))
        .height(Size::flex(1.0))
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

fn centered(text: &str) -> Rect {
    rect().expanded().center().child(
        label()
            .color(Color::from_rgb(200, 200, 210))
            .font_size(14.0)
            .text(text.to_owned()),
    )
}
