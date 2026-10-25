//! PDF face: a stroke canvas painted over the item's PDF attachment.
//!
//! Owns everything PDF-specific:
//!
//! * Loading the PDF blob from
//!   [`crate::library::bodies::pdf_path`] off the UI thread.
//! * Wrapping it in a [`PdfBackground`] and installing it onto the
//!   mounted [`Board`] via `set_background`.
//! * Publishing the long-press anchor-ref closure
//!   ([`AnchorSourceRef`]) through the Freya context so
//!   [`crate::app::canvas_body`] picks it up without the shell having
//!   to know PDF anchor semantics.
//!
//! The shell contributes only the generic
//! [`CanvasMount::Item`] mount flavour — strokes land on
//! `doc_path(id)` through the standard autosave worker. Everything
//! else is this plugin's business.

use std::sync::{Arc, Mutex, PoisonError};

use freya::prelude::*;
use freya_pdf::{PdfBackground, PdfDocument};

use crate::app::{AnchorSourceCtx, CanvasMount, CanvasPane};
use crate::bookmark::SourceRef;
use crate::canvas::{AnchorSourceRef, lock};
use crate::library::{ItemId, ViewSlot, bodies};
use crate::pdf_select_overlay::PdfSelectOverlay;

/// Freya component that renders the PDF face. Mounts a shell
/// [`CanvasPane`] over a plain [`CanvasMount::Item`] (strokes against
/// `doc_path(id)`) and layers on the PDF-specific side effects:
///
/// background install + anchor-source context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PdfFace {
    pub item: ItemId,
    pub slot: Option<ViewSlot>,
}

impl Component for PdfFace {
    fn render(&self) -> impl IntoElement {
        let item = self.item;
        let mount = CanvasMount::Item {
            id: item,
            slot: self.slot,
        };

        use_provide_context(|| crate::app::FacePaintsBackground);
        // Publish the long-press anchor-ref for this item. Any
        // descendant `canvas_body` picks it up via `try_consume_context`
        // and threads it through `drawing_surface`.
        use_provide_context(move || AnchorSourceCtx {
            source: pdf_anchor_source(item),
            text_press: Some(Arc::new(move |world, window| {
                crate::pdf_select_overlay::select_word_at(item, mount, world, window)
            })),
        });

        // Install the PDF background on whichever `Board` the inner
        // `CanvasPane` publishes. `active_boards` is keyed by mount, so
        // the lookup resolves even in a split layout where sibling
        // mounts share the same item id. First render spawns the
        // async load; later renders no-op because `use_hook` runs once
        // per component instance.
        let pages = use_hook(|| Arc::new(std::sync::Mutex::new(None::<Vec<(f32, f32, f32, f32)>>)));
        crate::plugins::faces::use_jumps(
            crate::plugins::builtin::pdf::KIND,
            item,
            PageTarget {
                mount,
                pages: Arc::clone(&pages),
            },
        );
        let loaded_pages = Arc::clone(&pages);
        use_hook(move || {
            let replaced = Reloads::subscribe(item);
            spawn(async move {
                loop {
                    install_background(item, mount, &loaded_pages);
                    if replaced.recv_async().await.is_err() {
                        return;
                    }
                }
            });
        });

        // Overlay is always mounted; it self-gates on
        // [`InputMode::Select`] inside its handlers so the stroke pump
        // keeps full control of the canvas in Draw / Pan modes.
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .content(Content::Flex)
            .child(
                CanvasPane { mount }.into_element(),
            )
            .child(PdfSelectOverlay { item, mount }.into_element())
    }
}

/// Load `item`'s file and put it under the ink of `mount`'s board.
fn install_background(item: ItemId, mount: CanvasMount, pages: &PageRects) {
    let bg = match load_pdf_background(item, pages) {
        Ok(bg) => bg,
        Err(err) => {
            log::error!("load pdf id={item:?}: {err}");
            return;
        }
    };
    let Some(board) = crate::active_boards::get(mount) else {
        log::warn!("PdfFace install_background: no board registered for {item:?}");
        return;
    };
    lock(&board).set_background(bg);
}

