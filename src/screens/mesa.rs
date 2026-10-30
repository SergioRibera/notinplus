//! A mesa: the open documents side by side, each pane with its tabs,
//! the document panel beside them and the palette over everything.

mod bar;
mod header;
mod here;
mod notices;
mod palette;
mod panel;
pub(crate) mod panes;

use std::sync::Arc;

use freya::prelude::*;
use notein_plugin::services::{HistoryRequest, HistoryStep};
use notein_ui_kit::theme::palette as theme;
use notein_ui_kit::{
    Button, EmptyState, ModalController, PanelEdge, SidePanel, Toast, Toasts, tokens, width_class,
};

use crate::library::{
    ItemId, LibraryData, PaneId, PaneTab, SplitOrientation, SplitSide, TabTarget, ViewSlot,
    Workspace, WorkspaceId,
};
use crate::state::{self, LibraryState, LibraryView};
use notein_i18n::tr;
use panes::SplitEdge;

pub use panel::PanelTab;

/// Persisted operations on one mesa's panes and tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ops {
    pub mesa: WorkspaceId,
}

impl Ops {
    /// Make `pane` the focused one.
    pub fn focus(self, pane: PaneId) {
        let mesa = self.mesa;
        state::edit(crate::state::LibraryEdit::FocusPane, async move |lib| {
            lib.focus_workspace_pane(mesa, Some(pane)).await
        });
    }

    pub fn select_tab(self, pane: PaneId, index: usize) {
        let mesa = self.mesa;
        state::edit(crate::state::LibraryEdit::SwitchTab, async move |lib| {
            lib.focus_pane_tab(mesa, pane, index).await
        });
    }

    /// Close tab `index` of `pane`. A scratch canvas with something
    /// written offers to keep it for a moment.
    pub fn close_tab(self, pane: PaneId, index: usize, tab: &PaneTab) {
        let mesa = self.mesa;
        if let TabTarget::Scratch(scratch) = tab.target {
            scratch_closed(scratch);
        }
        state::edit(crate::state::LibraryEdit::CloseTab, async move |lib| {
            lib.close_pane_tab(mesa, pane, index).await
        });
    }

    /// Ask what to open in a new tab of `pane`.
    pub fn new_tab(self, pane: PaneId) {
        crate::flows::pickers::document_picker(crate::flows::pickers::Purpose::NewTab {
            pane: Some(pane),
        });
    }

    /// Open a scratch canvas in `pane`.
    pub fn new_scratch(self, pane: PaneId) {
        let mesa = self.mesa;
        state::edit(crate::state::LibraryEdit::OpenDraft, async move |lib| {
            lib.add_scratch_tab(mesa, pane).await
        });
    }

    /// Split `pane`, the new pane on the right or below.
    pub fn split(self, pane: PaneId, orientation: SplitOrientation) {
        let mesa = self.mesa;
        state::edit(crate::state::LibraryEdit::SplitPane, async move |lib| {
            lib.split_workspace_pane(mesa, pane, orientation, SplitSide::Second)
                .await
                .map(drop)
        });
    }

    pub fn close_pane(self, pane: PaneId) {
        let mesa = self.mesa;
        state::edit(crate::state::LibraryEdit::ClosePane, async move |lib| {
            lib.close_workspace_pane(mesa, pane).await
        });
    }

    pub fn resize(self, path: Vec<SplitSide>, ratio: f32) {
        let mesa = self.mesa;
        state::edit(crate::state::LibraryEdit::Resize, async move |lib| {
            lib.set_workspace_split_ratio(mesa, &path, ratio).await
        });
    }

    pub fn move_tab(self, src: PaneId, index: usize, dst: PaneId) {
        let mesa = self.mesa;
        state::edit(crate::state::LibraryEdit::MoveTab, async move |lib| {
            lib.move_workspace_tab(mesa, src, index, dst).await
        });
    }

    pub fn reorder(self, pane: PaneId, from: usize, to: usize) {
        let mesa = self.mesa;
        state::edit(crate::state::LibraryEdit::ReorderTabs, async move |lib| {
            lib.reorder_pane_tab(mesa, pane, from, to).await
        });
    }

    /// Move a tab onto `edge` of `dst`, splitting it.
    pub fn split_with_tab(self, src: PaneId, index: usize, dst: PaneId, edge: SplitEdge) {
        let mesa = self.mesa;
        state::edit(crate::state::LibraryEdit::SplitPane, async move |lib| {
            lib.move_workspace_tab_to_split(
                mesa,
                src,
                index,
                dst,
                edge.orientation(),
                edge.new_side(),
            )
            .await
            .map(drop)
        });
    }
}

/// Time the "Borrador descartado" toast offers to keep the canvas.
const SCRATCH_GRACE: std::time::Duration =
    std::time::Duration::from_millis(notein_ui_kit::tokens::toast::SCRATCH);

