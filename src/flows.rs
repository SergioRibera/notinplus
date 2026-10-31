//! What actions do. Every menu, button and shortcut that changes the
//! library or opens a sheet calls into here, so a behaviour lives in
//! one place whatever surface started it.

pub mod create;
pub mod forms;
pub mod pickers;
pub mod transfer;

use freya::prelude::*;
use notein_plugin::UserMessage;
use notein_plugin::kind::ActionEffect;
use notein_ui_kit::{
    Button, ButtonVariant, ListRow, ModalController, Sheet, SheetFooter, Toast, ToastTone, Toasts,
};

use notein_i18n::tr;

use crate::library::{FolderId, ItemId, NotebookFate, TabTarget, WorkspaceId};
use crate::plugins::content;
use crate::space::OpenHint;
use crate::state;

fn toast(text: impl Into<String>) {
    Toasts::get().show(Toast::new(text.into()));
}

/// Open `item` in the other pane.
#[sismo::function("flows::open_beside")]
pub fn open_beside(item: ItemId) {
    crate::space::open(TabTarget::Item(item), OpenHint::Beside);
}

/// Open `item` alone in a new mesa.
#[sismo::function("flows::open_in_new_mesa")]
pub fn open_in_new_mesa(item: ItemId) {
    crate::space::open(TabTarget::Item(item), OpenHint::NewSpace);
}

/// Open `item` where the user is.
#[sismo::function("flows::open")]
pub fn open(item: ItemId) {
    crate::space::open(TabTarget::Item(item), OpenHint::Focused);
}

/// The notebooks sheet of `item`.
#[sismo::function("flows::notebook")]
pub fn notebook(item: ItemId) {
    pickers::notebook_sheet(item);
}

/// Pick a document to link `item` with.
#[sismo::function("flows::link_with")]
pub fn link_with(item: ItemId) {
    pickers::document_picker(pickers::Purpose::Link { from: item });
}

/// Pin or unpin `item`.
#[sismo::function("flows::toggle_pin")]
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
#[sismo::function("flows::pin_items")]
pub fn pin_items(items: Vec<ItemId>) {
    let n = items.len();
    state::edit(crate::state::LibraryEdit::Pin, async move |lib| {
        lib.pin_items(&items).await
    });
    toast(tr!("flows-pinned-documents", "count" => n));
}

/// The share sheet for `item`.
#[sismo::function("flows::share_item")]
pub fn share_item(item: ItemId) {
    crate::screens::collab::share(crate::screens::collab::ShareTarget {
        item: Some(item),
        mesa: None,
    });
}

/// The share sheet for a whole mesa.
#[sismo::function("flows::share_mesa")]
pub fn share_mesa(workspace: WorkspaceId) {
    crate::screens::collab::share(crate::screens::collab::ShareTarget {
        item: None,
        mesa: Some(workspace),
    });
}

/// Export the given items.
#[sismo::function("flows::export_items")]
pub fn export_items(items: Vec<ItemId>) {
    transfer::export(transfer::ExportScope::Items(items));
}

/// Export a folder and its contents.
#[sismo::function("flows::export_folder")]
pub fn export_folder(folder: FolderId) {
    transfer::export(transfer::ExportScope::Folder(folder));
}

/// Duplicate `item` next to it.
#[sismo::function("flows::duplicate")]
pub fn duplicate(item: ItemId) {
    spawn_in_window(traced!("task::flows::duplicate", async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let copied = handle.lock().await.duplicate_item(item).await;
        match copied {
            Ok(copy) => {
                Toasts::get().show(
                    Toast::new(tr!("flows-duplicated"))
                        .action(tr!("common-open"), move |()| open(copy)),
                );
            }
            Err(err) => {
                log::error!("duplicate {item}: {err}");
                Toasts::get()
                    .show(Toast::new(tr!("errors-headline-detail", "headline" => tr!("flows-duplicate-failed"), "detail" => err.user_message())).tone(ToastTone::Error));
            }
        }
    }));
}

