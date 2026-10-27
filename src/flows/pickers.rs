//! Choosing another document or a folder: one selector with search,
//! recents and creation shortcuts behind "Vincular con…", the notebook
//! sheet, a new tab and "Mover a…".

use std::rc::Rc;

use freya::prelude::*;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Button, EmptyState, Group, ListRow, ModalController, SearchField, Section, Sheet, SheetFooter,
    Tag, Toast, Toasts, Trailing,
};

use crate::library::{FolderId, ItemId, LinkKind, PaneId, ROOT_FOLDER, TabTarget};
use crate::references::kind_color;
use crate::space::OpenHint;
use crate::state::LibraryView;

/// Why the document selector is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// Link the picked document with `from`.
    Link { from: ItemId },
    /// Make the picked canvas a notebook of `of`.
    UseAsNotebook { of: ItemId },
    /// Pick which document gets a new notebook.
    NotebookFor,
    /// Open the picked document in a new tab of `pane` (or the focused
    /// pane).
    NewTab { pane: Option<PaneId> },
}

impl Purpose {
    const fn title(self) -> &'static str {
        match self {
            Self::Link { .. } => "Vincular con…",
            Self::UseAsNotebook { .. } => "Usar un lienzo existente",
            Self::NotebookFor => "¿Cuaderno de qué documento?",
            Self::NewTab { .. } => "Abrir en pestaña nueva",
        }
    }

    const fn excluded(self) -> Option<ItemId> {
        match self {
            Self::Link { from } => Some(from),
            Self::UseAsNotebook { of } => Some(of),
            Self::NotebookFor | Self::NewTab { .. } => None,
        }
    }

    /// The kind every pick must be: notebooks are made of one.
    fn only_kind(self) -> Option<String> {
        match self {
            Self::UseAsNotebook { .. } => crate::plugins::catalog()
                .notebook_kind()
                .map(|kind| kind.info().id.into_owned()),
            Self::Link { .. } | Self::NotebookFor | Self::NewTab { .. } => None,
        }
    }

    fn pick(self, item: ItemId) {
        match self {
            Self::Link { from } => link(from, item, LinkKind::Related),
            Self::UseAsNotebook { of } => link(of, item, LinkKind::Companion),
            Self::NotebookFor => crate::notebooks::new_notebook(item),
            Self::NewTab { pane } => crate::space::open(
                TabTarget::Item(item),
                pane.map_or(OpenHint::Focused, OpenHint::InPane),
            ),
        }
    }
}

fn link(from: ItemId, to: ItemId, kind: LinkKind) {
    spawn_in_window(async move {
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let linked = handle.lock().await.create_link(from, to, kind).await;
        match linked {
            Ok(_) if kind == LinkKind::Companion => {
                crate::space::open(TabTarget::Item(to), OpenHint::Beside);
            }
            Ok(_) => {
                Toasts::get().show(
                    Toast::new("Vinculado")
                        .action("Abrir al lado", move |()| super::open_beside(to)),
                );
            }
            Err(err) => {
                log::error!("link: {err}");
                Toasts::get()
                    .show(Toast::new("No se pudo vincular").tone(notein_ui_kit::ToastTone::Error));
            }
        }
    });
}