/// A scratch canvas is closing: when it holds anything, keep its doc
/// for a few seconds behind "Guardar".
fn scratch_closed(scratch: crate::library::ScratchId) {
    let mount = crate::app::CanvasMount::Ephemeral(scratch);
    let Some(board) = crate::active_boards::get(mount) else {
        return;
    };
    let doc = crate::canvas::lock(&board).doc().clone();
    if doc.is_blank() {
        return;
    }
    let folder = crate::screens::home::Browse::get().folder();
    Toasts::get().show(
        Toast::new(tr!("mesa-draft-discarded"))
            .duration(SCRATCH_GRACE)
            .action(tr!("common-save"), move |()| {
                save_scratch(doc.clone(), folder);
            }),
    );
}

fn save_scratch(doc: crate::doc::Doc, folder: crate::library::FolderId) {
    let name = tr!(
        "mesa-draft-name",
        "date" => chrono::Local::now().format("%d/%m").to_string(),
    );
    state::edit(crate::state::LibraryEdit::SaveDraft, async move |lib| {
        let item = lib
            .create_item(
                folder,
                crate::library::DocKind::plugin(crate::plugins::builtin::canvas::KIND),
                &name,
                None,
                Vec::new(),
                crate::library::BackgroundStyle::default(),
            )
            .await?;
        lib.save_doc(item, &doc).await?;
        Toasts::get().show(
            Toast::new(tr!("mesa-draft-saved", "name" => name.clone()))
                .action(tr!("common-open"), move |()| crate::flows::open(item)),
        );
        Ok(())
    });
}

/// UI state of the open mesa, shared by its bar, panes and panel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct MesaUi {
    pub panel: State<bool>,
    pub panel_tab: State<PanelTab>,
}

impl std::fmt::Debug for MesaUi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MesaUi").finish_non_exhaustive()
    }
}

impl MesaUi {
    pub fn toggle_panel(mut self) {
        let open = *self.panel.peek();
        self.panel.set(!open);
    }

    pub fn show_panel(mut self, tab: PanelTab) {
        self.panel_tab.set(tab);
        self.panel.set(true);
    }
}

/// The focused pane and its active tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Focus {
    pub pane: PaneId,
    pub index: usize,
    pub tab: Option<PaneTab>,
}

impl Focus {
    /// Focus of `mesa`: its focused pane, else the first one.
    #[must_use]
    pub fn of(mesa: &Workspace) -> Option<Self> {
        let leaves = mesa.layout.leaves();
        let pane = mesa
            .focused_pane
            .and_then(|id| leaves.iter().find(|p| p.id == id))
            .or_else(|| leaves.first())?;
        let index = pane.active_tab as usize;
        Some(Self {
            pane: pane.id,
            index,
            tab: pane.tabs.get(index).cloned(),
        })
    }

    #[must_use]
    pub fn item(&self) -> Option<ItemId> {
        self.tab.as_ref().and_then(PaneTab::item)
    }
}

/// The mesa route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mesa {
    pub id: WorkspaceId,
}

impl Component for Mesa {
    fn render(&self) -> impl IntoElement {
        let p = theme();
        let view = LibraryView::get();
        let insets = *crate::hooks::use_safe_area_insets().read();
        // The mesa pads itself by the insets: the panes, their faces and
        // the canvas chrome inside must not pad again.
        crate::hooks::provide_safe_area_override(istmo::plugins::EdgeInsets::default());
        let body = match view.state() {
            LibraryState::Loading => rect().expanded().into_element(),
            LibraryState::Failed(why) => EmptyState::new(
                freya::icons::lucide::triangle_alert(),
                tr!("mesa-failed-title"),
            )
            .body(why)
            .into_element(),
            LibraryState::Ready(data) => match data.workspace(self.id) {
                Some(_) => Open { data, id: self.id }.into_element(),
                None => {
                    EmptyState::new(freya::icons::lucide::layout_grid(), tr!("mesa-gone-title"))
                        .body(tr!("mesa-gone-body"))
                        .action(Button::new(tr!("mesa-go-home")).primary().on_press(|_| {
                            crate::screens::home::open_folder(crate::library::ROOT_FOLDER);
                        }))
                        .into_element()
                }
            },
        };
        rect()
            .expanded()
            .background(p.bg)
            .padding((insets.top, insets.right, insets.bottom, insets.left))
            .child(body)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Open {
    data: Arc<LibraryData>,
    id: WorkspaceId,
}

impl Component for Open {
    fn render(&self) -> impl IntoElement {
        let compact = width_class().is_compact();
        let panel = use_state(|| false);
        let panel_tab = use_state(PanelTab::default);
        let ui = MesaUi { panel, panel_tab };
        let ops = Ops { mesa: self.id };
        let Some(mesa) = self.data.workspace(self.id).cloned() else {
            return rect().into_element();
        };
        let focus = Focus::of(&mesa);
        let shortcuts = Shortcuts {
            ops,
            ui,
            focus: focus.clone(),
            data: Arc::clone(&self.data),
        };
        let show_panel = *panel.read() && !compact;
        if *panel.read() && compact {
            let mut panel = panel;
            panel.set(false);
            panel::open_sheet(Arc::clone(&self.data), focus.as_ref(), *panel_tab.peek());
        }
        rect()
            .expanded()
            .content(Content::Flex)
            .on_global_key_down(move |e: Event<KeyboardEventData>| shortcuts.handle(&e))
            .child(bar::MesaBar {
                data: Arc::clone(&self.data),
                mesa: self.id,
                ui,
                focus: focus.clone(),
            })
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.0))
                    .horizontal()
                    .content(Content::Flex)
                    .child(
                        SidePanel::new(
                            show_panel,
                            tokens::size::PANEL_WIDTH,
                            panel::Panel {
                                data: Arc::clone(&self.data),
                                focus,
                                tab: panel_tab,
                            },
                        )
                        .edge(PanelEdge::Left),
                    )
                    .child(
                        rect()
                            .width(Size::flex(1.0))
                            .height(Size::fill())
                            .padding((0.0, tokens::space::XS, tokens::space::XS, tokens::space::XS))
                            .child(panes::Panes {
                                data: Arc::clone(&self.data),
                                mesa,
                                ops,
                            }),
                    )
                    .child(crate::annotation::AnnotationSidebar),
            )
            .into_element()
    }
}