/// Pick a folder to move `items` and `folders` into.
#[sismo::function("flows::move_items")]
pub fn move_items(items: Vec<ItemId>, folders: Vec<FolderId>) {
    pickers::folder_picker(items, folders);
}

/// The edit sheet of `item`'s kind.
#[sismo::function("flows::edit_item")]
pub fn edit_item(item: ItemId) {
    forms::edit_item(item);
}

/// The edit sheet of a folder.
#[sismo::function("flows::edit_folder")]
pub fn edit_folder(folder: FolderId) {
    forms::edit_folder(folder);
}

/// The edit sheet of a mesa.
#[sismo::function("flows::edit_mesa")]
pub fn edit_mesa(workspace: WorkspaceId) {
    forms::edit_mesa(workspace);
}

/// Run one of `item`'s kind actions and carry out its effect.
#[sismo::function("flows::run_kind_action")]
pub fn run_kind_action(item: ItemId, action: &str) {
    let action = action.to_owned();
    spawn_in_window(traced!("task::flows::kind_action", async move {
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
                    Toasts::get().show(Toast::new(tr!("flows-copy-failed")).tone(ToastTone::Error));
                }
            },
            ActionEffect::Share {
                file_name,
                mime,
                bytes,
            } => share_file(file_name, mime, bytes),
            ActionEffect::Edit => edit_item(item),
        }
    }));
}

/// Hand a file to the system share sheet.
#[sismo::function("flows::share_file")]
pub fn share_file(name: String, mime: String, bytes: Vec<u8>) {
    share(notein_plugin::Share::File {
        subject: name.clone(),
        name,
        mime,
        bytes,
    });
}

/// Hand `what` to the system share sheet.
#[sismo::function("flows::share")]
pub fn share(what: notein_plugin::Share) {
    use istmo_share::{ShareClient, ShareFile, ShareRequest};
    // A menu that runs this closes first, unmounting the scope `spawn`
    // would tie the task to; dropping the task cancels the call.
    spawn_in_window(traced!("task::flows::share", async move {
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
            Toasts::get().show(Toast::new(tr!("flows-share-failed")).tone(ToastTone::Error));
        }
    }));
}

/// Toast for a move to the trash, with Undo.
#[sismo::function("flows::trashed_toast")]
fn trashed_toast(text: String, batch: u64) {
    Toasts::get().show(Toast::new(text).tone(ToastTone::Trash).action(
        tr!("flows-undo"),
        move |()| {
            state::edit(crate::state::LibraryEdit::Restore, async move |lib| {
                lib.restore_batch(batch).await.map(drop)
            });
        },
    ));
}

/// Move items to the trash in one step, with Undo.
#[sismo::function("flows::trash_items")]
pub fn trash_items(items: Vec<ItemId>, fate: NotebookFate) {
    spawn_in_window(traced!("task::flows::trash_items", async move {
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
                    || tr!("flows-trashed-documents", "count" => receipt.items),
                    |name| tr!("flows-trashed-named", "name" => name),
                );
                trashed_toast(text, receipt.batch);
            }
            Err(err) => {
                log::error!("trash: {err}");
                Toasts::get().show(Toast::new(tr!("errors-headline-detail", "headline" => tr!("flows-trash-failed"), "detail" => err.user_message())).tone(ToastTone::Error));
            }
        }
    }));
}

/// Move `item` to the trash. Asks only when it has notebooks.
#[sismo::function("flows::trash_item")]
pub fn trash_item(item: ItemId) {
    trash_documents(vec![item]);
}