/// Open the document selector for `purpose`.
pub fn document_picker(purpose: Purpose) {
    Sheet::titled(purpose.title())
        .child(DocumentPicker { purpose })
        .open();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DocumentPicker {
    purpose: Purpose,
}

impl Component for DocumentPicker {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let query = use_state(String::new);
        let mut filter = use_state(|| Option::<String>::None);
        let purpose = self.purpose;
        let Some(data) = LibraryView::get().data() else {
            return rect().into_element();
        };
        let needle = query.read().trim().to_lowercase();
        let only = purpose.only_kind().or_else(|| filter.read().clone());
        let mut items: Vec<_> = data
            .live_items()
            .filter(|i| Some(i.id) != purpose.excluded())
            .filter(|i| only.as_deref().is_none_or(|k| i.kind.is(k)))
            .filter(|i| needle.is_empty() || i.name.to_lowercase().contains(&needle))
            .cloned()
            .collect();
        items.sort_by_key(|i| std::cmp::Reverse(i.updated_at));
        items.truncate(40);
        let now = crate::format::now();
        let kinds: Vec<_> = crate::plugins::catalog()
            .kinds()
            .iter()
            .map(|k| k.info())
            .collect();
        let current_filter = filter.read().clone();
        let typed = query.read().trim().to_owned();
        let rows = items.into_iter().map(|item| {
            let info = crate::plugins::kind_info(&item.kind.plugin_id);
            let label = info
                .as_ref()
                .map_or_else(|| "Documento".to_owned(), |k| k.label.to_string());
            let mut row = ListRow::new(item.name.clone())
                .subtitle(format!(
                    "{label} · {}",
                    crate::format::day(item.updated_at, now)
                ))
                .on_press(move |_| {
                    ModalController::get().close();
                    purpose.pick(item.id);
                });
            if let Some(info) = info {
                row = row.kind(info.icon.clone(), kind_color(&p, info.color));
            }
            row.into_element()
        });
        let rows: Vec<Element> = rows.collect();
        let empty = rows.is_empty();
        rect()
            .width(Size::fill())
            .spacing(12.0)
            .child(SearchField::new(query, "Buscar en la biblioteca").auto_focus(true))
            .maybe_child(purpose.only_kind().is_none().then(|| {
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::wrap_spacing(6.0))
                    .spacing(6.0)
                    .child(
                        Tag::new("Todo")
                            .active(current_filter.is_none())
                            .on_press(move |_| filter.set(None)),
                    )
                    .children(kinds.iter().map(|k| {
                        let id = k.id.to_string();
                        let active = current_filter.as_deref() == Some(id.as_str());
                        Tag::new(k.plural.clone())
                            .active(active)
                            .on_press(move |_| filter.set(Some(id.clone())))
                            .into_element()
                    }))
            }))
            .maybe_child((!empty).then(|| {
                Section::new(if needle.is_empty() {
                    "Recientes"
                } else {
                    "Resultados"
                })
                .child(Group::default().children(rows))
            }))
            .maybe_child(empty.then(|| {
                let can_create = matches!(purpose, Purpose::NewTab { .. }) && !typed.is_empty();
                rect()
                    .width(Size::fill())
                    .spacing(10.0)
                    .child(EmptyState::new(
                        freya::icons::lucide::search(),
                        if typed.is_empty() {
                            "No hay documentos".to_owned()
                        } else {
                            format!("Nada se llama «{typed}»")
                        },
                    ))
                    .maybe_child(can_create.then(|| create_named(&typed, purpose)))
            }))
            .into_element()
    }
}

/// "Crear con este nombre": every entry a name is enough for.
fn create_named(name: &str, purpose: Purpose) -> Element {
    let p = palette();
    let entries = named_entries();
    Section::new("Crear con este nombre")
        .child(Group::default().children(entries.into_iter().map(|entry| {
            let name = name.to_owned();
            let color = kind_color(&p, entry.entry.color);
            ListRow::new(format!("{} «{name}»", entry.entry.label))
                .kind(entry.entry.icon.clone(), color)
                .on_press(move |_| {
                    ModalController::get().close();
                    create_quick(entry.clone(), name.clone(), purpose);
                })
                .into_element()
        })))
        .into_element()
}

pub fn create_quick(entry: notein_plugin::catalog::CatalogEntry, name: String, purpose: Purpose) {
    spawn_in_window(async move {
        let Some(kind) = crate::plugins::kind(&entry.kind) else {
            return;
        };
        let Ok(created) = kind.create(
            &entry.entry.id,
            notein_plugin::create::CreationRequest {
                name: &name,
                ..notein_plugin::create::CreationRequest::default()
            },
        ) else {
            return;
        };
        let Ok(handle) = crate::library::shared().await else {
            return;
        };
        let made = {
            let mut lib = handle.lock().await;
            lib.create_item(
                ROOT_FOLDER,
                crate::library::DocKind::plugin(entry.kind.clone()),
                &created.name,
                None,
                Vec::new(),
                crate::library::BackgroundStyle::default(),
            )
            .await
        };
        match made {
            Ok(item) => {
                crate::plugins::content::write(item, &entry.kind, &created.content);
                purpose.pick(item);
            }
            Err(err) => log::error!("create named: {err}"),
        }
    });
}

/// The notebooks of `item`: open one, make a new one beside, or use an
/// existing canvas.
pub fn notebook_sheet(item: ItemId) {
    let Some(data) = LibraryView::get().data() else {
        return;
    };
    let notebooks = data.live_notebooks_of(item);
    if notebooks.is_empty() {
        crate::notebooks::new_notebook(item);
        return;
    }
    let now = crate::format::now();
    let p = palette();
    let rows: Vec<Element> = notebooks
        .iter()
        .filter_map(|id| data.item(*id).cloned())
        .map(|nb| {
            let others: Vec<String> = data
                .companion_parents_of(nb.id)
                .filter(|parent| *parent != item)
                .filter_map(|parent| data.item(parent).map(|i| i.name.clone()))
                .collect();
            let subtitle = if others.is_empty() {
                format!("Editado {}", crate::format::day(nb.updated_at, now))
            } else {
                format!("También cuaderno de {}", others.join(", "))
            };
            let id = nb.id;
            ListRow::new(nb.name)
                .subtitle(subtitle)
                .kind(freya::icons::lucide::pen_line(), p.kinds.canvas)
                .on_press(move |_| {
                    ModalController::get().close();
                    super::open_beside(id);
                })
                .into_element()
        })
        .collect();
    Sheet::titled("Cuaderno")
        .kind(freya::icons::lucide::notebook_pen(), p.kinds.canvas)
        .child(Section::new("Cuadernos de este documento").child(Group::default().children(rows)))
        .child(
            Group::default()
                .child(
                    ListRow::new("Nuevo cuaderno")
                        .icon(freya::icons::lucide::plus())
                        .on_press(move |_| {
                            ModalController::get().close();
                            crate::notebooks::new_notebook(item);
                        }),
                )
                .child(
                    ListRow::new("Usar un lienzo existente…")
                        .icon(freya::icons::lucide::pen_line())
                        .chevron()
                        .on_press(move |_| {
                            ModalController::get().close();
                            document_picker(Purpose::UseAsNotebook { of: item });
                        }),
                ),
        )
        .open();
}