/// The mesa's keyboard shortcuts.
#[derive(Clone)]
struct Shortcuts {
    ops: Ops,
    ui: MesaUi,
    focus: Option<Focus>,
    data: Arc<LibraryData>,
}

impl Shortcuts {
    /// Hand undo or redo to the document focused in the mesa; its face
    /// decides what that means (and ignores it when its own text field
    /// already took the keystroke).
    fn step_history(&self, step: HistoryStep) -> bool {
        let Some(Focus {
            pane,
            tab:
                Some(PaneTab {
                    target: TabTarget::Item(item),
                    ..
                }),
            ..
        }) = &self.focus
        else {
            return false;
        };
        let Some(found) = self.data.item(*item) else {
            return false;
        };
        let slot = ViewSlot {
            workspace: self.ops.mesa,
            pane: *pane,
        };
        crate::plugins::host::shell().step_history(
            &found.kind.plugin_id,
            HistoryRequest {
                doc: crate::plugins::content::doc_id(*item),
                pane: Some(crate::plugins::faces::pane_key(slot)),
                step,
            },
        )
    }

    fn handle(&self, e: &Event<KeyboardEventData>) {
        let command = e.modifiers.ctrl() || e.modifiers.meta();
        if !command || ModalController::get().is_open() {
            return;
        }
        let shift = e.modifiers.shift();
        let pane = self.focus.as_ref().map(|f| f.pane);
        let handled = match &e.key {
            Key::Character(c) => match (c.to_lowercase().as_str(), shift) {
                ("k", _) => {
                    palette::open(
                        Arc::clone(&self.data),
                        self.ops,
                        self.ui,
                        self.focus.as_ref(),
                    );
                    true
                }
                ("t", false) => pane.is_some_and(|p| {
                    self.ops.new_tab(p);
                    true
                }),
                ("w", false) => match &self.focus {
                    Some(Focus {
                        pane,
                        index,
                        tab: Some(tab),
                    }) => {
                        self.ops.close_tab(*pane, *index, tab);
                        true
                    }
                    _ => false,
                },
                ("\\", _) => pane.is_some_and(|p| {
                    self.ops.split(p, SplitOrientation::Horizontal);
                    true
                }),
                ("b", false) => {
                    self.ui.toggle_panel();
                    true
                }
                ("n", false) => {
                    crate::flows::create::open_menu_from_keyboard(crate::flows::create::Context {
                        folder: crate::screens::home::Browse::get().folder(),
                        ..Default::default()
                    });
                    true
                }
                ("n", true) => {
                    crate::flows::create::new_mesa();
                    true
                }
                ("o", false) => {
                    crate::flows::pickers::document_picker(
                        crate::flows::pickers::Purpose::NewTab { pane },
                    );
                    true
                }
                (",", _) => {
                    crate::screens::settings::open();
                    true
                }
                ("z", false) => self.step_history(HistoryStep::Undo),
                ("z", true) | ("y", false) => self.step_history(HistoryStep::Redo),
                _ => false,
            },
            Key::Named(NamedKey::Tab) => self.next_tab(shift),
            Key::Named(_) => false,
        };
        if handled {
            e.stop_propagation();
            e.prevent_default();
        }
    }

    fn next_tab(&self, back: bool) -> bool {
        let Some(focus) = &self.focus else {
            return false;
        };
        let count = self
            .data
            .workspace(self.ops.mesa)
            .and_then(|w| w.layout.find_leaf(focus.pane))
            .map_or(0, |p| p.tabs.len());
        if count <= 1 {
            return false;
        }
        let next = if back {
            (focus.index + count - 1) % count
        } else {
            (focus.index + 1) % count
        };
        self.ops.select_tab(focus.pane, next);
        true
    }
}
