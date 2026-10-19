//! Notebook (companion canvas) and link flows shared by every face.
//!
//! A notebook is a stroke canvas attached to a document through a
//! [`LinkKind::Companion`] link. The flows here create, attach and
//! open notebooks, create [`LinkKind::Related`] links and open other
//! documents beside the current one. Faces call them from their own
//! chrome; nothing here knows which plugin is asking.

use freya::prelude::spawn_forever;

use crate::components::{ItemPicker, NotebookChooser, PickerChoice};
use crate::home::open_library;
use crate::library::{
    BackgroundStyle, DocKind, ItemId, Library, LibraryError, LinkKind, NotFoundKind, Result,
    TabTarget,
};
use crate::space::{self, OpenHint};

/// Plugin id of the canvas that every notebook is made of.
pub const NOTEBOOK_PLUGIN: &str = crate::builtin::stroke_canvas::KIND_ID;

/// Open the notebook of `item` beside it. Creates one when there is
/// none; asks which one when there are several.
pub fn open_notebook(item: ItemId) {
    spawn_forever(async move {
        let Ok(handle) = open_library().await else {
            return;
        };
        let companions: Vec<(ItemId, String)> = {
            let lib = handle.lock().await;
            lib.companions_of(item)
                .filter_map(|id| lib.item(id).map(|it| (id, it.name.clone())))
                .collect()
        };
        match companions.as_slice() {
            [] => {
                let created = {
                    let mut lib = handle.lock().await;
                    create_notebook(&mut lib, item).await
                };
                match created {
                    Ok(id) => space::open(TabTarget::Item(id), OpenHint::Beside),
                    Err(err) => log::error!("create notebook: {err}"),
                }
            }
            [(only, _)] => space::open(TabTarget::Item(*only), OpenHint::Beside),
            _ => NotebookChooser::open(item, companions),
        }
    });
}

/// Create a fresh notebook for `item` and open it beside it.
pub fn new_notebook(item: ItemId) {
    spawn_forever(async move {
        let Ok(handle) = open_library().await else {
            return;
        };
        let created = {
            let mut lib = handle.lock().await;
            create_notebook(&mut lib, item).await
        };
        match created {
            Ok(id) => space::open(TabTarget::Item(id), OpenHint::Beside),
            Err(err) => log::error!("create notebook: {err}"),
        }
    });
}

/// Pick an existing canvas and make it a notebook of `item`.
pub fn attach_existing_notebook(item: ItemId) {
    ItemPicker::new(move |choice: PickerChoice| {
        let PickerChoice::Existing(canvas) = choice else {
            return;
        };
        link_and_open(item, canvas, LinkKind::Companion, true);
    })
    .title("Usar un lienzo existente")
    .only_plugin(NOTEBOOK_PLUGIN)
    .exclude(item)
    .quick_actions(false)
    .open();
}

/// Pick any document and mark it as related to `item`.
pub fn link_related(item: ItemId) {
    ItemPicker::new(move |choice: PickerChoice| {
        let PickerChoice::Existing(other) = choice else {
            return;
        };
        link_and_open(item, other, LinkKind::Related, false);
    })
    .title("Vincular con…")
    .exclude(item)
    .quick_actions(false)
    .open();
}

/// Pick what to open beside the current document, creating it when
/// the user asks for something new.
pub fn open_beside_picker() {
    ItemPicker::new(|choice: PickerChoice| {
        spawn_forever(async move {
            let Ok(handle) = open_library().await else {
                return;
            };
            let target = {
                let mut lib = handle.lock().await;
                choice.resolve(&mut lib).await
            };
            if let Some(target) = target {
                space::open(target, OpenHint::Beside);
            }
        });
    })
    .title("Abrir al lado")
    .open();
}

fn link_and_open(from: ItemId, to: ItemId, kind: LinkKind, open: bool) {
    spawn_forever(async move {
        let Ok(handle) = open_library().await else {
            return;
        };
        let linked = {
            let mut lib = handle.lock().await;
            lib.create_link(from, to, kind).await
        };
        match linked {
            Ok(_) if open => space::open(TabTarget::Item(to), OpenHint::Beside),
            Ok(_) => space::notify_changed(),
            Err(err) => log::error!("create link: {err}"),
        }
    });
}

/// Create "Cuaderno de <item>" next to `item` and link it as its
/// notebook.
///
/// # Errors
/// [`LibraryError::NotFound`] when `item` is gone; persistence
/// failures.
pub async fn create_notebook(lib: &mut Library, item: ItemId) -> Result<ItemId> {
    let parent = lib
        .item(item)
        .cloned()
        .ok_or(LibraryError::NotFound(NotFoundKind::Item(item)))?;
    let name = format!("Cuaderno de {}", parent.name);
    let id = lib
        .create_item(
            parent.folder,
            DocKind::plugin(NOTEBOOK_PLUGIN),
            &name,
            parent.color,
            Vec::new(),
            BackgroundStyle::default(),
        )
        .await?;
    lib.create_link(item, id, LinkKind::Companion).await?;
    Ok(id)
}
