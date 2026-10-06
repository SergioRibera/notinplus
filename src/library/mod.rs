//! Local library of `.notinplus` docs.
//!
//! Owns the on-device index of folders, items, and tags, and brokers
//! reads / writes of doc bodies against the filesystem root chosen by
//! [`istmo::path::data_dir`]. Home / gallery UI drives every mutation
//! through this facade so persistence, ordering, and validation live
//! in a single place.
//!
//! Split into four submodules:
//!
//! * [`model`] — the persisted types (folders, items, tags, ids).
//! * [`error`] — [`LibraryError`] + `From` conversions.
//! * [`index`] — index blob load / save via `istmo-data-store`.
//! * [`bodies`] — on-disk `.notinplus` + PDF blob helpers.
//!
//! The top-level [`Library`] type wires the three together, keeps the
//! decoded index in memory, and reflects every mutation back to disk
//! before returning.

pub mod bodies;
pub mod error;
pub mod index;
pub mod model;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use istmo::Runtime;
use istmo_data_store::{DataStoreClient, DataStoreConfig};

pub use self::error::{LibraryError, NotFoundKind, Result};
pub use self::model::{
    BackgroundStyle, Folder, FolderId, INDEX_VERSION, Item, ItemId, ItemKind, LibraryIndex,
    ROOT_FOLDER, Rgba, Tag, TagId,
};

use crate::doc::Doc;

/// Cross-platform library of persisted docs.
///
/// Cheap to hold across UI renders — reads hit an in-memory
/// [`LibraryIndex`], writes issue a single bincode round-trip through
/// [`DataStoreClient`] plus (for body ops) one filesystem write.
///
/// Construct with [`Library::open`]; do not clone. If you need the
/// facade in multiple places, wrap it in an `Arc<Mutex<Library>>` at
/// the call site.
#[derive(Debug)]
pub struct Library {
    client: DataStoreClient,
    index: LibraryIndex,
}

impl Library {
    /// Acquire a data-store instance scoped to
    /// [`index::NAMESPACE`] and hydrate the index from disk.
    ///
    /// # Errors
    /// Any runtime / backend / codec failure surfacing through
    /// [`LibraryError`].
    pub async fn open(runtime: &Arc<Runtime>) -> Result<Self> {
        let client = DataStoreClient::from_runtime_with(
            runtime,
            DataStoreConfig::new(index::NAMESPACE.to_owned()),
        )
        .await?;
        let index = index::load(&client).await?;
        Ok(Self { client, index })
    }

    /// Borrow the raw index — convenient for UIs that render the full
    /// tree in a single traversal.
    #[must_use]
    pub const fn index(&self) -> &LibraryIndex {
        &self.index
    }

    // ---------- Reads ---------------------------------------------------

    /// Direct children of `parent`. Order preserved from creation
    /// order; sort at the UI layer when the user picks a different
    /// column.
    #[must_use]
    pub fn folders_of(&self, parent: FolderId) -> Vec<&Folder> {
        self.index
            .folders
            .iter()
            .filter(|f| f.parent == parent)
            .collect()
    }

    /// Items whose parent folder is `folder`.
    #[must_use]
    pub fn items_of(&self, folder: FolderId) -> Vec<&Item> {
        self.index
            .items
            .iter()
            .filter(|i| i.folder == folder)
            .collect()
    }

    /// Items carrying `tag` in their tag list.
    #[must_use]
    pub fn items_with_tag(&self, tag: TagId) -> Vec<&Item> {
        self.index
            .items
            .iter()
            .filter(|i| i.tags.contains(&tag))
            .collect()
    }

    /// Folders carrying `tag` in their tag list.
    #[must_use]
    pub fn folders_with_tag(&self, tag: TagId) -> Vec<&Folder> {
        self.index
            .folders
            .iter()
            .filter(|f| f.tags.contains(&tag))
            .collect()
    }

    /// Every tag the user has defined.
    #[must_use]
    pub fn tags(&self) -> &[Tag] {
        &self.index.tags
    }

    /// Look up a folder by id.
    #[must_use]
    pub fn folder(&self, id: FolderId) -> Option<&Folder> {
        self.index.folder(id)
    }

