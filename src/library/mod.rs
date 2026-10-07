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
    BackgroundStyle, Folder, FolderId, INDEX_VERSION, Item, ItemId, ItemKind, LibraryIndex, Pane,
    PaneId, PaneLayout, PaneTab, ROOT_FOLDER, Rgba, SplitOrientation, SplitSide, Tag, TagId,
    WORKSPACE_INDEX_VERSION, Workspace, WorkspaceId, WorkspaceIndex,
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
    workspaces: WorkspaceIndex,
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
        let workspaces = index::load_workspaces(&client).await?;
        let mut lib = Self {
            client,
            index,
            workspaces,
        };
        lib.ensure_default_workspace().await?;
        Ok(lib)
    }

    /// Guarantee a non-empty [`WorkspaceIndex`]: the switcher assumes
    /// at least one workspace exists and that `active` points at it.
    /// Idempotent — a fresh install mints "Workspace 1" exactly once;
    /// subsequent opens pass through.
    async fn ensure_default_workspace(&mut self) -> Result<()> {
        if !self.workspaces.workspaces.is_empty() {
            if self.workspaces.active.is_none() {
                self.workspaces.active = Some(self.workspaces.workspaces[0].id);
                self.persist_workspaces().await?;
            }
            return Ok(());
        }
        self.create_workspace("Workspace 1", None).await?;
        Ok(())
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
    pub async fn save_doc(&mut self, id: ItemId, doc: &mut Doc) -> Result<()> {
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
    /// [`LibraryError::NotFound`] when the item is unknown or does not
    /// own a PDF body ([`ItemKind::PdfCanvas`], [`ItemKind::SourcePdf`],
    /// [`ItemKind::DocLinked`]), plus body I/O failures.
    pub async fn attach_pdf(&mut self, id: ItemId, bytes: &[u8]) -> Result<()> {
        let item = self
            .index
            .item(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Item(id)))?;
        if !matches!(
            item.kind,
            ItemKind::PdfCanvas | ItemKind::SourcePdf | ItemKind::DocLinked
        ) {
            return Err(LibraryError::NotFound(NotFoundKind::Item(id)));
        }
        bodies::write_pdf(id, bytes)?;
        let now = now_millis();
        if let Some(m) = self.index.item_mut(id) {
            m.updated_at = now;
        }
        self.persist().await
    }

    /// Create a single [`ItemKind::DocLinked`] item bundling a canvas
    /// doc + the imported PDF. The PDF blob lands at the item's own
    /// [`bodies::pdf_path`], the doc body is written lazily on first
    /// stroke / bookmark edit.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] when `folder` is unknown; body I/O
    /// failures when the PDF cannot be written.
    pub async fn create_doc_linked_pdf(
        &mut self,
        folder: FolderId,
        name: &str,
        pdf_bytes: &[u8],
    ) -> Result<ItemId> {
        let linked = self
            .create_item(
                folder,
                ItemKind::DocLinked,
                name,
                None,
                Vec::new(),
                BackgroundStyle::default(),
            )
            .await?;
        bodies::write_pdf(linked, pdf_bytes)?;
        Ok(linked)
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

    // ---------- Workspaces ---------------------------------------------

    /// Borrow the raw workspace index.
    #[must_use]
    pub const fn workspaces_index(&self) -> &WorkspaceIndex {
        &self.workspaces
    }

    /// Every persisted workspace, creation order.
    #[must_use]
    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces.workspaces
    }

    /// Look up a workspace by id.
    #[must_use]
    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.workspace(id)
    }

    /// The workspace the user last had focus on, or `None` when the
    /// index is empty.
    #[must_use]
    pub fn active_workspace(&self) -> Option<WorkspaceId> {
        self.workspaces.active
    }

    /// Create a new empty workspace. Becomes active automatically when
    /// the index previously had no active entry.
    ///
    /// # Errors
    /// Runtime / data-store / codec failures surfacing through
    /// [`LibraryError`].
    pub async fn create_workspace(
        &mut self,
        name: &str,
        color: Option<Rgba>,
    ) -> Result<WorkspaceId> {
        let now = now_millis();
        let id = self.workspaces.alloc_workspace_id();
        self.workspaces
            .workspaces
            .push(Workspace::fresh(id, name.to_owned(), color, now));
        if self.workspaces.active.is_none() {
            self.workspaces.active = Some(id);
        }
        self.persist_workspaces().await?;
        Ok(id)
    }

    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn rename_workspace(&mut self, id: WorkspaceId, name: &str) -> Result<()> {
        let now = now_millis();
        let w = self
            .workspaces
            .workspace_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(id)))?;
        name.clone_into(&mut w.name);
        w.updated_at = now;
        self.persist_workspaces().await
    }

    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn set_workspace_color(
        &mut self,
        id: WorkspaceId,
        color: Option<Rgba>,
    ) -> Result<()> {
        let now = now_millis();
        let w = self
            .workspaces
            .workspace_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(id)))?;
        w.color = color;
        w.updated_at = now;
        self.persist_workspaces().await
    }

    /// Replace the workspace's pane layout wholesale. The shell calls
    /// this on every tab / split change — the tree is small enough that
    /// serialising the whole thing per edit stays cheap and avoids a
    /// patch protocol until CRDT sync lands.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn set_workspace_layout(
        &mut self,
        id: WorkspaceId,
        layout: PaneLayout,
    ) -> Result<()> {
        let now = now_millis();
        let w = self
            .workspaces
            .workspace_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(id)))?;
        w.layout = layout;
        w.updated_at = now;
        self.persist_workspaces().await
    }

    /// Split the pane with id `target` inside workspace `workspace`.
    /// The existing pane keeps whichever side `new_side` does not
    /// address; a brand-new empty pane takes the `new_side` slot and
    /// becomes focused.
    ///
    /// # Errors
    /// * [`LibraryError::NotFound`] when the workspace or target pane
    ///   is unknown.
    pub async fn split_workspace_pane(
        &mut self,
        workspace: WorkspaceId,
        target: PaneId,
        orientation: SplitOrientation,
        new_side: SplitSide,
    ) -> Result<PaneId> {
        let now = now_millis();
        let w = self
            .workspaces
            .workspace_mut(workspace)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        if w.layout.find_leaf(target).is_none() {
            return Err(LibraryError::NotFound(NotFoundKind::Workspace(workspace)));
        }
        let new_id = w.alloc_pane_id();
        if !w.layout.split_leaf(target, orientation, new_id, new_side) {
            return Err(LibraryError::NotFound(NotFoundKind::Workspace(workspace)));
        }
        w.focused_pane = Some(new_id);
        w.updated_at = now;
        self.persist_workspaces().await?;
        Ok(new_id)
    }

    /// Update the ratio of the split node addressed by `path` inside
    /// `workspace`. Ratio is clamped into `0.05..=0.95` by the layout.
    ///
    /// # Errors
    /// * [`LibraryError::NotFound`] when the workspace is unknown or
    ///   `path` does not land on a split node.
    pub async fn set_workspace_split_ratio(
        &mut self,
        workspace: WorkspaceId,
        path: &[SplitSide],
        ratio: f32,
    ) -> Result<()> {
        let now = now_millis();
        let w = self
            .workspaces
            .workspace_mut(workspace)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        if !w.layout.set_ratio_at(path, ratio) {
            return Err(LibraryError::NotFound(NotFoundKind::Workspace(workspace)));
        }
        w.updated_at = now;
        self.persist_workspaces().await
    }

    /// Move the tab at `src_index` in pane `src` to the end of pane
    /// `dst`, flipping focus to that pane. Same-pane and missing-pane
    /// hits are rejected as [`LibraryError::NotFound`].
    ///
    /// # Errors
    /// * [`LibraryError::NotFound`] when the workspace / either pane /
    ///   source index is unknown, or `src == dst`.
    pub async fn move_workspace_tab(
        &mut self,
        workspace: WorkspaceId,
        src: PaneId,
        src_index: usize,
        dst: PaneId,
    ) -> Result<()> {
        let now = now_millis();
        let w = self
            .workspaces
            .workspace_mut(workspace)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        if !w.layout.move_tab(src, src_index, dst) {
            return Err(LibraryError::NotFound(NotFoundKind::Workspace(workspace)));
        }
        w.focused_pane = Some(dst);
        w.updated_at = now;
        self.persist_workspaces().await
    }

    /// Close the pane with id `target`. The sibling collapses up to
    /// take its place. The last pane in a workspace cannot be closed
    /// through this call — the user deletes the workspace instead.
    ///
    /// # Errors
    /// * [`LibraryError::NotFound`] when the workspace is unknown or
    ///   `target` is not a leaf.
    pub async fn close_workspace_pane(
        &mut self,
        workspace: WorkspaceId,
        target: PaneId,
    ) -> Result<()> {
        let now = now_millis();
        let w = self
            .workspaces
            .workspace_mut(workspace)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        if !w.layout.remove_leaf(target) {
            return Err(LibraryError::NotFound(NotFoundKind::Workspace(workspace)));
        }
        // Focus a surviving leaf — the sibling that absorbed the slot
        // is the natural follow-up.
        if w.focused_pane == Some(target) {
            w.focused_pane = w.layout.leaves().first().map(|p| p.id);
        }
        w.updated_at = now;
        self.persist_workspaces().await
    }

    /// Append a tab to the given pane and mark it active.
    ///
    /// # Errors
    /// * [`LibraryError::NotFound`] when the workspace or pane is
    ///   unknown.
    pub async fn add_pane_tab(
        &mut self,
        workspace: WorkspaceId,
        pane: PaneId,
        item: ItemId,
    ) -> Result<()> {
        let now = now_millis();
        let w = self
            .workspaces
            .workspace_mut(workspace)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        let leaf = w
            .layout
            .find_leaf_mut(pane)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        leaf.tabs.push(PaneTab { item });
        leaf.active_tab = u32::try_from(leaf.tabs.len().saturating_sub(1)).unwrap_or(u32::MAX);
        w.updated_at = now;
        self.persist_workspaces().await
    }

    /// Close the tab at `index` within the given pane. The next tab
    /// up (or down when the removed one was last) takes focus. A pane
    /// with no tabs stays alive — close the pane itself to drop it.
    ///
    /// # Errors
    /// * [`LibraryError::NotFound`] when the workspace or pane is
    ///   unknown, or `index` is out of range.
    pub async fn close_pane_tab(
        &mut self,
        workspace: WorkspaceId,
        pane: PaneId,
        index: usize,
    ) -> Result<()> {
        let now = now_millis();
        let w = self
            .workspaces
            .workspace_mut(workspace)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        let leaf = w
            .layout
            .find_leaf_mut(pane)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        if index >= leaf.tabs.len() {
            return Err(LibraryError::NotFound(NotFoundKind::Workspace(workspace)));
        }
        leaf.tabs.remove(index);
        let new_len = leaf.tabs.len();
        let active = leaf.active_tab as usize;
        leaf.active_tab = if new_len == 0 {
            0
        } else if active >= new_len {
            u32::try_from(new_len.saturating_sub(1)).unwrap_or(u32::MAX)
        } else {
            leaf.active_tab
        };
        w.updated_at = now;
        self.persist_workspaces().await
    }

    /// Flip the active tab inside `pane`. Out-of-range `index` is
    /// clamped to the last valid tab.
    ///
    /// # Errors
    /// * [`LibraryError::NotFound`] when the workspace or pane is
    ///   unknown.
    pub async fn focus_pane_tab(
        &mut self,
        workspace: WorkspaceId,
        pane: PaneId,
        index: usize,
    ) -> Result<()> {
        let now = now_millis();
        let w = self
            .workspaces
            .workspace_mut(workspace)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        let leaf = w
            .layout
            .find_leaf_mut(pane)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        let max = leaf.tabs.len().saturating_sub(1);
        leaf.active_tab = u32::try_from(index.min(max)).unwrap_or(u32::MAX);
        w.focused_pane = Some(pane);
        w.updated_at = now;
        self.persist_workspaces().await
    }

    /// Focus the given pane. `None` clears focus (the shell routes
    /// input to no pane in particular — rare, mostly a test seam).
    ///
    /// # Errors
    /// * [`LibraryError::NotFound`] when the workspace is unknown or
    ///   `pane` is given but not present in the layout.
    pub async fn focus_workspace_pane(
        &mut self,
        workspace: WorkspaceId,
        pane: Option<PaneId>,
    ) -> Result<()> {
        let now = now_millis();
        let w = self
            .workspaces
            .workspace_mut(workspace)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        if let Some(id) = pane
            && w.layout.find_leaf(id).is_none()
        {
            return Err(LibraryError::NotFound(NotFoundKind::Workspace(workspace)));
        }
        w.focused_pane = pane;
        w.updated_at = now;
        self.persist_workspaces().await
    }

    /// Flip the active workspace. `None` clears the selection — the
    /// shell uses that to force the switcher to its empty state after
    /// deleting the last workspace.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] when `id` is not in the index.
    pub async fn set_active_workspace(&mut self, id: Option<WorkspaceId>) -> Result<()> {
        if let Some(wid) = id
            && self.workspaces.workspace(wid).is_none()
        {
            return Err(LibraryError::NotFound(NotFoundKind::Workspace(wid)));
        }
        self.workspaces.active = id;
        self.persist_workspaces().await
    }

    /// Remove a workspace. If it was the active one, the active
    /// pointer falls back to the first remaining workspace (or `None`
    /// when the list is now empty).
    ///
    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn delete_workspace(&mut self, id: WorkspaceId) -> Result<()> {
        let pos = self
            .workspaces
            .workspaces
            .iter()
            .position(|w| w.id == id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(id)))?;
        self.workspaces.workspaces.remove(pos);
        if self.workspaces.active == Some(id) {
            self.workspaces.active = self.workspaces.workspaces.first().map(|w| w.id);
        }
        self.persist_workspaces().await
    }

    // ---------- Bundle import ------------------------------------------

    /// Merge a parsed `.notein` bundle into the library. Caller must
    /// have confirmed `preflight.has_collisions() == false` upstream;
    /// this method does not re-check — a UUID clash here would silently
    /// overwrite the live entry.
    ///
    /// Items whose declared `folder` is not in the bundle land under
    /// [`ROOT_FOLDER`] so an orphaned reference stays visible instead
    /// of silently disappearing.
    ///
    /// # Errors
    /// Filesystem / persistence failures.
    pub async fn import_bundle(&mut self, bundle: &crate::bundle::ParsedBundle) -> Result<()> {
        let known_folders: std::collections::HashSet<FolderId> = bundle
            .manifest
            .folders
            .iter()
            .map(|f| f.id)
            .chain(self.index.folders.iter().map(|f| f.id))
            .chain(std::iter::once(ROOT_FOLDER))
            .collect();

        for tag in &bundle.manifest.tags {
            if self.index.tag(tag.id).is_none() {
                self.index.tags.push(tag.clone());
            }
        }

        for folder in &bundle.manifest.folders {
            let mut f = folder.clone();
            if !known_folders.contains(&f.parent) {
                f.parent = ROOT_FOLDER;
            }
            self.index.folders.push(f);
        }

        for item in &bundle.manifest.items {
            let mut i = item.clone();
            if !known_folders.contains(&i.folder) {
                i.folder = ROOT_FOLDER;
            }
            self.index.items.push(i);
        }

        for ws in &bundle.manifest.workspaces {
            self.workspaces.workspaces.push(ws.clone());
        }

        for (id, bytes) in &bundle.docs {
            bodies::write_doc_bytes(*id, bytes)?;
        }
        for (id, bytes) in &bundle.pdfs {
            bodies::write_pdf(*id, bytes)?;
        }

        self.persist().await?;
        self.persist_workspaces().await?;
        Ok(())
    }

    // ---------- Internal helpers ---------------------------------------

    async fn persist(&self) -> Result<()> {
        index::save(&self.client, &self.index).await
    }

    async fn persist_workspaces(&self) -> Result<()> {
        index::save_workspaces(&self.client, &self.workspaces).await
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

    fn mk_item(seed: u8) -> ItemId {
        let mut bytes = [0u8; 16];
        bytes[0] = seed;
        ItemId::from_bytes(bytes)
    }

    fn mk_pane(seed: u8) -> PaneId {
        let mut bytes = [0u8; 16];
        bytes[0] = seed;
        PaneId::from_bytes(bytes)
    }

    fn mk_workspace(seed: u8) -> WorkspaceId {
        let mut bytes = [0u8; 16];
        bytes[0] = seed;
        WorkspaceId::from_bytes(bytes)
    }

    #[test]
    fn id_counters_never_reuse() {
        let idx = LibraryIndex::default();
        let a = idx.alloc_item_id();
        let b = idx.alloc_item_id();
        assert_ne!(a, b);
        assert!(!a.is_nil());
        assert!(!b.is_nil());
    }

    #[test]
    fn workspace_index_round_trip() {
        use bincode::config;
        let mut idx = WorkspaceIndex::default();
        let a = idx.alloc_workspace_id();
        let left = mk_pane(1);
        let right = mk_pane(2);
        let layout = PaneLayout::Split {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.6,
            first: Box::new(PaneLayout::Leaf(Pane {
                id: left,
                tabs: vec![
                    PaneTab { item: mk_item(42) },
                    PaneTab { item: mk_item(7) },
                ],
                active_tab: 1,
            })),
            second: Box::new(PaneLayout::Leaf(Pane {
                id: right,
                tabs: vec![PaneTab { item: mk_item(99) }],
                active_tab: 0,
            })),
        };
        idx.workspaces.push(Workspace {
            id: a,
            name: "Draft".into(),
            color: Some([200, 50, 50, 255]),
            layout,
            focused_pane: Some(right),
            created_at: 10,
            updated_at: 20,
        });
        idx.active = Some(a);
        let bytes = bincode::encode_to_vec(&idx, config::standard()).unwrap();
        let (decoded, _) =
            bincode::decode_from_slice::<WorkspaceIndex, _>(&bytes, config::standard()).unwrap();
        assert_eq!(idx, decoded);
        assert_eq!(decoded.workspaces[0].layout.leaves().len(), 2);
    }

    #[test]
    fn split_leaf_adds_sibling_and_remove_leaf_collapses() {
        let root = mk_pane(1);
        let mut layout = PaneLayout::leaf(root);
        let new_id = mk_pane(2);
        assert!(layout.split_leaf(root, SplitOrientation::Horizontal, new_id, SplitSide::Second));
        assert_eq!(layout.leaves().len(), 2);
        assert!(layout.find_leaf(root).is_some());
        assert!(layout.find_leaf(new_id).is_some());

        // Removing one leaf collapses the Split to the sibling.
        assert!(layout.remove_leaf(new_id));
        assert!(matches!(layout, PaneLayout::Leaf(ref p) if p.id == root));
    }

    #[test]
    fn remove_leaf_refuses_root_single_leaf() {
        let root = mk_pane(1);
        let mut layout = PaneLayout::leaf(root);
        // Nothing to collapse into — refusal keeps the workspace with
        // at least one leaf for the shell to render.
        assert!(!layout.remove_leaf(root));
    }

    #[test]
    fn pane_add_close_focus_tab_direct() {
        // Direct in-memory tab mutation mirrors the Library::*_pane_tab
        // helpers without going through DataStoreClient.
        let mut w = Workspace::fresh(mk_workspace(1), "W".into(), None, 0);
        let root = w.focused_pane.unwrap();

        {
            let leaf = w.layout.find_leaf_mut(root).unwrap();
            leaf.tabs.push(PaneTab { item: mk_item(10) });
            leaf.tabs.push(PaneTab { item: mk_item(11) });
            leaf.active_tab = 1;
        }

        // Focus tab 0.
        {
            let leaf = w.layout.find_leaf_mut(root).unwrap();
            leaf.active_tab = 0;
            assert_eq!(leaf.active_tab, 0);
        }

        // Close tab 0 — the shift leaves one tab with index 0 active.
        {
            let leaf = w.layout.find_leaf_mut(root).unwrap();
            leaf.tabs.remove(0);
            let new_len = leaf.tabs.len();
            if leaf.active_tab as usize >= new_len {
                leaf.active_tab = u32::try_from(new_len.saturating_sub(1)).unwrap_or(0);
            }
            assert_eq!(leaf.tabs.len(), 1);
            assert_eq!(leaf.tabs[0].item, mk_item(11));
            assert_eq!(leaf.active_tab, 0);
        }
    }

    #[test]
    fn clamp_ratios_bounds_divider() {
        let mut layout = PaneLayout::Split {
            orientation: SplitOrientation::Vertical,
            ratio: 1.5,
            first: Box::new(PaneLayout::leaf(mk_pane(1))),
            second: Box::new(PaneLayout::leaf(mk_pane(2))),
        };
        layout.clamp_ratios();
        if let PaneLayout::Split { ratio, .. } = layout {
            assert!((ratio - 0.95).abs() < 1e-6);
        } else {
            panic!("expected Split")
        }
    }

    #[test]
    fn workspace_ids_never_reuse() {
        let idx = WorkspaceIndex::default();
        let a = idx.alloc_workspace_id();
        let b = idx.alloc_workspace_id();
        assert_ne!(a, b);
        assert!(!a.is_nil());
        assert!(!b.is_nil());
    }

    #[test]
    fn set_ratio_at_updates_nested_split() {
        let mut layout = PaneLayout::Split {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.5,
            first: Box::new(PaneLayout::leaf(mk_pane(1))),
            second: Box::new(PaneLayout::Split {
                orientation: SplitOrientation::Vertical,
                ratio: 0.5,
                first: Box::new(PaneLayout::leaf(mk_pane(2))),
                second: Box::new(PaneLayout::leaf(mk_pane(3))),
            }),
        };
        // Root ratio.
        assert!(layout.set_ratio_at(&[], 0.3));
        // Nested right-side split.
        assert!(layout.set_ratio_at(&[SplitSide::Second], 0.7));
        // Walking into a leaf fails.
        assert!(!layout.set_ratio_at(&[SplitSide::First], 0.4));
        // Walking off the bottom of a nested tree fails.
        assert!(!layout.set_ratio_at(
            &[SplitSide::Second, SplitSide::First, SplitSide::First],
            0.4,
        ));

        let PaneLayout::Split {
            ratio, second, ..
        } = layout
        else {
            panic!("root collapsed")
        };
        assert!((ratio - 0.3).abs() < 1e-6);
        if let PaneLayout::Split { ratio, .. } = *second {
            assert!((ratio - 0.7).abs() < 1e-6);
        } else {
            panic!("nested collapsed");
        }
    }

    #[test]
    fn set_ratio_at_clamps_out_of_range_input() {
        let mut layout = PaneLayout::Split {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.5,
            first: Box::new(PaneLayout::leaf(mk_pane(1))),
            second: Box::new(PaneLayout::leaf(mk_pane(2))),
        };
        assert!(layout.set_ratio_at(&[], 1.5));
        let PaneLayout::Split { ratio, .. } = layout else {
            panic!("collapsed")
        };
        assert!((ratio - 0.95).abs() < 1e-6);
    }

    #[test]
    fn move_tab_moves_between_leaves_and_clamps_active_index() {
        let left = mk_pane(1);
        let right = mk_pane(2);
        let mut layout = PaneLayout::Split {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.5,
            first: Box::new(PaneLayout::Leaf(Pane {
                id: left,
                tabs: vec![PaneTab { item: mk_item(10) }, PaneTab { item: mk_item(11) }],
                active_tab: 1,
            })),
            second: Box::new(PaneLayout::Leaf(Pane {
                id: right,
                tabs: vec![PaneTab { item: mk_item(99) }],
                active_tab: 0,
            })),
        };
        assert!(layout.move_tab(left, 1, right));
        let src = layout.find_leaf(left).unwrap();
        assert_eq!(src.tabs.len(), 1);
        assert_eq!(src.tabs[0].item, mk_item(10));
        // The removed tab was the active one; clamp drops to the only
        // remaining tab.
        assert_eq!(src.active_tab, 0);
        let dst = layout.find_leaf(right).unwrap();
        assert_eq!(dst.tabs.len(), 2);
        assert_eq!(dst.tabs[1].item, mk_item(11));
        // Destination focuses the just-arrived tab.
        assert_eq!(dst.active_tab, 1);
    }

    #[test]
    fn move_tab_rejects_same_pane_and_missing_targets() {
        let only = mk_pane(1);
        let mut layout = PaneLayout::Leaf(Pane {
            id: only,
            tabs: vec![PaneTab { item: mk_item(10) }],
            active_tab: 0,
        });
        // Same pane is a no-op reject (ordering is a different op).
        assert!(!layout.move_tab(only, 0, only));
        // Missing destination keeps the source intact.
        assert!(!layout.move_tab(only, 0, mk_pane(99)));
        let src = layout.find_leaf(only).unwrap();
        assert_eq!(src.tabs.len(), 1);
        assert_eq!(src.tabs[0].item, mk_item(10));
    }
}
