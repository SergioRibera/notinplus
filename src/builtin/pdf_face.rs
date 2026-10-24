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

use std::sync::Arc;

use freya::prelude::*;
use freya_pdf::{PdfBackground, PdfDocument};

use crate::app::{AnchorSourceCtx, CanvasMount, CanvasPane};
use crate::bookmark::SourceRef;
use crate::builtin::pdf_source;
use crate::canvas::{AnchorSourceRef, lock};
use crate::library::{ItemId, ViewSlot, bodies};
use crate::pdf_select_overlay::PdfSelectOverlay;
use crate::pdf_viewer::{PdfAnchor, encode_pdf_anchor};

/// Freya component that renders the PDF face. Mounts a shell
/// [`CanvasPane`] over a plain [`CanvasMount::Item`] (strokes against
/// `doc_path(id)`) and layers on the PDF-specific side effects:
/// background install + anchor-source context.
#[derive(Clone, Copy, Debug, PartialEq)]
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

        // Publish the long-press anchor-ref for this item. Any
        // descendant `canvas_body` picks it up via `try_consume_context`
        // and threads it through `drawing_surface`.
        use_provide_context(move || AnchorSourceCtx {
            source: pdf_anchor_source(item),
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
            spawn(async move {
                let bg = match load_pdf_background(item, &loaded_pages) {
                    Ok(bg) => bg,
                    Err(err) => {
                        log::error!("load pdf id={item:?}: {err}");
                        return;
                    }
                };
                let Some(board) = crate::active_boards::get(mount) else {
                    log::warn!("PdfFace install_background: no board registered for {item:?}",);
                    return;
                };
                lock(&board).set_background(bg);
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

/// Decode the PDF blob at [`bodies::pdf_path`] into a
/// [`PdfBackground`] usable by the stroke canvas as its backdrop.
fn load_pdf_background(
    id: ItemId,
    pages: &std::sync::Mutex<Option<Vec<(f32, f32, f32, f32)>>>,
) -> Result<Arc<dyn freya_canvas_bg::CanvasBackground>, String> {
    let bytes = bodies::read_pdf(id).map_err(|e| format!("read pdf: {e}"))?;
    let doc = PdfDocument::open_bytes(bytes).map_err(|e| format!("open pdf: {e:?}"))?;
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
        let anchor = encode_pdf_anchor(PdfAnchor::Page {
            source: item,
            page: att.page.0 as u32,
        });
        SourceRef::Plugin {
            plugin_id: pdf_source::KIND_ID.to_owned(),
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
    pages: Arc<std::sync::Mutex<Option<Vec<(f32, f32, f32, f32)>>>>,
}

impl crate::plugins::faces::Apply for PageTarget {
    fn apply(&self, anchor: &[u8]) -> bool {
        use notein_plugin::payload::Payload;
        let Ok(anchor) = crate::plugins::builtin::pdf::PdfAnchor::from_bytes(anchor) else {
            return true;
        };
        let rect = {
            let pages = self.pages.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(pages) = pages.as_ref() else {
                return false;
            };
            pages.get(anchor.page as usize).copied()
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