    /// Look up an item by id.
    #[must_use]
    pub fn item(&self, id: ItemId) -> Option<&Item> {
        self.index.item(id)
    }

    /// Look up a tag by id.
    #[must_use]
    pub fn tag(&self, id: TagId) -> Option<&Tag> {
        self.index.tag(id)
    }

    // ---------- Folder mutations ---------------------------------------

    /// Create a folder under `parent`. Pass [`ROOT_FOLDER`] for a
    /// top-level entry.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] when `parent` is neither the root
    /// nor a known folder id.
    pub async fn create_folder(
        &mut self,
        parent: FolderId,
        name: &str,
        color: Option<Rgba>,
        mut tags: Vec<TagId>,
    ) -> Result<FolderId> {
        self.ensure_folder_target(parent)?;
        for t in &tags {
            if self.index.tag(*t).is_none() {
                return Err(LibraryError::NotFound(NotFoundKind::Tag(*t)));
            }
        }
        tags.sort_unstable_by_key(|t| t.0);
        tags.dedup();
        let now = now_millis();
        let id = self.index.alloc_folder_id();
        self.index.folders.push(Folder {
            id,
            parent,
            name: name.to_owned(),
            color,
            icon: None,
            tags,
            created_at: now,
            updated_at: now,
        });
        self.persist().await?;
        Ok(id)
    }

