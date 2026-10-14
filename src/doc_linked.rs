//! Shell for a [`crate::library::ItemKind::DocLinked`] bundle.
//!
//! A DocLinked item owns a PDF attachment and the canvas doc that
//! annotates it under the same [`ItemId`]. [`DocLinkedView`] mounts
//! the PDF as the stroke canvas background so strokes, highlights and
//! long-press pins land directly over the pages.
//!
//! A persisted per-item preference (see [`crate::doc_linked_prefs`])
//! lets the user split the shell into two side-by-side panes — PDF on
//! one side, canvas on the other — or keep them overlaid in a single
//! surface. Overlay mode is the default; the split-view chrome buttons
//! live in the top-right stack (see [`DocLinkedChromeExt`]).
//!
//! [`SourcePdfView`] stays available for standalone
//! [`crate::library::ItemKind::SourcePdf`] items (future flows).

use freya::prelude::*;
use istmo::plugins::EdgeInsets;

use crate::app::{CanvasMount, canvas_body};
use crate::doc_linked_prefs::{self, DocLinkedSplit};
use crate::hooks::provide_safe_area_override;
use crate::library::{ItemId, SplitOrientation};

/// Context value a `DocLinked` shell installs so the chrome's
/// top-right stack can render the split-mode buttons without the chrome
/// code having to know about linked-doc specifics.
///
/// Shells that do not install this value get the default chrome layout
/// (no split buttons) — [`crate::app::top_right_toolbar`] falls back to
/// its baseline button row via `try_consume_context`.
#[derive(Clone)]
pub struct DocLinkedChromeExt {
    /// Whether the shell is currently in split mode. Flips the top-row
    /// visibility: split buttons when `false`, the close `×` when
    /// `true`.
    pub is_split: bool,
    /// Trigger a horizontal split — PDF left, canvas right.
    pub on_split_horizontal: NoArgCallback<()>,
    /// Trigger a vertical split — PDF top, canvas bottom.
    pub on_split_vertical: NoArgCallback<()>,
    /// Collapse the split back to overlay mode. Only wired on the
    /// canvas side of a split; the PDF side renders no chrome.
    pub on_close: NoArgCallback<()>,
}

impl std::fmt::Debug for DocLinkedChromeExt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocLinkedChromeExt")
            .field("is_split", &self.is_split)
            .finish_non_exhaustive()
    }
}

/// Canvas + PDF bundle route backed by a
/// [`crate::library::ItemKind::DocLinked`] item.
#[derive(Debug, PartialEq)]
pub struct DocLinkedView {
    pub id: ItemId,
}

impl Component for DocLinkedView {
    fn render(&self) -> impl IntoElement {
        let bundle = self.id;
        let insets = crate::hooks::use_safe_area_insets();
        let pad = *insets.read();
        // Carve the safe area at this shell; nested chrome (overlays,
        // modals) sees zero so it does not re-apply the same inset.
        provide_safe_area_override(EdgeInsets::default());

        let split_state = use_state(move || doc_linked_prefs::load(bundle));
        let split = *split_state.read();

        // Writes the new split preference, kicks off a best-effort
        // disk save in the background. Save failures log and are
        // ignored — the next successful save supersedes the old
        // on-disk state, and the UI already reflects the intent.
        let apply = move |mut setter: State<DocLinkedSplit>, next: DocLinkedSplit| {
            setter.set(next);
            spawn(async move {
                if let Err(err) = doc_linked_prefs::save(bundle, &next) {
                    log::warn!("doc_linked_prefs::save: {err}");
                }
            });
        };

        let on_split_horizontal: NoArgCallback<()> = NoArgCallback::new(move || {
            apply(
                split_state,
                DocLinkedSplit {
                    orientation: Some(SplitOrientation::Horizontal),
                    ratio: 0.5,
                },
            );
        });
        let on_split_vertical: NoArgCallback<()> = NoArgCallback::new(move || {
            apply(
                split_state,
                DocLinkedSplit {
                    orientation: Some(SplitOrientation::Vertical),
                    ratio: 0.5,
                },
            );
        });
        let on_close: NoArgCallback<()> = NoArgCallback::new(move || {
            apply(
                split_state,
                DocLinkedSplit {
                    orientation: None,
                    ratio: split.ratio,
                },
            );
        });

        safe_area_shell(pad)
            .background(Color::from_rgb(15, 15, 18))
            .vertical()
            .content(Content::Flex)
            .child(doc_linked_body(
                bundle,
                split,
                on_split_horizontal,
                on_split_vertical,
                on_close,
            ))
    }
}

