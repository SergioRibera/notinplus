//! Notebooks: a canvas attached to a document through a
//! [`LinkKind::Companion`] link, opened beside it.

use freya::prelude::spawn_in_window;
use notein_i18n::tr;

use crate::library::{
    BackgroundStyle, DocKind, ItemId, Library, LibraryError, LinkKind, NotFoundKind, Result,
    TabTarget,
};
use crate::space::{self, OpenHint};

/// Create a notebook for `item` and open it beside it.
#[sismo::function("notebooks::new_notebook")]
pub fn new_notebook(item: ItemId) {
    spawn_in_window(traced!("task::notebooks::new", async move {
        let Ok(handle) = crate::library::shared().await else {
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
    }));
}

/// Create "Notebook of <doc>" (in the active language) next to `item` and link it as its
/// notebook.
///
/// # Errors
/// [`LibraryError`] when `item` is gone or the library cannot write.
#[sismo::function]
pub async fn create_notebook(lib: &mut Library, item: ItemId) -> Result<ItemId> {
    let parent = lib
        .item(item)
        .cloned()
        .ok_or(LibraryError::NotFound(NotFoundKind::Item(item)))?;
    let name = tr!("notebooks-default-name", "document" => parent.name.clone());
    let kind = crate::plugins::catalog().notebook_kind().map_or_else(
        || crate::plugins::builtin::canvas::KIND.to_owned(),
        |k| k.info().id.into_owned(),
    );
    let id = lib
        .create_item(
            parent.folder,
            DocKind::plugin(&kind),
            &name,
            parent.color,
            Vec::new(),
            BackgroundStyle::default(),
        )
        .await?;
    lib.create_link(item, id, LinkKind::Companion).await?;
    Ok(id)
}