/// A chosen folder.
#[derive(Clone)]
struct OnFolder(Rc<dyn Fn(FolderId)>);

impl PartialEq for OnFolder {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone, PartialEq)]
struct FolderTree {
    current: FolderId,
    /// Folders that cannot be targets (the ones being moved).
    excluded: Vec<FolderId>,
    on_pick: OnFolder,
}

/// Folders under `parent`, depth first and by name, with their depth.
fn walk_folders(
    data: &crate::library::LibraryData,
    parent: FolderId,
    depth: usize,
    out: &mut Vec<(FolderId, String, usize)>,
    excluded: &[FolderId],
) {
    let mut children: Vec<_> = data
        .folders_of(parent)
        .into_iter()
        .filter(|f| !excluded.contains(&f.id))
        .collect();
    children.sort_by_key(|f| f.name.to_lowercase());
    for folder in children {
        out.push((folder.id, folder.name.clone(), depth));
        walk_folders(data, folder.id, depth + 1, out, excluded);
    }
}

impl Component for FolderTree {
    fn render(&self) -> impl IntoElement {
        let Some(data) = LibraryView::get().data() else {
            return rect().into_element();
        };
        let mut rows = vec![(ROOT_FOLDER, "Biblioteca".to_owned(), 0)];
        walk_folders(&data, ROOT_FOLDER, 1, &mut rows, &self.excluded);
        let current = self.current;
        Group::default()
            .children(rows.into_iter().map(|(id, name, depth)| {
                let pick = self.on_pick.clone();
                #[allow(clippy::cast_precision_loss)] // shallow trees
                let indent = depth as f32 * 18.0;
                let row = ListRow::new(name)
                    .icon(if id == ROOT_FOLDER {
                        freya::icons::lucide::library()
                    } else {
                        freya::icons::lucide::folder()
                    })
                    .indent(indent)
                    .selected(id == current);
                let row = if id == current {
                    row.trailing(Trailing::Text("Acá".into()))
                } else {
                    row
                };
                row.on_press(move |_| {
                    ModalController::get().close();
                    (pick.0)(id);
                })
                .into_element()
            }))
            .into_element()
    }
}

/// Pick a folder on top of the current sheet; `on_pick` gets it.
pub fn choose_folder(current: FolderId, on_pick: impl Fn(FolderId) + 'static) {
    Sheet::titled("Se guarda en")
        .on_back(|()| ModalController::get().close())
        .child(FolderTree {
            current,
            excluded: Vec::new(),
            on_pick: OnFolder(Rc::new(on_pick)),
        })
        .push();
}

/// "Mover a…": pick where `items` and `folders` go.
pub fn folder_picker(items: Vec<ItemId>, folders: Vec<FolderId>) {
    let count = items.len() + folders.len();
    let current = LibraryView::get()
        .data()
        .and_then(|d| items.first().and_then(|i| d.item(*i)).map(|i| i.folder))
        .unwrap_or(ROOT_FOLDER);
    let excluded = folders.clone();
    Sheet::titled(if count == 1 {
        "Mover a…".to_owned()
    } else {
        format!("Mover {count} elementos a…")
    })
    .child(FolderTree {
        current,
        excluded,
        on_pick: OnFolder(Rc::new(move |target| {
            let items = items.clone();
            let folders = folders.clone();
            crate::state::edit("mover", async move |lib| {
                for item in items {
                    lib.move_item(item, target).await?;
                }
                for folder in folders {
                    lib.move_folder(folder, target).await?;
                }
                Ok(())
            });
        })),
    })
    .footer(SheetFooter::new([Button::new("Cancelar")
        .ghost()
        .on_press(|_| ModalController::get().close())
        .into_element()]))
    .open();
}

/// The create entries a name is enough for, in the catalog's order.
#[must_use]
pub fn named_entries() -> Vec<notein_plugin::catalog::CatalogEntry> {
    crate::plugins::catalog()
        .entries()
        .into_iter()
        .filter(|e| e.entry.named)
        .collect()
}