/// Open faces per PDF, told to load the file again when it is replaced.
struct Reloads;

impl Reloads {
    fn registry() -> &'static Mutex<Vec<(ItemId, flume::Sender<()>)>> {
        static FACES: Mutex<Vec<(ItemId, flume::Sender<()>)>> = Mutex::new(Vec::new());
        &FACES
    }

    fn subscribe(item: ItemId) -> flume::Receiver<()> {
        let (tx, rx) = flume::unbounded();
        Self::registry()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((item, tx));
        rx
    }
}

/// Reload every open view of `item`, after its file changed.
pub fn reload(item: ItemId) {
    Reloads::registry()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|(id, tx)| !tx.is_disconnected() && (*id != item || tx.send(()).is_ok()));
}

/// World rectangles `(x, y, w, h)` of the pages, once the PDF is open.
type PageRects = std::sync::Mutex<Option<Vec<(f32, f32, f32, f32)>>>;

/// Why a PDF could not become the canvas background.
#[derive(Debug)]
enum LoadError {
    Read(crate::library::LibraryError),
    Open(freya_pdf::PdfError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(err) => write!(f, "read pdf: {err}"),
            Self::Open(err) => write!(f, "open pdf: {err}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Decode the PDF blob at [`bodies::pdf_path`] into a
/// [`PdfBackground`] usable by the stroke canvas as its backdrop.
fn load_pdf_background(
    id: ItemId,
    pages: &PageRects,
) -> Result<Arc<dyn freya_canvas_bg::CanvasBackground>, LoadError> {
    let bytes = bodies::read_pdf(id).map_err(LoadError::Read)?;
    let doc = PdfDocument::open_bytes(bytes).map_err(LoadError::Open)?;
    *pages.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(freya_pdf::write::page_world_rects(
        doc.page_sizes(),
        freya_pdf::write::PAGE_GAP,
    ));
    Ok(Arc::new(PdfBackground::new(doc)))
}

/// Build the per-item [`AnchorSourceRef`] closure. Long-press pins
/// that land over a page get a [`SourceRef::Plugin`] carrying an
/// encoded [`PdfAnchor::Page`] so chip strips and cross-pane
/// navigation can resolve back to this bundle + page.
fn pdf_anchor_source(item: ItemId) -> AnchorSourceRef {
    Arc::new(move |att| {
        use notein_plugin::payload::Payload;
        let anchor = crate::plugins::builtin::pdf::PdfAnchor {
            page: u32::try_from(att.page.0).unwrap_or(u32::MAX),
            text: None,
            range: None,
        }
        .to_bytes();
        SourceRef::Plugin {
            plugin_id: crate::plugins::builtin::pdf::KIND.to_owned(),
            item: item.0,
            anchor,
            selection: None,
        }
    })
}

/// Where a PDF jump lands: the page rectangles once the document is
/// loaded.
#[derive(Clone)]
struct PageTarget {
    mount: CanvasMount,
    pages: Arc<PageRects>,
}

impl crate::plugins::faces::Apply for PageTarget {
    fn apply(&self, anchor: &[u8]) -> bool {
        use notein_plugin::payload::Payload;
        let Ok(anchor) = crate::plugins::builtin::pdf::PdfAnchor::from_bytes(anchor) else {
            return true;
        };
        let rect = self
            .pages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|pages| pages.get(anchor.page as usize).copied());
        let Some(rect) = rect else {
            return false;
        };
        let Some(board) = crate::active_boards::get(self.mount) else {
            return false;
        };
        if let Some((x, y, w, h)) = rect {
            lock(&board).center_on_world(x + w / 2.0, y + h / 2.0);
        }
        true
    }
}
