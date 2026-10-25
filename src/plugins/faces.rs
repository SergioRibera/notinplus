//! Faces of the builtin kinds, and the shell's entry point to render
//! any document through the catalog.

use freya::prelude::*;
use notein_plugin::face::{Face, FaceContext, FaceRequest, PaneKey};

use super::builtin::{canvas::CanvasKind, pdf::PdfKind};
use super::content::{SnapshotStore, doc_id};
use crate::app::{CanvasMount, CanvasPane};
use crate::library::{DocKind, ItemId, PaneId, ViewSlot, WorkspaceId};

/// Library pane slot → plugin pane key.
#[must_use]
pub fn pane_key(slot: ViewSlot) -> PaneKey {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(&slot.workspace.0);
    bytes[16..].copy_from_slice(&slot.pane.0);
    PaneKey(bytes)
}

/// Plugin pane key → library pane slot.
#[must_use]
pub fn view_slot(key: PaneKey) -> ViewSlot {
    let mut workspace = [0u8; 16];
    let mut pane = [0u8; 16];
    workspace.copy_from_slice(&key.0[..16]);
    pane.copy_from_slice(&key.0[16..]);
    ViewSlot {
        workspace: WorkspaceId(workspace),
        pane: PaneId(pane),
    }
}

const fn item_of<K: notein_plugin::kind::DocumentKind>(ctx: &FaceContext<K>) -> ItemId {
    ItemId(ctx.doc.0)
}

/// Run `on_press` for presses on the header actions of `item` (a
/// document of `kind`) in the pane `slot`.
pub(crate) fn use_header_presses(
    kind: &'static str,
    item: ItemId,
    slot: Option<ViewSlot>,
    mut on_press: impl FnMut(notein_plugin::services::HeaderPress) + 'static,
) {
    use_hook(move || {
        let rx = notein_plugin::services::HostServices::header_presses(
            &*super::host::shell(),
            kind,
        );
        let doc = doc_id(item);
        let pane = slot.map(pane_key);
        spawn(async move {
            while let Ok(press) = rx.recv_async().await {
                if press.doc == doc && press.pane == pane {
                    on_press(press);
                }
            }
        });
    });
}

/// Pane body for `item`; a notice when its plugin is missing or off.
#[must_use]
pub fn render(item: ItemId, kind: &DocKind, slot: Option<ViewSlot>) -> Element {
    let face = super::catalog().face(&kind.plugin_id);
    face.map_or_else(
        || {
            MissingFace {
                kind: kind.plugin_id.clone(),
            }
            .into_element()
        },
        |face| {
            face.render(FaceRequest {
                doc: doc_id(item),
                pane: slot.map(pane_key),
                host: super::host::handle(),
                store: SnapshotStore::new(item, kind.plugin_id.clone()),
            })
        },
    )
}

/// Placeholder for documents whose plugin is not available.
#[derive(Clone, Debug, PartialEq, Eq)]
struct MissingFace {
    kind: String,
}

impl Component for MissingFace {
    fn render(&self) -> impl IntoElement {
        notein_ui_kit::EmptyState::new(freya::icons::lucide::puzzle(), "Necesita un plugin")
            .body(format!(
                "Este documento se abre con un plugin que no está instalado o está desactivado ({}). Instalalo desde Ajustes › Plugins.",
                self.kind
            ))
    }
}

/// Lienzo.
#[derive(Debug, Default)]
pub struct CanvasFace;

impl Face for CanvasFace {
    type Kind = CanvasKind;

    fn render(&self, ctx: FaceContext<CanvasKind>) -> Element {
        let item = item_of(&ctx);
        CanvasWithJumps {
            mount: CanvasMount::Item {
                id: item,
                slot: ctx.pane.map(view_slot),
            },
            item,
        }
        .into_element()
    }
}

/// PDF.
#[derive(Debug, Default)]
pub struct PdfFace;

impl Face for PdfFace {
    type Kind = PdfKind;

