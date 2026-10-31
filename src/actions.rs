//! The one list of document actions: long press, right click, the
//! header's `⋯` and the command palette all show it, filtered by where
//! it was asked from.

use freya::prelude::*;
use notein_i18n::tr;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{Action, ActionEntry, ActionList, ModalController};

use crate::flows;
use crate::library::{FolderId, ItemId, LibraryData, WorkspaceId};
use crate::references::kind_color;

/// Where the list was asked from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A card in Home.
    Library,
    /// The document's own header inside a mesa.
    Header,
}

/// A press handler that closes the menu, then runs `f`.
#[sismo::function("actions::run")]
pub(crate) fn run(f: impl Fn() + 'static) -> EventHandler<()> {
    EventHandler::new(move |()| {
        ModalController::get().close();
        f();
    })
}

/// Actions for document `item`.
#[must_use]
#[allow(clippy::too_many_lines)] // one flat list of menu entries
#[sismo::function("actions::for_item")]
pub fn for_item(data: &LibraryData, item: ItemId, origin: Origin) -> Vec<ActionEntry> {
    use freya::icons::lucide;
    let Some(found) = data.item(item) else {
        return Vec::new();
    };
    let p = palette();
    let pinned = data.is_pinned(item);
    let kind = crate::plugins::kind(&found.kind.plugin_id);
    let mut entries = Vec::new();
    if origin == Origin::Library {
        entries.push(ActionEntry::Item(
            Action::new(
                tr!("actions-open-beside"),
                run(move || flows::open_beside(item)),
            )
            .icon(lucide::columns_2())
            .shortcut("Ctrl \\"),
        ));
        entries.push(ActionEntry::Item(
            Action::new(
                tr!("actions-open-new-desk"),
                run(move || flows::open_in_new_mesa(item)),
            )
            .icon(lucide::layout_grid()),
        ));
    }
    let notebooks = data.live_notebooks_of(item);
    let is_notebook = crate::plugins::kind_info(&found.kind.plugin_id).is_some_and(|k| k.notebook);
    if !is_notebook {
        entries.push(ActionEntry::Item(
            Action::new(
                if notebooks.is_empty() {
                    tr!("actions-new-notebook")
                } else {
                    tr!("actions-notebook")
                },
                run(move || flows::notebook(item)),
            )
            .icon(lucide::notebook_pen())
            .submenu(),
        ));
    }
    entries.push(ActionEntry::Item(
        Action::new(
            tr!("actions-link-with"),
            run(move || flows::link_with(item)),
        )
        .icon(lucide::link_2()),
    ));
    entries.push(ActionEntry::Divider);
    entries.push(ActionEntry::Item(
        Action::new(
            if pinned {
                tr!("actions-unpin")
            } else {
                tr!("actions-pin")
            },
            run(move || flows::toggle_pin(item, !pinned)),
        )
        .icon(if pinned {
            lucide::pin_off()
        } else {
            lucide::pin()
        }),
    ));
    entries.push(ActionEntry::Item(
        Action::new(tr!("actions-share"), run(move || flows::share_item(item)))
            .icon(lucide::share_2()),
    ));
    entries.push(ActionEntry::Item(
        Action::new(
            tr!("actions-export"),
            run(move || flows::export_items(vec![item])),
        )
        .icon(lucide::download()),
    ));
    entries.push(ActionEntry::Item(
        Action::new(
            tr!("actions-duplicate"),
            run(move || flows::duplicate(item)),
        )
        .icon(lucide::copy()),
    ));
    entries.push(ActionEntry::Item(
        Action::new(
            tr!("actions-move-to"),
            run(move || flows::move_items(vec![item], Vec::new())),
        )
        .icon(lucide::folder_input()),
    ));
    entries.push(ActionEntry::Item(
        Action::new(tr!("actions-edit"), run(move || flows::edit_item(item)))
            .icon(lucide::square_pen()),
    ));
    if let Some(kind) = kind {
        let info = kind.info();
        let actions = kind.actions();
        if !actions.is_empty() {
            entries.push(ActionEntry::Divider);
            entries.push(ActionEntry::Header(info.label.clone()));
            let color = kind_color(&p, info.color);
            for action in actions {
                let id = action.id.to_string();
                entries.push(ActionEntry::Item(
                    Action::new(action.label, run(move || flows::run_kind_action(item, &id)))
                        .icon(action.icon)
                        .kind_color(color),
                ));
            }
        }
    }
    entries.push(ActionEntry::Divider);
    entries.push(ActionEntry::Item(
        Action::new(
            tr!("common-move-to-trash"),
            run(move || flows::trash_item(item)),
        )
        .icon(lucide::trash_2())
        .danger(),
    ));
    entries
}

