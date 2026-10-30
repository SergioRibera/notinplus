//! What actions do. Every menu, button and shortcut that changes the
//! library or opens a sheet calls into here, so a behaviour lives in
//! one place whatever surface started it.

pub mod create;
pub mod forms;
pub mod pickers;
pub mod transfer;

use freya::prelude::*;
use notein_plugin::kind::ActionEffect;
use notein_ui_kit::{
    Button, ButtonVariant, ListRow, ModalController, Sheet, SheetFooter, Toast, ToastTone, Toasts,
};

use crate::library::{FolderId, ItemId, NotebookFate, TabTarget, WorkspaceId};
use crate::plugins::content;
use crate::space::OpenHint;
use crate::state;

fn toast(text: impl Into<String>) {
    Toasts::get().show(Toast::new(text.into()));
}

/// Open `item` in the other pane.
pub fn open_beside(item: ItemId) {
    crate::space::open(TabTarget::Item(item), OpenHint::Beside);
}

/// Open `item` alone in a new mesa.
pub fn open_in_new_mesa(item: ItemId) {
    crate::space::open(TabTarget::Item(item), OpenHint::NewSpace);
}

/// Open `item` where the user is.
pub fn open(item: ItemId) {
    crate::space::open(TabTarget::Item(item), OpenHint::Focused);
}

/// The notebooks sheet of `item`.
pub fn notebook(item: ItemId) {
    pickers::notebook_sheet(item);
}

/// Pick a document to link `item` with.
pub fn link_with(item: ItemId) {
    pickers::document_picker(pickers::Purpose::Link { from: item });
}

/// Pin or unpin `item`.
pub fn toggle_pin(item: ItemId, pin: bool) {
    if pin {
        state::edit(crate::state::LibraryEdit::Pin, async move |lib| {
            lib.pin_items(&[item]).await
        });
    } else {
        state::edit(crate::state::LibraryEdit::Unpin, async move |lib| {
            lib.unpin_items(&[item]).await
        });
    }
}

/// Pin several items at once.
pub fn pin_items(items: Vec<ItemId>) {
    let n = items.len();
    state::edit(crate::state::LibraryEdit::Pin, async move |lib| {
        lib.pin_items(&items).await
    });
    toast(crate::format::count(
        n,
        "documento fijado",
        "documentos fijados",
    ));
}

/// The share sheet for `item`.
pub fn share_item(item: ItemId) {
    crate::screens::collab::share(crate::screens::collab::ShareTarget {
        item: Some(item),
        mesa: None,
    });
}

/// The share sheet for a whole mesa.
pub fn share_mesa(workspace: WorkspaceId) {
    crate::screens::collab::share(crate::screens::collab::ShareTarget {
        item: None,
        mesa: Some(workspace),
    });
}

/// Export the given items.
pub fn export_items(items: Vec<ItemId>) {
    transfer::export(transfer::ExportScope::Items(items));
}

/// Export a folder and its contents.
pub fn export_folder(folder: FolderId) {
    transfer::export(transfer::ExportScope::Folder(folder));
}

/// Duplicate `item` next to it.
pub fn duplicate(item: ItemId) {
    spawn_in_window(async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let copied = handle.lock().await.duplicate_item(item).await;
        match copied {
            Ok(copy) => {
                Toasts::get().show(Toast::new("Duplicado").action("Abrir", move |()| open(copy)));
            }
            Err(err) => {
                log::error!("duplicate {item}: {err}");
                Toasts::get().show(Toast::new("No se pudo duplicar").tone(ToastTone::Error));
            }
        }
    });
}

/// Pick a folder to move `items` and `folders` into.
pub fn move_items(items: Vec<ItemId>, folders: Vec<FolderId>) {
    pickers::folder_picker(items, folders);
}

/// The edit sheet of `item`'s kind.
pub fn edit_item(item: ItemId) {
    forms::edit_item(item);
}

/// The edit sheet of a folder.
pub fn edit_folder(folder: FolderId) {
    forms::edit_folder(folder);
}

/// The edit sheet of a mesa.
pub fn edit_mesa(workspace: WorkspaceId) {
    forms::edit_mesa(workspace);
}