    /// # Errors
    /// [`LibraryError::NotFound`] / [`LibraryError::RootImmutable`].
    pub async fn rename_folder(&mut self, id: FolderId, name: &str) -> Result<()> {
        Self::reject_root(id)?;
        let now = now_millis();
        let folder = self
            .index
            .folder_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Folder(id)))?;
        name.clone_into(&mut folder.name);
        folder.updated_at = now;
        self.persist().await
    }

    /// # Errors
    /// [`LibraryError::NotFound`], [`LibraryError::Cycle`] when the
    /// requested move would place a folder inside its own subtree, or
    /// [`LibraryError::RootImmutable`] when reparenting the root.
    pub async fn move_folder(&mut self, id: FolderId, new_parent: FolderId) -> Result<()> {
        Self::reject_root(id)?;
        self.ensure_folder_target(new_parent)?;
        if self.index.folder_has_ancestor(new_parent, id) {
            return Err(LibraryError::Cycle);
        }
        let now = now_millis();
        let folder = self
            .index
            .folder_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Folder(id)))?;
        folder.parent = new_parent;
        folder.updated_at = now;
        self.persist().await
    }

    /// # Errors
    /// [`LibraryError::NotFound`] / [`LibraryError::RootImmutable`].
    pub async fn set_folder_color(&mut self, id: FolderId, color: Option<Rgba>) -> Result<()> {
        Self::reject_root(id)?;
        let now = now_millis();
        let folder = self
            .index
            .folder_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Folder(id)))?;
        folder.color = color;
        folder.updated_at = now;
        self.persist().await
    }

    /// # Errors
    /// [`LibraryError::NotFound`] / [`LibraryError::RootImmutable`].
    pub async fn set_folder_icon(&mut self, id: FolderId, icon: Option<String>) -> Result<()> {
        Self::reject_root(id)?;
        let now = now_millis();
        let folder = self
            .index
            .folder_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Folder(id)))?;
        folder.icon = icon;
        folder.updated_at = now;
        self.persist().await
    }

    /// Replace the folder's tag list. Same semantics as
    /// [`Library::set_item_tags`] — unknown tag ids error out,
    /// duplicates are deduplicated.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] when any referenced tag / the folder
    /// itself is unknown, or [`LibraryError::RootImmutable`] when `id`
    /// is the root.
    pub async fn set_folder_tags(&mut self, id: FolderId, mut tags: Vec<TagId>) -> Result<()> {
        Self::reject_root(id)?;
        for t in &tags {
            if self.index.tag(*t).is_none() {
                return Err(LibraryError::NotFound(NotFoundKind::Tag(*t)));
            }
        }
        tags.sort_unstable_by_key(|t| t.0);
        tags.dedup();
        let now = now_millis();
        let folder = self
            .index
            .folder_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Folder(id)))?;
        folder.tags = tags;
        folder.updated_at = now;
        self.persist().await
    }

    /// Delete `id` **and every folder / item descending from it**.
    /// Body files are purged alongside their items.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] / [`LibraryError::RootImmutable`].
    pub async fn delete_folder(&mut self, id: FolderId) -> Result<()> {
        Self::reject_root(id)?;
        if self.index.folder(id).is_none() {
            return Err(LibraryError::NotFound(NotFoundKind::Folder(id)));
        }
        let condemned = self.collect_subtree(id);
        // Wipe items whose folder falls inside the condemned subtree.
        let mut kept_items = Vec::with_capacity(self.index.items.len());
        for item in std::mem::take(&mut self.index.items) {
            if condemned.contains(&item.folder) {
                bodies::purge(item.id);
            } else {
                kept_items.push(item);
            }
        }
        self.index.items = kept_items;
        self.index.folders.retain(|f| !condemned.contains(&f.id));
        self.persist().await
    }

    // ---------- Item mutations -----------------------------------------

    /// Register a new item inside `folder`. The doc body is **not**
    /// created here — call [`Library::save_doc`] afterwards to
    /// materialise it (or leave it lazily written on the first user
    /// edit).
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] when `folder` is unknown.
    pub async fn create_item(
        &mut self,
        folder: FolderId,
        kind: ItemKind,
        name: &str,
        color: Option<Rgba>,
        mut tags: Vec<TagId>,
        background: BackgroundStyle,
    ) -> Result<ItemId> {
        self.ensure_folder_target(folder)?;
        for t in &tags {
            if self.index.tag(*t).is_none() {
                return Err(LibraryError::NotFound(NotFoundKind::Tag(*t)));
            }
        }
        tags.sort_unstable_by_key(|t| t.0);
        tags.dedup();
        let now = now_millis();
        let id = self.index.alloc_item_id();
        self.index.items.push(Item {
            id,
            folder,
            kind,
            name: name.to_owned(),
            color,
            tags,
            background,
            created_at: now,
            updated_at: now,
            thumbnail: None,
        });
        self.persist().await?;
        Ok(id)
    }

    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn rename_item(&mut self, id: ItemId, name: &str) -> Result<()> {
        let now = now_millis();
        let item = self
            .index
            .item_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Item(id)))?;
        name.clone_into(&mut item.name);
        item.updated_at = now;
        self.persist().await
    }

    /// # Errors
    /// [`LibraryError::NotFound`] when either id is unknown.
    pub async fn move_item(&mut self, id: ItemId, folder: FolderId) -> Result<()> {
        self.ensure_folder_target(folder)?;
        let now = now_millis();
        let item = self
            .index
            .item_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Item(id)))?;
        item.folder = folder;
        item.updated_at = now;
        self.persist().await
    }

    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn set_item_color(&mut self, id: ItemId, color: Option<Rgba>) -> Result<()> {
        let now = now_millis();
        let item = self
            .index
            .item_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Item(id)))?;
        item.color = color;
        item.updated_at = now;
        self.persist().await
    }

    /// Replace the item's tag list. Callers pass a set-like `Vec`;
    /// duplicates are dedup'd here so downstream consumers can rely on
    /// unique membership.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] when any referenced tag / the item
    /// itself is unknown.
    pub async fn set_item_tags(&mut self, id: ItemId, mut tags: Vec<TagId>) -> Result<()> {
        for t in &tags {
            if self.index.tag(*t).is_none() {
                return Err(LibraryError::NotFound(NotFoundKind::Tag(*t)));
            }
        }
        tags.sort_unstable_by_key(|t| t.0);
        tags.dedup();
        let now = now_millis();
        let item = self
            .index
            .item_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Item(id)))?;
        item.tags = tags;
        item.updated_at = now;
        self.persist().await
    }

    /// Swap the item's paper pattern.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn set_item_background(
        &mut self,
        id: ItemId,
        background: BackgroundStyle,
    ) -> Result<()> {
        let now = now_millis();
        let item = self
            .index
            .item_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Item(id)))?;
        item.background = background;
        item.updated_at = now;
        self.persist().await
    }

    /// Stash a fresh thumbnail (or clear it with `None`). The library
    /// never generates one itself — the UI provides encoded bytes.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn set_item_thumbnail(
        &mut self,
        id: ItemId,
        thumbnail: Option<Vec<u8>>,
    ) -> Result<()> {
        let now = now_millis();
        let item = self
            .index
            .item_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Item(id)))?;
        item.thumbnail = thumbnail;
        item.updated_at = now;
        self.persist().await
    }

    /// Remove the item and its on-disk bodies.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn delete_item(&mut self, id: ItemId) -> Result<()> {
        let before = self.index.items.len();
        self.index.items.retain(|i| i.id != id);
        if self.index.items.len() == before {
            return Err(LibraryError::NotFound(NotFoundKind::Item(id)));
        }
        bodies::purge(id);
        self.persist().await
    }

    // ---------- Tag mutations ------------------------------------------

    /// # Errors
    /// Runtime / codec / backend failure.
    pub async fn create_tag(&mut self, name: &str, color: Rgba) -> Result<TagId> {
        let id = self.index.alloc_tag_id();
        self.index.tags.push(Tag {
            id,
            name: name.to_owned(),
            color,
        });
        self.persist().await?;
        Ok(id)
    }

    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn rename_tag(&mut self, id: TagId, name: &str) -> Result<()> {
        let tag = self
            .index
            .tag_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Tag(id)))?;
        name.clone_into(&mut tag.name);
        self.persist().await
    }

    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn set_tag_color(&mut self, id: TagId, color: Rgba) -> Result<()> {
        let tag = self
            .index
            .tag_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Tag(id)))?;
        tag.color = color;
        self.persist().await
    }

    /// Remove `id` and sweep it out of every item's tag list.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn delete_tag(&mut self, id: TagId) -> Result<()> {
        let before = self.index.tags.len();
        self.index.tags.retain(|t| t.id != id);
        if self.index.tags.len() == before {
            return Err(LibraryError::NotFound(NotFoundKind::Tag(id)));
        }
        for item in &mut self.index.items {
            item.tags.retain(|t| *t != id);
        }
        self.persist().await
    }

    // ---------- Doc / PDF body I/O -------------------------------------

    /// Decode and return the [`Doc`] body for `id`.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] when the item is unknown,
    /// [`LibraryError::Io`] when no body has ever been written,
    /// [`LibraryError::Codec`] when the on-disk bytes fail to decode.
    // Async signature mirrors `save_doc` so call sites can `.await`
    // both halves of a load/save cycle without special-casing the read.
    #[allow(clippy::unused_async)]
    pub async fn load_doc(&self, id: ItemId) -> Result<Doc> {
        if self.index.item(id).is_none() {
            return Err(LibraryError::NotFound(NotFoundKind::Item(id)));
        }
        bodies::read_doc(id)
    }

    /// Persist `doc` as `id`'s body. Updates the item's
    /// `updated_at` and reflects the mutation to the index.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] / body I/O.
    pub async fn save_doc(&mut self, id: ItemId, doc: &Doc) -> Result<()> {
        if self.index.item(id).is_none() {
            return Err(LibraryError::NotFound(NotFoundKind::Item(id)));
        }
        bodies::write_doc(id, doc)?;
        let now = now_millis();
        if let Some(item) = self.index.item_mut(id) {
            item.updated_at = now;
        }
        self.persist().await
    }

    /// Attach raw PDF bytes to `id`. Overwrites any prior attachment.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] when the item is unknown or is not
    /// [`ItemKind::PdfCanvas`], plus body I/O failures.
    pub async fn attach_pdf(&mut self, id: ItemId, bytes: &[u8]) -> Result<()> {
        let item = self
            .index
            .item(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Item(id)))?;
        if item.kind != ItemKind::PdfCanvas {
            return Err(LibraryError::NotFound(NotFoundKind::Item(id)));
        }
        bodies::write_pdf(id, bytes)?;
        let now = now_millis();
        if let Some(m) = self.index.item_mut(id) {
            m.updated_at = now;
        }
        self.persist().await
    }

    /// Read the PDF attachment as raw bytes.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] / body I/O.
    #[allow(clippy::unused_async)]
    pub async fn read_pdf(&self, id: ItemId) -> Result<Vec<u8>> {
        if self.index.item(id).is_none() {
            return Err(LibraryError::NotFound(NotFoundKind::Item(id)));
        }
        bodies::read_pdf(id)
    }

    // ---------- Internal helpers ---------------------------------------

    async fn persist(&self) -> Result<()> {
        index::save(&self.client, &self.index).await
    }

    fn ensure_folder_target(&self, id: FolderId) -> Result<()> {
        if id == ROOT_FOLDER {
            return Ok(());
        }
        if self.index.folder(id).is_none() {
            return Err(LibraryError::NotFound(NotFoundKind::Folder(id)));
        }
        Ok(())
    }

    #[inline]
    fn reject_root(id: FolderId) -> Result<()> {
        if id == ROOT_FOLDER {
            Err(LibraryError::RootImmutable)
        } else {
            Ok(())
        }
    }

    /// Every folder id that would be caught in a recursive delete
    /// starting from `root` (inclusive).
    fn collect_subtree(&self, root: FolderId) -> Vec<FolderId> {
        let mut out = vec![root];
        let mut cursor = 0;
        while cursor < out.len() {
            let parent = out[cursor];
            cursor += 1;
            for f in &self.index.folders {
                if f.parent == parent && !out.contains(&f.id) {
                    out.push(f.id);
                }
            }
        }
        out
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Direct index tests — sidesteps the DataStoreClient dep so the
    // core invariants can be exercised without a live runtime.

    #[test]
    fn folder_cycle_check_handles_root() {
        let mut idx = LibraryIndex::default();
        let a = idx.alloc_folder_id();
        idx.folders.push(Folder {
            id: a,
            parent: ROOT_FOLDER,
            name: "A".into(),
            color: None,
            icon: None,
            tags: Vec::new(),
            created_at: 0,
            updated_at: 0,
        });
        let b = idx.alloc_folder_id();
        idx.folders.push(Folder {
            id: b,
            parent: a,
            name: "B".into(),
            color: None,
            icon: None,
            tags: Vec::new(),
            created_at: 0,
            updated_at: 0,
        });
        assert!(idx.folder_has_ancestor(b, a));
        assert!(idx.folder_has_ancestor(b, ROOT_FOLDER));
        assert!(!idx.folder_has_ancestor(a, b));
    }

    #[test]
    fn round_trip_bincode() {
        use bincode::config;
        let mut idx = LibraryIndex::default();
        let tag = idx.alloc_tag_id();
        idx.tags.push(Tag {
            id: tag,
            name: "urgent".into(),
            color: [255, 0, 0, 255],
        });
        let folder = idx.alloc_folder_id();
        idx.folders.push(Folder {
            id: folder,
            parent: ROOT_FOLDER,
            name: "Sketches".into(),
            color: Some([12, 34, 56, 255]),
            icon: None,
            tags: Vec::new(),
            created_at: 1,
            updated_at: 2,
        });
        let item = idx.alloc_item_id();
        idx.items.push(Item {
            id: item,
            folder,
            kind: ItemKind::Canvas,
            name: "Untitled".into(),
            color: None,
            tags: vec![tag],
            background: BackgroundStyle::default(),
            created_at: 3,
            updated_at: 4,
            thumbnail: None,
        });
        let bytes = bincode::encode_to_vec(&idx, config::standard()).unwrap();
        let (decoded, _) =
            bincode::decode_from_slice::<LibraryIndex, _>(&bytes, config::standard()).unwrap();
        assert_eq!(idx, decoded);
    }

    #[test]
    fn id_counters_never_reuse() {
        let mut idx = LibraryIndex::default();
        let a = idx.alloc_item_id();
        let b = idx.alloc_item_id();
        assert_ne!(a, b);
        assert!(b.0 > a.0);
    }
}