/// Body renderer. Overlay mode mounts a single canvas with the
/// standard chrome; split mode mounts two independent canvases laid
/// out along the chosen axis. The canvas on the "annotation side"
/// retains the chrome (so the user still has the toolbar + overlays);
/// the PDF-side pane is a plain canvas_body call — no chrome.
///
/// V1 trade-off: each split pane carries its own [`crate::canvas::Board`]
/// since the current mount pipeline is not wired for board sharing.
/// Both load from the same on-disk doc so they start identical; edits
/// on either side autosave to the same file, so the last mutation
/// wins. The common split-view use case (PDF on one side for reading,
/// annotations on the other) does not exercise divergence — the user
/// edits only one side. We revisit this when a real multi-edit flow
/// shows up.
fn doc_linked_body(
    id: ItemId,
    split: DocLinkedSplit,
    on_split_horizontal: NoArgCallback<()>,
    on_split_vertical: NoArgCallback<()>,
    on_close: NoArgCallback<()>,
) -> Element {
    match split.orientation {
        None => DocLinkedOverlay {
            id,
            on_split_horizontal,
            on_split_vertical,
        }
        .into_element(),
        Some(orientation) => DocLinkedSplitBody {
            id,
            orientation,
            on_close,
        }
        .into_element(),
    }
}

/// Default DocLinked body: a single canvas mount with the standard
/// chrome. The split-mode buttons live in the chrome's top-right stack
/// via the [`DocLinkedChromeExt`] context installed here.
#[derive(Clone, PartialEq)]
struct DocLinkedOverlay {
    id: ItemId,
    on_split_horizontal: NoArgCallback<()>,
    on_split_vertical: NoArgCallback<()>,
}

impl std::fmt::Debug for DocLinkedOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocLinkedOverlay")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Component for DocLinkedOverlay {
    fn render(&self) -> impl IntoElement {
        let id = self.id;
        use_provide_context({
            let on_split_horizontal = self.on_split_horizontal.clone();
            let on_split_vertical = self.on_split_vertical.clone();
            move || DocLinkedChromeExt {
                is_split: false,
                on_split_horizontal: on_split_horizontal.clone(),
                on_split_vertical: on_split_vertical.clone(),
                on_close: NoArgCallback::new(|| {}),
            }
        });
        let state = crate::app::use_chrome_state();
        let coords = crate::app::use_chrome_coords();
        rect()
            .width(Size::fill())
            .height(Size::flex(1.0))
            .content(Content::Flex)
            .child(canvas_body(CanvasMount::PdfBacked(id), state))
            .child(crate::app::canvas_chrome(
                state,
                coords,
                EdgeInsets::default(),
            ))
    }
}

/// Split DocLinked body: two independent canvas mounts laid out along
/// `orientation`. The first child reads the PDF with no chrome (a
/// quieter reading surface); the second child carries the editable
/// canvas with the standard chrome and installs
/// [`DocLinkedChromeExt`] with `is_split = true` so the chrome
/// renders the close `×` instead of the split buttons.
#[derive(Clone, PartialEq)]
struct DocLinkedSplitBody {
    id: ItemId,
    orientation: SplitOrientation,
    on_close: NoArgCallback<()>,
}

impl std::fmt::Debug for DocLinkedSplitBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocLinkedSplitBody")
            .field("id", &self.id)
            .field("orientation", &self.orientation)
            .finish_non_exhaustive()
    }
}

