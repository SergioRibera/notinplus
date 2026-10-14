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
//! Board hoisting (V2): [`DocLinkedView`] creates the mount's `Board`
//! at its own scope via [`crate::app::mount_board`] and installs the
//! `Arc<Mutex<Board>>` as a Freya context value. Both halves of a
//! split layout reuse this single board — their `canvas_body` calls
//! pick it up via `try_consume_context`, so strokes drawn on one side
//! appear on the other and only one autosave worker owns the on-disk
//! doc. Collapsing the split (`×` button) leaves the board intact and
//! the next render reattaches the overlay to it.
//!
//! [`SourcePdfView`] stays available for standalone
//! [`crate::library::ItemKind::SourcePdf`] items (future flows).

use std::sync::Arc;

use freya::prelude::*;
use istmo::plugins::EdgeInsets;

use crate::app::{CanvasMount, ChromeState, PopupCoords, canvas_body, mount_board};
use crate::canvas::lock;
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

        // Chrome state + popup coords live at the shell level so a
        // split-toggle (unmounting the inner overlay / split subtree)
        // does not reset the toolbar's reactive state. Workers spawned
        // by `mount_board` capture these signals, so the canvas-side
        // chrome keeps syncing zoom / undo / redo across remounts.
        let state = crate::app::use_chrome_state();
        let coords = crate::app::use_chrome_coords();

        // Single board instance for the whole shell. `mount_board`
        // publishes it as a Freya context value, so every inner
        // `canvas_body` call reuses the same `Arc<Mutex<Board>>`
        // instead of standing up a sibling with its own autosave
        // worker (would race-write the same on-disk doc).
        let board = use_hook(move || mount_board(CanvasMount::PdfBacked(bundle), state));
        // Owning-side cleanup: cancels any in-flight stroke and
        // clears the item ↔ board slot. Inner `canvas_body`s are
        // reuse-path mounts and skip this teardown.
        use_drop({
            let board = Arc::clone(&board);
            let mut active_setter = state.active_board;
            let mut active_item_setter = state.active_item;
            move || {
                crate::active_boards::clear_if(bundle, &board);
                lock(&board).cancel();
                let same = active_setter
                    .peek()
                    .as_ref()
                    .map_or(false, |cur| Arc::ptr_eq(cur, &board));
                if same {
                    active_setter.set(None);
                    if *active_item_setter.peek() == Some(bundle) {
                        active_item_setter.set(None);
                    }
                }
            }
        });
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
                state,
                coords,
                on_split_horizontal,
                on_split_vertical,
                on_close,
            ))
    }
}

/// Body renderer. Overlay mode mounts a single canvas with the
/// standard chrome; split mode mounts two canvases (PDF-only, canvas)
/// laid out along the chosen axis. Both the overlay and the canvas
/// side of a split share the hoisted `ChromeState`, so the single
/// redraw-sync worker spawned by `mount_board` keeps the chrome's
/// zoom / undo / redo signals live regardless of mode.
fn doc_linked_body(
    id: ItemId,
    split: DocLinkedSplit,
    state: ChromeState,
    coords: PopupCoords,
    on_split_horizontal: NoArgCallback<()>,
    on_split_vertical: NoArgCallback<()>,
    on_close: NoArgCallback<()>,
) -> Element {
    match split.orientation {
        None => DocLinkedOverlay {
            id,
            state,
            coords,
            on_split_horizontal,
            on_split_vertical,
        }
        .into_element(),
        Some(orientation) => DocLinkedSplitBody {
            id,
            orientation,
            state,
            coords,
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
    state: ChromeState,
    coords: PopupCoords,
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
        let state = self.state;
        let coords = self.coords;
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

/// Split DocLinked body: PDF-only pane next to the editable canvas
/// pane. Both reuse the single `Board` hoisted at the
/// [`DocLinkedView`] scope — strokes stay in one authoritative place
/// and only one autosave worker owns the on-disk doc.
#[derive(Clone, PartialEq)]
struct DocLinkedSplitBody {
    id: ItemId,
    orientation: SplitOrientation,
    state: ChromeState,
    coords: PopupCoords,
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
            .child(first_wrap.child(DocLinkedPdfSide { id: self.id }.into_element()))
            .child(second_wrap.child(
                DocLinkedCanvasSide {
                    id: self.id,
                    state: self.state,
                    coords: self.coords,
                    on_close: self.on_close.clone(),
                }
                .into_element(),
            ))
    }
}

/// PDF-only split pane: a canvas mount with no chrome. The reused
/// board publishes its surface rect to the pen pump via its own
/// `drawing_surface` id, so a stylus landing on this side routes
/// correctly even though the chrome and toolbar live on the sibling
/// side. Uses a throwaway `ChromeState` — none of its signals are
/// read because there is no chrome rendered here.
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

/// Canvas split pane: full chrome bound to the hoisted `ChromeState`
/// (so the toolbar's zoom / undo / redo signals stay live). Installs
/// [`DocLinkedChromeExt`] with `is_split = true` so the chrome
/// renders the close `×` instead of the split buttons.
#[derive(Clone, PartialEq)]
struct DocLinkedCanvasSide {
    id: ItemId,
    state: ChromeState,
    coords: PopupCoords,
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
        let state = self.state;
        let coords = self.coords;
        use_provide_context({
            let on_close = self.on_close.clone();
            move || DocLinkedChromeExt {
                is_split: true,
                on_split_horizontal: NoArgCallback::new(|| {}),
                on_split_vertical: NoArgCallback::new(|| {}),
                on_close: on_close.clone(),
            }
        });
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