/// Run one of `item`'s kind actions and carry out its effect.
pub fn run_kind_action(item: ItemId, action: &str) {
    let action = action.to_owned();
    spawn_in_window(async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let Some(found) = handle.lock().await.item(item).cloned() else {
            return;
        };
        let Some(kind) = crate::plugins::kind(&found.kind.plugin_id) else {
            return;
        };
        let bytes = content::read(item, &found.kind.plugin_id).unwrap_or_default();
        let meta = content::meta(&found);
        match kind.run_action(&action, meta, &bytes) {
            ActionEffect::Done => {}
            ActionEffect::Copy {
                text,
                toast: message,
            } => match freya::clipboard::Clipboard::set(text) {
                Ok(()) => toast(message),
                Err(err) => {
                    log::warn!("clipboard: {err:?}");
                    Toasts::get().show(Toast::new("No se pudo copiar").tone(ToastTone::Error));
                }
            },
            ActionEffect::Share {
                file_name,
                mime,
                bytes,
            } => share_file(file_name, mime, bytes),
            ActionEffect::Edit => edit_item(item),
        }
    });
}

/// Hand a file to the system share sheet.
pub fn share_file(name: String, mime: String, bytes: Vec<u8>) {
    share(notein_plugin::Share::File {
        subject: name.clone(),
        name,
        mime,
        bytes,
    });
}

/// Hand `what` to the system share sheet.
pub fn share(what: notein_plugin::Share) {
    use istmo_share::{ShareClient, ShareFile, ShareRequest};
    // A menu that runs this closes first, unmounting the scope `spawn`
    // would tie the task to; dropping the task cancels the call.
    spawn_in_window(async move {
        let request = match what {
            notein_plugin::Share::Text { text, subject } => {
                ShareRequest::text(text).with_subject(subject)
            }
            notein_plugin::Share::File {
                name,
                mime,
                bytes,
                subject,
            } => ShareRequest::files([ShareFile::bytes(name, bytes).with_mime_type(mime)])
                .with_subject(subject),
        };
        let result = match ShareClient::acquire() {
            Ok(client) => client
                .share(request)
                .await
                .map_err(istmo_share::ShareError::from),
            Err(err) => Err(istmo_share::ShareError::from(err)),
        };
        if let Err(err) = result {
            log::warn!("share: {err}");
            Toasts::get().show(Toast::new("No se pudo compartir").tone(ToastTone::Error));
        }
    });
}

/// Toast for a move to the trash, with Deshacer.
fn trashed_toast(text: String, batch: u64) {
    Toasts::get().show(
        Toast::new(text)
            .tone(ToastTone::Trash)
            .action("Deshacer", move |()| {
                state::edit(crate::state::LibraryEdit::Restore, async move |lib| {
                    lib.restore_batch(batch).await.map(drop)
                });
            }),
    );
}

/// Move items to the trash in one step, with Deshacer.
pub fn trash_items(items: Vec<ItemId>, fate: NotebookFate) {
    spawn_in_window(async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let mut lib = handle.lock().await;
        let single = (items.len() == 1)
            .then(|| lib.item(items[0]).map(|i| i.name.clone()))
            .flatten();
        let closing: Vec<ItemId> = items.clone();
        match lib.trash_items(&items, fate).await {
            Ok(receipt) => {
                if let Err(err) = lib.close_item_tabs(&closing).await {
                    log::warn!("close trashed tabs: {err}");
                }
                drop(lib);
                let text = single.map_or_else(
                    || {
                        format!(
                            "{} se movieron a la papelera",
                            crate::format::count(receipt.items, "documento", "documentos")
                        )
                    },
                    |name| format!("«{name}» se movió a la papelera"),
                );
                trashed_toast(text, receipt.batch);
            }
            Err(err) => {
                log::error!("trash: {err}");
                Toasts::get()
                    .show(Toast::new("No se pudo mover a la papelera").tone(ToastTone::Error));
            }
        }
    });
}

/// Move `item` to the trash. Asks only when it has notebooks.
pub fn trash_item(item: ItemId) {
    trash_documents(vec![item]);
}

/// Move `items` to the trash. Asks once when any of them has notebooks.
pub fn trash_documents(items: Vec<ItemId>) {
    spawn_in_window(async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let (title, notebooks) = {
            let lib = handle.lock().await;
            let notebooks: usize = items.iter().map(|i| lib.live_notebooks_of(*i).len()).sum();
            let title = match items.as_slice() {
                [one] => match lib.item(*one) {
                    Some(found) => format!("Mover «{}» a la papelera", found.name),
                    None => return,
                },
                many => format!(
                    "Mover {} a la papelera",
                    crate::format::count(many.len(), "documento", "documentos")
                ),
            };
            drop(lib);
            (title, notebooks)
        };
        if notebooks == 0 {
            trash_items(items, NotebookFate::Keep);
            return;
        }
        ask_notebook_fate(items, title, notebooks);
    });
}

