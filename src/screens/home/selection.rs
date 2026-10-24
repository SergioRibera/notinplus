//! Picking several documents and folders at once, and the bar that acts
//! on them in place of the + button.

use freya::prelude::*;
use notein_ui_kit::{BarItem, SelectionBar};

use super::card::Selecting;
use crate::flows;
use crate::library::{FolderId, ItemId};

/// What is selected in Home.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Selection {
    pub active: bool,
    pub items: Vec<ItemId>,
    pub folders: Vec<FolderId>,
}

fn toggle<T: PartialEq>(list: &mut Vec<T>, value: T) {
    if let Some(at) = list.iter().position(|v| *v == value) {
        list.remove(at);
    } else {
        list.push(value);
    }
}

impl Selection {
    /// Start selecting, with `first` already picked.
    pub fn start_with_item(&mut self, first: ItemId) {
        self.active = true;
        if !self.items.contains(&first) {
            self.items.push(first);
        }
    }

    pub fn toggle_item(&mut self, item: ItemId) {
        toggle(&mut self.items, item);
    }

    pub fn toggle_folder(&mut self, folder: FolderId) {
        toggle(&mut self.folders, folder);
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.items.len() + self.folders.len()
    }

    #[must_use]
    pub fn of_item(&self, item: ItemId) -> Selecting {
        match (self.active, self.items.contains(&item)) {
            (false, _) => Selecting::No,
            (true, true) => Selecting::Selected,
            (true, false) => Selecting::Unselected,
        }
    }

    #[must_use]
    pub fn of_folder(&self, folder: FolderId) -> Selecting {
        match (self.active, self.folders.contains(&folder)) {
            (false, _) => Selecting::No,
            (true, true) => Selecting::Selected,
            (true, false) => Selecting::Unselected,
        }
    }
}

/// The bar shown while selecting.
#[derive(Clone, Copy, PartialEq)]
pub struct SelectionActions {
    pub selection: State<Selection>,
    pub folder: FolderId,
}

impl std::fmt::Debug for SelectionActions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectionActions").finish_non_exhaustive()
    }
}

impl Component for SelectionActions {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let selection = self.selection;
        let current = selection.read().clone();
        let folder = self.folder;
        let done = move || {
            let mut selection = selection;
            selection.write().clear();
        };
        let items = current.items.clone();
        let folders = current.folders.clone();
        let has_docs = !items.is_empty();
        let caption = if current.count() == 0 {
            "Tocá para elegir".to_owned()
        } else {
            crate::format::count(current.count(), "seleccionado", "seleccionados")
        };
        let mut bar = vec![
            BarItem::icon(lucide::x(), move |()| done()),
            BarItem::Caption(caption.into()),
            BarItem::Divider,
        ];
        if current.count() > 0 {
            let (move_items, move_folders) = (items.clone(), folders.clone());
            bar.push(BarItem::action(
                "Mover",
                lucide::folder_input(),
                move |()| {
                    flows::move_items(move_items.clone(), move_folders.clone());
                    done();
                },
            ));
        }
        if has_docs {
            let pin = items.clone();
            bar.push(BarItem::action("Fijar", lucide::pin(), move |()| {
                flows::pin_items(pin.clone());
                done();
            }));
            if let [one] = items.as_slice() {
                let one = *one;
                bar.push(BarItem::action("Compartir", lucide::share_2(), move |()| {
                    flows::share_item(one);
                    done();
                }));
            }
            let export = items.clone();
            bar.push(BarItem::action("Exportar", lucide::download(), move |()| {
                flows::export_items(export.clone());
                done();
            }));
            let group = items.clone();
            bar.push(BarItem::action(
                format!("Nueva carpeta con {}", items.len()),
                lucide::folder_plus(),
                move |()| {
                    flows::create::new_folder(flows::create::Context {
                        folder,
                        selection: group.clone(),
                    });
                    done();
                },
            ));
        }
        if current.count() > 0 {
            bar.push(BarItem::Divider);
            bar.push(BarItem::action("Papelera", lucide::trash_2(), move |()| {
                if !items.is_empty() {
                    flows::trash_documents(items.clone());
                }
                for f in &folders {
                    flows::trash_folder(*f);
                }
                done();
            }));
        }
        SelectionBar::new(bar)
    }
}