    fn render(&self, ctx: FaceContext<PdfKind>) -> Element {
        crate::builtin::pdf_face::PdfFace {
            item: item_of(&ctx),
            slot: ctx.pane.map(view_slot),
        }
        .into_element()
    }
}

/// Nota.
#[derive(Debug, Default)]
pub struct NoteFace;

impl Face for NoteFace {
    type Kind = notein_notes::NotesKind;

    fn header_actions(&self) -> Vec<notein_plugin::face::HeaderAction> {
        crate::notes_face::header_actions()
    }

    fn render(&self, ctx: FaceContext<notein_notes::NotesKind>) -> Element {
        crate::notes_face::NotesFace {
            item: item_of(&ctx),
            slot: ctx.pane.map(view_slot),
        }
        .into_element()
    }
}

/// Biblia.
#[derive(Debug, Default)]
pub struct BibleFace;

impl Face for BibleFace {
    type Kind = notein_bible::BibleKind;

    fn header_actions(&self) -> Vec<notein_plugin::face::HeaderAction> {
        crate::bible_face::header_actions()
    }

    fn render(&self, ctx: FaceContext<notein_bible::BibleKind>) -> Element {
        crate::bible_face::BibleFace {
            item: item_of(&ctx),
            slot: ctx.pane.map(view_slot),
        }
        .into_element()
    }
}

/// How often, and how many times, a jump waits for the board of a
/// freshly opened face to mount.
const MOUNT_POLL: std::time::Duration = std::time::Duration::from_millis(80);
const MOUNT_TRIES: usize = 25;

/// Something a face does with an anchor of its kind.
pub(crate) trait Apply: Clone + 'static {
    /// Apply `anchor`; `false` while the face is not ready yet.
    fn apply(&self, anchor: &[u8]) -> bool;
}

/// Follow the jumps queued for one document of `kind`: the one that
/// opened it, then those sent while it is on screen.
pub(crate) fn use_jumps(kind: &'static str, item: ItemId, target: impl Apply) {
    use notein_plugin::services::HostServices;
    use_hook(move || {
        let doc = doc_id(item);
        let host = super::host::shell();
        let first = host.take_jump(kind, doc);
        let rx = host.jumps(kind);
        spawn(async move {
            let apply = |anchor: Vec<u8>| {
                let target = target.clone();
                async move {
                    for _ in 0..MOUNT_TRIES {
                        if target.apply(&anchor) {
                            return;
                        }
                        async_io::Timer::after(MOUNT_POLL).await;
                    }
                    log::info!("jump: {kind} face never became ready");
                }
            };
            if let Some(anchor) = first {
                apply(anchor).await;
            }
            while let Ok(jump) = rx.recv_async().await {
                if jump.doc == doc {
                    super::host::shell().take_jump(kind, doc);
                    apply(jump.anchor).await;
                }
            }
        });
    });
}

#[derive(Clone, Copy)]
struct CanvasTarget {
    mount: CanvasMount,
}

impl Apply for CanvasTarget {
    fn apply(&self, anchor: &[u8]) -> bool {
        use notein_plugin::payload::Payload;
        let Some(board) = crate::active_boards::get(self.mount) else {
            return false;
        };
        let Ok(anchor) = super::builtin::canvas::CanvasAnchor::from_bytes(anchor) else {
            return true;
        };
        let mut guard = crate::canvas::lock(&board);
        if let Some(layer) = anchor.layer {
            guard.set_active_layer(layer);
        }
        let view = anchor
            .bookmark
            .and_then(|bookmark| guard.view_on_bookmark(crate::ids::BookmarkId(bookmark)));
        drop(guard);
        if let Some(view) = view {
            crate::canvas::glide(&board, view);
        }
        true
    }
}

/// Canvas face with its jumps.
#[derive(Clone, Copy, PartialEq)]
struct CanvasWithJumps {
    mount: CanvasMount,
    item: ItemId,
}

impl Component for CanvasWithJumps {
    fn render(&self) -> impl IntoElement {
        use_jumps(super::builtin::canvas::KIND, self.item, CanvasTarget { mount: self.mount });
        CanvasPane { mount: self.mount }
    }
}