impl Component for DocLinkedSplitBody {
    fn render(&self) -> impl IntoElement {
        let container = match self.orientation {
            SplitOrientation::Horizontal => rect().horizontal(),
            SplitOrientation::Vertical => rect().vertical(),
        };
        let (first_wrap, second_wrap) = match self.orientation {
            SplitOrientation::Horizontal => (
                rect().width(Size::flex(1.)).height(Size::fill()),
                rect().width(Size::flex(1.)).height(Size::fill()),
            ),
            SplitOrientation::Vertical => (
                rect().width(Size::fill()).height(Size::flex(1.)),
                rect().width(Size::fill()).height(Size::flex(1.)),
            ),
        };
        container
            .width(Size::fill())
            .height(Size::flex(1.0))
            .content(Content::Flex)
            .child(first_wrap.child(
                DocLinkedPdfSide { id: self.id }.into_element(),
            ))
            .child(second_wrap.child(
                DocLinkedCanvasSide {
                    id: self.id,
                    on_close: self.on_close.clone(),
                }
                .into_element(),
            ))
    }
}

/// PDF-only split pane: a canvas mount with no chrome. The user still
/// gets pan + zoom on the surface, and strokes land on this pane's
/// own [`crate::canvas::Board`] — same autosave target as the canvas
/// side, so if the user accidentally draws here it still persists.
#[derive(Clone, PartialEq)]
struct DocLinkedPdfSide {
    id: ItemId,
}

impl std::fmt::Debug for DocLinkedPdfSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocLinkedPdfSide")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Component for DocLinkedPdfSide {
    fn render(&self) -> impl IntoElement {
        let state = crate::app::use_chrome_state();
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .content(Content::Flex)
            .child(canvas_body(CanvasMount::PdfBacked(self.id), state))
    }
}

/// Canvas split pane: full chrome, carries the close `×` via
/// [`DocLinkedChromeExt`].
#[derive(Clone, PartialEq)]
struct DocLinkedCanvasSide {
    id: ItemId,
    on_close: NoArgCallback<()>,
}

impl std::fmt::Debug for DocLinkedCanvasSide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocLinkedCanvasSide")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Component for DocLinkedCanvasSide {
    fn render(&self) -> impl IntoElement {
        let id = self.id;
        use_provide_context({
            let on_close = self.on_close.clone();
            move || DocLinkedChromeExt {
                is_split: true,
                on_split_horizontal: NoArgCallback::new(|| {}),
                on_split_vertical: NoArgCallback::new(|| {}),
                on_close: on_close.clone(),
            }
        });
        let state = crate::app::use_chrome_state();
        let coords = crate::app::use_chrome_coords();
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .content(Content::Flex)
            .child(canvas_body(CanvasMount::PdfBacked(id), state))
            .child(crate::app::canvas_chrome(
                state,
                coords,
                EdgeInsets::default(),
            ))
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
        let id = self.id;
        let state = crate::app::use_chrome_state();
        let coords = crate::app::use_chrome_coords();
        let insets = crate::hooks::use_safe_area_insets();
        let pad = *insets.read();
        provide_safe_area_override(EdgeInsets::default());

        let canvas_el = canvas_body(CanvasMount::PdfBacked(id), state).into_element();
        let body = rect()
            .width(Size::fill())
            .height(Size::flex(1.0))
            .child(canvas_el);
        safe_area_shell(pad)
            .background(Color::from_rgb(15, 15, 18))
            .vertical()
            .content(Content::Flex)
            .child(body)
            .child(crate::app::canvas_chrome(
                state,
                coords,
                EdgeInsets::default(),
            ))
    }
}

/// Full-window rect that reserves platform safe-area insets via padding.
/// Keep this consistent with [`crate::workspace_view`]'s sister helper —
/// absolute positioning + `.expanded()` would shift the origin without
/// shrinking the box, so the shell would hang off the right/bottom
/// edges of the window.
fn safe_area_shell(pad: EdgeInsets) -> Rect {
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .padding((pad.top, pad.right, pad.bottom, pad.left))
}
