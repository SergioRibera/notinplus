//! Shell-side navigation for [`crate::bookmark::SourceRef`] chips.
//!
//! Bookmark cards + sidebar rows render each ref as a chip; tapping
//! the chip calls into here. For PDF plugin anchors we:
//!
//! 1. Resolve the backing [`crate::library::ItemId`] from the ref.
//! 2. Make sure a PDF plugin instance is open (see
//!    [`crate::active_pdf_sources::ensure_registered`]) so we have
//!    world-space geometry on hand.
//! 3. Scroll the PDF-face board to the anchor's content centre.
//! 4. Fire a 500 ms pulse highlight via
//!    [`crate::active_pdf_sources::pulse`] so the user visually tracks
//!    where the chip landed.
//!
//! Other `SourceRef` variants (Canvas, Item, External) are stubbed
//! until Phase 6 — chips still render; tapping logs a warning.

use crate::active_boards;
use crate::active_pdf_sources;
use crate::app::CanvasMount;
use crate::bookmark::SourceRef;
use crate::canvas::lock;
use crate::library::ItemId;
use crate::pdf_viewer::PDF_PLUGIN_ID;

/// Human-readable one-liner for a chip's visible label. For PDF
/// anchors returns `"PDF p.N"` (or `"PDF p.N (range)"` when the
/// anchor carries a char range). Falls back to the plugin id for
/// unknown sources.
#[must_use]
pub fn chip_label(src: &SourceRef) -> String {
    match src {
        SourceRef::Plugin {
            plugin_id,
            anchor,
            selection,
            ..
        } if plugin_id == PDF_PLUGIN_ID => {
            use crate::pdf_viewer::decode_pdf_anchor;
            let page = decode_pdf_anchor(anchor).map(|a| a.page() + 1);
            match (page, selection.is_some()) {
                (Some(p), true) => format!("PDF p.{p} · fragmento"),
                (Some(p), false) => format!("PDF p.{p}"),
                _ => "PDF".to_owned(),
            }
        }
        SourceRef::Plugin { plugin_id, .. } => plugin_id.clone(),
        SourceRef::Canvas { .. } => "Bookmark".to_owned(),
        SourceRef::External { url } => url.clone(),
    }
}

/// Navigate to `src`. For PDF plugin anchors this scrolls the active
/// PDF face + triggers a pulse highlight; other variants currently
/// log a warning and no-op pending Phase 6 chip plumbing.
pub fn navigate(src: &SourceRef) {
    match src {
        SourceRef::Plugin {
            plugin_id,
            item,
            anchor,
            ..
        } if plugin_id == PDF_PLUGIN_ID => {
            let item_id = ItemId(*item);
            navigate_pdf(item_id, anchor.clone());
        }
        SourceRef::Plugin { plugin_id, .. } => {
            log::warn!("navigate: unsupported plugin ref {plugin_id}");
        }
        SourceRef::Canvas { bookmark } => {
            log::warn!("navigate: cross-canvas bookmark nav not implemented (target={bookmark:?})");
        }
        SourceRef::External { url } => {
            log::warn!("navigate: external url nav not implemented ({url})");
        }
    }
}

/// Scroll the PDF face for `item` to the anchor's content centre + pulse
/// highlight. Opens the plugin instance lazily when it's not already
/// registered.
fn navigate_pdf(item: ItemId, anchor: Vec<u8>) {
    let Some(handle) = active_pdf_sources::ensure_registered(item) else {
        log::warn!("navigate_pdf: failed to open PDF plugin for {item:?}");
        return;
    };
    if let Some((wx, wy)) = handle
        .plugin
        .anchor_scroll_target_world(handle.instance, &anchor)
    {
        if let Some(board) = active_boards::get(CanvasMount::Item(item)) {
            lock(&board).center_on_world(wx, wy);
        }
    }
    active_pdf_sources::pulse(item, anchor);
}