/// Actions for folder `folder`.
#[must_use]
#[sismo::function("actions::for_folder")]
pub fn for_folder(folder: FolderId) -> Vec<ActionEntry> {
    use freya::icons::lucide;
    let mut entries = Vec::new();
    if let Some(receipt) = flows::transfer::undoable_import(folder) {
        entries.push(ActionEntry::Item(
            Action::new(
                tr!("actions-undo-import"),
                run(move || flows::transfer::undo_import(receipt.clone())),
            )
            .icon(lucide::undo_2()),
        ));
        entries.push(ActionEntry::Divider);
    }
    entries.extend([
        ActionEntry::Item(
            Action::new(tr!("actions-edit"), run(move || flows::edit_folder(folder)))
                .icon(lucide::square_pen()),
        ),
        ActionEntry::Item(
            Action::new(
                tr!("actions-move-to"),
                run(move || flows::move_items(Vec::new(), vec![folder])),
            )
            .icon(lucide::folder_input()),
        ),
        ActionEntry::Item(
            Action::new(
                tr!("actions-export"),
                run(move || flows::export_folder(folder)),
            )
            .icon(lucide::download()),
        ),
        ActionEntry::Divider,
        ActionEntry::Item(
            Action::new(
                tr!("common-move-to-trash"),
                run(move || flows::trash_folder(folder)),
            )
            .icon(lucide::trash_2())
            .danger(),
        ),
    ]);
    entries
}

/// Actions for mesa `workspace`.
#[must_use]
#[sismo::function("actions::for_mesa")]
pub fn for_mesa(data: &LibraryData, workspace: WorkspaceId) -> Vec<ActionEntry> {
    use freya::icons::lucide;
    let pinned = data.workspace(workspace).is_some_and(|w| w.pinned);
    vec![
        ActionEntry::Item(
            Action::new(
                tr!("common-open"),
                run(move || crate::space::open_space(workspace)),
            )
            .icon(lucide::layout_grid()),
        ),
        ActionEntry::Item(
            Action::new(
                tr!("actions-edit"),
                run(move || flows::edit_mesa(workspace)),
            )
            .icon(lucide::square_pen()),
        ),
        ActionEntry::Item(
            Action::new(
                if pinned {
                    tr!("actions-unpin-home")
                } else {
                    tr!("actions-pin-home")
                },
                run(move || flows::pin_mesa(workspace, !pinned)),
            )
            .icon(lucide::pin()),
        ),
        ActionEntry::Item(
            Action::new(
                tr!("actions-duplicate"),
                run(move || flows::duplicate_mesa(workspace)),
            )
            .icon(lucide::copy()),
        ),
        ActionEntry::Item(
            Action::new(
                tr!("actions-share-desk"),
                run(move || flows::share_mesa(workspace)),
            )
            .icon(lucide::share_2()),
        ),
        ActionEntry::Divider,
        ActionEntry::Item(
            Action::new(
                tr!("actions-delete-desk"),
                run(move || flows::delete_mesa(workspace)),
            )
            .icon(lucide::trash_2())
            .danger(),
        ),
    ]
}

/// Show `entries` hanging from the pointer at `at`.
#[sismo::function("actions::show")]
pub fn show(entries: Vec<ActionEntry>, at: CursorPoint) {
    ActionList::new(entries).show(at);
}
