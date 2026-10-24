//! The one list of document actions: long press, right click, the
//! header's `⋯` and the command palette all show it, filtered by where
//! it was asked from.

use freya::prelude::*;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{Action, ActionEntry, ActionList, ModalController, Popover};

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

fn run(f: impl Fn() + 'static) -> EventHandler<()> {
    EventHandler::new(move |()| {
        ModalController::get().close();
        f();
    })
}

/// Actions for document `item`.
#[must_use]
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
            Action::new("Abrir al lado", run(move || flows::open_beside(item)))
                .icon(lucide::columns_2())
                .shortcut("Ctrl \\"),
        ));
        entries.push(ActionEntry::Item(
            Action::new(
                "Abrir en mesa nueva",
                run(move || flows::open_in_new_mesa(item)),
            )
            .icon(lucide::layout_grid()),
        ));
    }
    let notebooks = data.live_notebooks_of(item);
    if !found.kind.is(crate::plugins::builtin::canvas::KIND) {
        entries.push(ActionEntry::Item(
            Action::new(
                if notebooks.is_empty() {
                    "Nuevo cuaderno"
                } else {
                    "Cuaderno"
                },
                run(move || flows::notebook(item)),
            )
            .icon(lucide::notebook_pen())
            .submenu(),
        ));
    }
    entries.push(ActionEntry::Item(
        Action::new("Vincular con…", run(move || flows::link_with(item))).icon(lucide::link_2()),
    ));
    entries.push(ActionEntry::Divider);
    entries.push(ActionEntry::Item(
        Action::new(
            if pinned { "Quitar de fijados" } else { "Fijar" },
            run(move || flows::toggle_pin(item, !pinned)),
        )
        .icon(if pinned {
            lucide::pin_off()
        } else {
            lucide::pin()
        }),
    ));
    entries.push(ActionEntry::Item(
        Action::new("Compartir…", run(move || flows::share_item(item))).icon(lucide::share_2()),
    ));
    entries.push(ActionEntry::Item(
        Action::new("Exportar…", run(move || flows::export_items(vec![item])))
            .icon(lucide::download()),
    ));
    entries.push(ActionEntry::Item(
        Action::new("Duplicar", run(move || flows::duplicate(item))).icon(lucide::copy()),
    ));
    entries.push(ActionEntry::Item(
        Action::new(
            "Mover a…",
            run(move || flows::move_items(vec![item], Vec::new())),
        )
        .icon(lucide::folder_input()),
    ));
    entries.push(ActionEntry::Item(
        Action::new("Editar…", run(move || flows::edit_item(item))).icon(lucide::square_pen()),
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
        Action::new("Mover a la papelera", run(move || flows::trash_item(item)))
            .icon(lucide::trash_2())
            .danger(),
    ));
    entries
}

/// Actions for folder `folder`.
#[must_use]
pub fn for_folder(folder: FolderId) -> Vec<ActionEntry> {
    use freya::icons::lucide;
    let mut entries = Vec::new();
    if let Some(receipt) = flows::transfer::undoable_import(folder) {
        entries.push(ActionEntry::Item(
            Action::new(
                "Deshacer la importación",
                run(move || flows::transfer::undo_import(receipt.clone())),
            )
            .icon(lucide::undo_2()),
        ));
        entries.push(ActionEntry::Divider);
    }
    entries.extend([
        ActionEntry::Item(
            Action::new("Editar…", run(move || flows::edit_folder(folder)))
                .icon(lucide::square_pen()),
        ),
        ActionEntry::Item(
            Action::new(
                "Mover a…",
                run(move || flows::move_items(Vec::new(), vec![folder])),
            )
            .icon(lucide::folder_input()),
        ),
        ActionEntry::Item(
            Action::new("Exportar…", run(move || flows::export_folder(folder)))
                .icon(lucide::download()),
        ),
        ActionEntry::Divider,
        ActionEntry::Item(
            Action::new(
                "Mover a la papelera",
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
pub fn for_mesa(data: &LibraryData, workspace: WorkspaceId) -> Vec<ActionEntry> {
    use freya::icons::lucide;
    let pinned = data.workspace(workspace).is_some_and(|w| w.pinned);
    vec![
        ActionEntry::Item(
            Action::new("Abrir", run(move || crate::space::open_space(workspace)))
                .icon(lucide::layout_grid()),
        ),
        ActionEntry::Item(
            Action::new("Editar…", run(move || flows::edit_mesa(workspace)))
                .icon(lucide::square_pen()),
        ),
        ActionEntry::Item(
            Action::new(
                if pinned {
                    "Quitar de Home"
                } else {
                    "Fijar en Home"
                },
                run(move || flows::pin_mesa(workspace, !pinned)),
            )
            .icon(lucide::pin()),
        ),
        ActionEntry::Item(
            Action::new("Duplicar", run(move || flows::duplicate_mesa(workspace)))
                .icon(lucide::copy()),
        ),
        ActionEntry::Item(
            Action::new("Compartir mesa…", run(move || flows::share_mesa(workspace)))
                .icon(lucide::share_2()),
        ),
        ActionEntry::Divider,
        ActionEntry::Item(
            Action::new("Borrar mesa", run(move || flows::delete_mesa(workspace)))
                .icon(lucide::trash_2())
                .danger(),
        ),
    ]
}

/// Show `entries` hanging from the pointer at `at`.
pub fn show(entries: Vec<ActionEntry>, at: CursorPoint) {
    #[allow(clippy::cast_possible_truncation)] // window coordinates
    let anchor = Area::new((at.x as f32, at.y as f32).into(), (1.0, 1.0).into());
    let height = entries
        .iter()
        .map(|e| match e {
            ActionEntry::Item(_) => 36.0,
            ActionEntry::Divider => 9.0,
            ActionEntry::Header(_) => 28.0,
        })
        .sum::<f32>()
        + 12.0;
    Popover::new(anchor, ActionList::new(entries))
        .width(280.0)
        .estimated_height(height)
        .open();
}