/// Move `items` to the trash. Asks once when any of them has notebooks.
#[sismo::function("flows::trash_documents")]
pub fn trash_documents(items: Vec<ItemId>) {
    spawn_in_window(traced!("task::flows::trash_documents", async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let (title, notebooks) = {
            let lib = handle.lock().await;
            let notebooks: usize = items.iter().map(|i| lib.live_notebooks_of(*i).len()).sum();
            let title = match items.as_slice() {
                [one] => match lib.item(*one) {
                    Some(found) => tr!("flows-trash-title-named", "name" => found.name.clone()),
                    None => return,
                },
                many => tr!("flows-trash-title-documents", "count" => many.len()),
            };
            drop(lib);
            (title, notebooks)
        };
        if notebooks == 0 {
            trash_items(items, NotebookFate::Keep);
            return;
        }
        ask_notebook_fate(items, title, notebooks);
    }));
}

/// "Move to trash" for documents with notebooks: one question,
/// keeping them preselected.
#[sismo::function("flows::ask_notebook_fate")]
fn ask_notebook_fate(items: Vec<ItemId>, title: String, notebooks: usize) {
    use freya::icons::lucide;
    let fate = State::create_in_scope(NotebookFate::Keep, ScopeId::ROOT);
    let subtitle = tr!("flows-trash-notebooks", "count" => notebooks);
    Sheet::titled(title)
        .kind(lucide::trash_2(), notein_ui_kit::palette().danger)
        .subtitle(subtitle)
        .child(NotebookFateChoice { fate })
        .footer(SheetFooter::new([
            Button::new(tr!("common-cancel"))
                .ghost()
                .on_press(|_| ModalController::get().close())
                .into_element(),
            Button::new(tr!("common-move-to-trash"))
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
    #[sismo::function("flows::notebook_fate_choice")]
    fn render(&self) -> impl IntoElement {
        let mut fate = self.fate;
        let current = *fate.read();
        let row = move |value: NotebookFate, title: String, sub: String| {
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
                tr!("flows-fate-keep"),
                tr!("flows-fate-keep-hint"),
            ))
            .child(row(
                NotebookFate::Trash,
                tr!("flows-fate-trash"),
                tr!("flows-fate-trash-hint"),
            ))
    }
}

/// Move a folder and everything in it to the trash, with Undo.
#[sismo::function("flows::trash_folder")]
pub fn trash_folder(folder: FolderId) {
    spawn_in_window(traced!("task::flows::trash_folder", async move {
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
                    0 => tr!("flows-trashed-named", "name" => name),
                    n => tr!("flows-trashed-folder", "name" => name, "count" => n),
                };
                trashed_toast(text, receipt.batch);
            }
            Err(err) => {
                log::error!("trash folder: {err}");
                Toasts::get().show(Toast::new(tr!("errors-headline-detail", "headline" => tr!("flows-trash-failed"), "detail" => err.user_message())).tone(ToastTone::Error));
            }
        }
    }));
}

/// Show or stop showing a mesa first in Home.
#[sismo::function("flows::pin_mesa")]
pub fn pin_mesa(workspace: WorkspaceId, pin: bool) {
    state::edit(crate::state::LibraryEdit::PinDesk, async move |lib| {
        lib.set_workspace_pinned(workspace, pin).await
    });
}

/// Copy a mesa with the same documents.
#[sismo::function("flows::duplicate_mesa")]
pub fn duplicate_mesa(workspace: WorkspaceId) {
    state::edit(crate::state::LibraryEdit::DuplicateDesk, async move |lib| {
        lib.duplicate_workspace(workspace).await.map(drop)
    });
}

/// Delete a mesa after one confirmation; its documents stay.
#[sismo::function("flows::delete_mesa")]
pub fn delete_mesa(workspace: WorkspaceId) {
    spawn_in_window(traced!("task::flows::delete_mesa", async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let title = handle.lock().await.workspace_title(workspace);
        Sheet::titled(tr!("flows-delete-desk-title", "title" => title))
            .subtitle(tr!("flows-delete-desk-hint"))
            .footer(SheetFooter::new([
                Button::new(tr!("common-cancel"))
                    .ghost()
                    .on_press(|_| ModalController::get().close())
                    .into_element(),
                Button::new(tr!("flows-delete-desk"))
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
    }));
}