/// "Mover a la papelera" for documents with notebooks: one question,
/// keeping them preselected.
fn ask_notebook_fate(items: Vec<ItemId>, title: String, notebooks: usize) {
    use freya::icons::lucide;
    let fate = State::create_in_scope(NotebookFate::Keep, ScopeId::ROOT);
    let subtitle = if notebooks == 1 {
        "Tiene 1 cuaderno con tus apuntes.".to_owned()
    } else {
        format!("Tienen {notebooks} cuadernos con tus apuntes.")
    };
    Sheet::titled(title)
        .kind(lucide::trash_2(), notein_ui_kit::palette().danger)
        .subtitle(subtitle)
        .child(NotebookFateChoice { fate })
        .footer(SheetFooter::new([
            Button::new("Cancelar")
                .ghost()
                .on_press(|_| ModalController::get().close())
                .into_element(),
            Button::new("Mover a la papelera")
                .variant(ButtonVariant::DangerFill)
                .on_press(move |_| {
                    ModalController::get().close();
                    trash_items(items.clone(), *fate.peek());
                })
                .into_element(),
        ]))
        .open();
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct NotebookFateChoice {
    fate: State<NotebookFate>,
}

impl Component for NotebookFateChoice {
    fn render(&self) -> impl IntoElement {
        let mut fate = self.fate;
        let current = *fate.read();
        let row = move |value: NotebookFate, title: &'static str, sub: &'static str| {
            ListRow::new(title)
                .subtitle(sub)
                .leading(notein_ui_kit::Leading::Check(current == value))
                .selected(current == value)
                .on_press(move |_| fate.set(value))
                .into_element()
        };
        notein_ui_kit::Group::default()
            .child(row(
                NotebookFate::Keep,
                "Mantener los cuadernos",
                "Quedan en la biblioteca, sin vínculo.",
            ))
            .child(row(
                NotebookFate::Trash,
                "Mandarlos también a la papelera",
                "Se pueden restaurar juntos.",
            ))
    }
}

/// Move a folder and everything in it to the trash, with Deshacer.
pub fn trash_folder(folder: FolderId) {
    spawn_in_window(async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let mut lib = handle.lock().await;
        let name = lib
            .folder(folder)
            .map(|f| f.name.clone())
            .unwrap_or_default();
        match lib.trash_folder(folder).await {
            Ok(receipt) => {
                drop(lib);
                let text = match receipt.items {
                    0 => format!("«{name}» se movió a la papelera"),
                    n => format!(
                        "«{name}» y {} se movieron a la papelera",
                        crate::format::count(n, "documento", "documentos")
                    ),
                };
                trashed_toast(text, receipt.batch);
            }
            Err(err) => {
                log::error!("trash folder: {err}");
                Toasts::get()
                    .show(Toast::new("No se pudo mover a la papelera").tone(ToastTone::Error));
            }
        }
    });
}

/// Show or stop showing a mesa first in Home.
pub fn pin_mesa(workspace: WorkspaceId, pin: bool) {
    state::edit(crate::state::LibraryEdit::PinDesk, async move |lib| {
        lib.set_workspace_pinned(workspace, pin).await
    });
}

/// Copy a mesa with the same documents.
pub fn duplicate_mesa(workspace: WorkspaceId) {
    state::edit(crate::state::LibraryEdit::DuplicateDesk, async move |lib| {
        lib.duplicate_workspace(workspace).await.map(drop)
    });
}

/// Delete a mesa after one confirmation; its documents stay.
pub fn delete_mesa(workspace: WorkspaceId) {
    spawn_in_window(async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let title = handle.lock().await.workspace_title(workspace);
        Sheet::titled(format!("¿Borrar la mesa «{title}»?"))
            .subtitle(
                "Los documentos quedan en la biblioteca. Solo se cierra este grupo de pestañas.",
            )
            .footer(SheetFooter::new([
                Button::new("Cancelar")
                    .ghost()
                    .on_press(|_| ModalController::get().close())
                    .into_element(),
                Button::new("Borrar mesa")
                    .variant(ButtonVariant::DangerFill)
                    .on_press(move |_| {
                        ModalController::get().close();
                        state::edit(crate::state::LibraryEdit::DeleteDesk, async move |lib| {
                            lib.delete_workspace(workspace).await
                        });
                    })
                    .into_element(),
            ]))
            .open();
    });
}
