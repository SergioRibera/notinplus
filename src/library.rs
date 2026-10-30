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
//! decoded index in memory, and hands every mutation to the store before
//! returning.

// The facade stays async: callers are written against a store that may
// have to wait, even where a write returns before it reaches the disk.
#![allow(clippy::unused_async)]

pub mod bodies;
pub mod error;
pub mod graveyard;
mod import;
pub mod index;
pub mod links;
pub mod model;
mod pins;
pub mod refs;
pub mod shared;
pub mod sync;
mod trash;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use istmo::Runtime;
use istmo_data_store::{DataStoreClient, DataStoreConfig};
use notein_i18n::tr;

/// How long a deletion marker is kept: ninety days, longer than any
/// device is expected to stay away.
const TOMBSTONE_TTL_MS: u64 = 90 * 24 * 60 * 60 * 1000;

pub use self::error::{LibraryError, NotFoundKind, Result};
pub use self::import::{Conflict, ImportPlan, ImportReceipt, Resolution, Review};
pub use self::links::{Link, LinkIndex, LinkKind};
pub use self::model::{
    AnnotationId, BackgroundStyle, DocKind, Folder, FolderId, INDEX_VERSION, Item, ItemId,
    LibraryIndex, LinkId, Pane, PaneId, PaneLayout, PaneTab, ROOT_FOLDER, Rgba, ScratchId,
    SplitOrientation, SplitSide, TabTarget, Tag, TagId, Trashed, ViewSlot, WORKSPACE_INDEX_VERSION,
    Workspace, WorkspaceId, WorkspaceIndex,
};
pub use self::refs::{RefEntry, RefOrigin};
pub use self::shared::{LibHandle, changes, shared, shared_blocking};
pub use self::trash::{
    NotebookFate, TrashEntry, TrashReceipt, TrashedRecord, days_left, retention_millis,
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
    client: Arc<DataStoreClient>,
    data: LibraryData,
}

/// Everything the library holds in memory: the index, the mesas and
/// the links. Screens read a snapshot of it; [`Library`] derefs to it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibraryData {
    index: LibraryIndex,
    workspaces: WorkspaceIndex,
    links: LinkIndex,
}

impl std::ops::Deref for Library {
    type Target = LibraryData;

    fn deref(&self) -> &LibraryData {
        &self.data
    }
}

impl Library {}

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
        let mut index = index::load(&client).await?.unwrap_or_else(|| {
            // Nothing readable under the current key: whatever the
            // item store holds belongs to an older schema.
            bodies::discard_store();
            LibraryIndex::default()
        });
        read_covers(&mut index);
        let mut workspaces = index::load_workspaces(&client).await?;
        for workspace in &mut workspaces.workspaces {
            for pane in workspace.layout.leaves_mut() {
                pane.tabs
                    .retain(|tab| !matches!(tab.target, TabTarget::Annotation(_)));
            }
        }
        let links = index::load_links(&client).await?;
        let mut lib = Self {
            client: Arc::new(client),
            data: LibraryData {
                index,
                workspaces,
                links,
            },
        };
        lib.ensure_default_workspace().await?;
        lib.compact_tombstones().await?;
        Ok(lib)
    }

    /// Drop deletion markers old enough that no device is expected to
    /// still carry the record they cancel.
    async fn compact_tombstones(&mut self) -> Result<()> {
        let now = now_millis();
        if self.data.links.compact(now, TOMBSTONE_TTL_MS) > 0 {
            self.persist_links()?;
        }
        let mut yard = graveyard::Graveyard::load();
        if yard.compact(now, TOMBSTONE_TTL_MS) > 0
            && let Err(err) = yard.save()
        {
            log::error!("graveyard save: {err}");
        }
        Ok(())
    }

    /// Guarantee a non-empty [`WorkspaceIndex`]: the switcher assumes
    /// at least one workspace exists and that `active` points at it.
    /// Idempotent — a fresh install mints "Workspace 1" exactly once;
    /// subsequent opens pass through.
    async fn ensure_default_workspace(&mut self) -> Result<()> {
        if !self.data.workspaces.workspaces.is_empty() {
            if self.data.workspaces.active.is_none() {
                self.data.workspaces.active = Some(self.data.workspaces.workspaces[0].id);
                self.persist_workspaces()?;
            }
            return Ok(());
        }
        self.create_workspace("", None).await?;
        Ok(())
    }

    // ---------- Reads ---------------------------------------------------

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
            if self.data.index.tag(*t).is_none() {
                return Err(LibraryError::NotFound(NotFoundKind::Tag(*t)));
            }
        }
        tags.sort_unstable_by_key(|t| t.0);
        tags.dedup();
        let now = now_millis();
        let id = FolderId::new_v4();
        self.data.index.folders.push(Folder {
            id,
            parent,
            name: name.to_owned(),
            color,
            icon: None,
            tags,
            created_at: now,
            updated_at: now,
            trashed: None,
        });
        self.persist()?;
        Ok(id)
    }

    /// # Errors
    /// [`LibraryError::NotFound`] / [`LibraryError::RootImmutable`].
    pub async fn rename_folder(&mut self, id: FolderId, name: &str) -> Result<()> {
        Self::reject_root(id)?;
        let now = now_millis();
        let folder = self
            .data
            .index
            .folder_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Folder(id)))?;
        name.clone_into(&mut folder.name);
        folder.updated_at = now;
        self.persist()
    }

    /// # Errors
    /// [`LibraryError::NotFound`], [`LibraryError::Cycle`] when the
    /// requested move would place a folder inside its own subtree, or
    /// [`LibraryError::RootImmutable`] when reparenting the root.
    pub async fn move_folder(&mut self, id: FolderId, new_parent: FolderId) -> Result<()> {
        Self::reject_root(id)?;
        self.ensure_folder_target(new_parent)?;
        if self.data.index.folder_has_ancestor(new_parent, id) {
            return Err(LibraryError::Cycle);
        }
        let now = now_millis();
        let folder = self
            .data
            .index
            .folder_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Folder(id)))?;
        folder.parent = new_parent;
        folder.updated_at = now;
        self.persist()
    }

    /// # Errors
    /// [`LibraryError::NotFound`] / [`LibraryError::RootImmutable`].
    pub async fn set_folder_color(&mut self, id: FolderId, color: Option<Rgba>) -> Result<()> {
        Self::reject_root(id)?;
        let now = now_millis();
        let folder = self
            .data
            .index
            .folder_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Folder(id)))?;
        folder.color = color;
        folder.updated_at = now;
        self.persist()
    }

    /// # Errors
    /// [`LibraryError::NotFound`] / [`LibraryError::RootImmutable`].
    pub async fn set_folder_icon(&mut self, id: FolderId, icon: Option<String>) -> Result<()> {
        Self::reject_root(id)?;
        let now = now_millis();
        let folder = self
            .data
            .index
            .folder_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Folder(id)))?;
        folder.icon = icon;
        folder.updated_at = now;
        self.persist()
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
            if self.data.index.tag(*t).is_none() {
                return Err(LibraryError::NotFound(NotFoundKind::Tag(*t)));
            }
        }
        tags.sort_unstable_by_key(|t| t.0);
        tags.dedup();
        let now = now_millis();
        let folder = self
            .data
            .index
            .folder_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Folder(id)))?;
        folder.tags = tags;
        folder.updated_at = now;
        self.persist()
    }

    /// Delete `id` **and every folder / item descending from it**.
    /// Body files are purged alongside their items.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] / [`LibraryError::RootImmutable`].
    pub async fn purge_folder(&mut self, id: FolderId) -> Result<()> {
        Self::reject_root(id)?;
        if self.data.index.folder(id).is_none() {
            return Err(LibraryError::NotFound(NotFoundKind::Folder(id)));
        }
        let condemned = self.collect_subtree(id);
        let now = now_millis();
        let mut yard = graveyard::Graveyard::load();
        // Wipe items whose folder falls inside the condemned subtree.
        let mut kept_items = Vec::with_capacity(self.data.index.items.len());
        for item in std::mem::take(&mut self.data.index.items) {
            if condemned.contains(&item.folder) {
                bodies::purge(item.id);
                refs::forget(item.id);
                self.data.links.retire_item(item.id, now);
                yard.bury(graveyard::Buried::Item, item.id.0, now);
            } else {
                kept_items.push(item);
            }
        }
        self.data.index.items = kept_items;
        self.data
            .index
            .folders
            .retain(|f| !condemned.contains(&f.id));
        for folder in &condemned {
            yard.bury(graveyard::Buried::Folder, folder.0, now);
        }
        if let Err(err) = yard.save() {
            log::error!("graveyard save: {err}");
        }
        self.persist_links()?;
        self.persist()
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
        kind: DocKind,
        name: &str,
        color: Option<Rgba>,
        mut tags: Vec<TagId>,
        background: BackgroundStyle,
    ) -> Result<ItemId> {
        self.ensure_folder_target(folder)?;
        for t in &tags {
            if self.data.index.tag(*t).is_none() {
                return Err(LibraryError::NotFound(NotFoundKind::Tag(*t)));
            }
        }
        tags.sort_unstable_by_key(|t| t.0);
        tags.dedup();
        let now = now_millis();
        let id = ItemId::new_v4();
        self.data.index.items.push(Item {
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
            trashed: None,
        });
        self.persist()?;
        Ok(id)
    }

    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn rename_item(&mut self, id: ItemId, name: &str) -> Result<()> {
        let now = now_millis();
        let item = self
            .data
            .index
            .item_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Item(id)))?;
        name.clone_into(&mut item.name);
        item.updated_at = now;
        self.persist()
    }

    /// # Errors
    /// [`LibraryError::NotFound`] when either id is unknown.
    pub async fn move_item(&mut self, id: ItemId, folder: FolderId) -> Result<()> {
        self.ensure_folder_target(folder)?;
        let now = now_millis();
        let item = self
            .data
            .index
            .item_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Item(id)))?;
        item.folder = folder;
        item.updated_at = now;
        self.persist()
    }

    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn set_item_color(&mut self, id: ItemId, color: Option<Rgba>) -> Result<()> {
        let now = now_millis();
        let item = self
            .data
            .index
            .item_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Item(id)))?;
        item.color = color;
        item.updated_at = now;
        self.persist()
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
            if self.data.index.tag(*t).is_none() {
                return Err(LibraryError::NotFound(NotFoundKind::Tag(*t)));
            }
        }
        tags.sort_unstable_by_key(|t| t.0);
        tags.dedup();
        let now = now_millis();
        let item = self
            .data
            .index
            .item_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Item(id)))?;
        item.tags = tags;
        item.updated_at = now;
        self.persist()
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
            .data
            .index
            .item_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Item(id)))?;
        item.background = background;
        item.updated_at = now;
        self.persist()
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
            .data
            .index
            .item_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Item(id)))?;
        bodies::write_cover(id, thumbnail.as_deref())?;
        item.thumbnail = thumbnail.map(Arc::from);
        item.updated_at = now;
        self.persist()
    }

    /// Copy `id` into the same folder as "Name (copy)", in the active language. Returns the
    /// new item.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`]; filesystem or persistence errors.
    pub async fn duplicate_item(&mut self, id: ItemId) -> Result<ItemId> {
        let source = self
            .data
            .index
            .item(id)
            .cloned()
            .ok_or(LibraryError::NotFound(NotFoundKind::Item(id)))?;
        let now = now_millis();
        let copy = ItemId::new_v4();
        bodies::copy_item(id, copy).map_err(LibraryError::from)?;
        self.data.index.items.push(Item {
            id: copy,
            name: tr!("library-item-copy-name", "name" => source.name.clone()),
            created_at: now,
            updated_at: now,
            trashed: None,
            ..source
        });
        self.persist()?;
        Ok(copy)
    }

    /// Remove the item and its on-disk bodies.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn purge_item(&mut self, id: ItemId) -> Result<()> {
        let before = self.data.index.items.len();
        self.data.index.items.retain(|i| i.id != id);
        if self.data.index.items.len() == before {
            return Err(LibraryError::NotFound(NotFoundKind::Item(id)));
        }
        bodies::purge(id);
        refs::forget(id);
        let now = now_millis();
        graveyard::bury(graveyard::Buried::Item, id.0, now);
        if self.data.links.retire_item(id, now) > 0 {
            self.persist_links()?;
        }
        self.persist()
    }

    /// Drop this library's copy of `id` without recording a deletion:
    /// the document still exists elsewhere, this device just stops
    /// holding it. Used when a shared session ends for a guest.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn forget_item(&mut self, id: ItemId) -> Result<()> {
        let before = self.data.index.items.len();
        self.data.index.items.retain(|i| i.id != id);
        if self.data.index.items.len() == before {
            return Err(LibraryError::NotFound(NotFoundKind::Item(id)));
        }
        bodies::purge(id);
        refs::forget(id);
        self.persist()
    }

    // ---------- Tag mutations ------------------------------------------

    /// # Errors
    /// Runtime / codec / backend failure.
    pub async fn create_tag(&mut self, name: &str, color: Rgba) -> Result<TagId> {
        let id = TagId::new_v4();
        self.data.index.tags.push(Tag {
            id,
            name: name.to_owned(),
            color,
            updated_at: now_millis(),
        });
        self.persist()?;
        Ok(id)
    }

    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn rename_tag(&mut self, id: TagId, name: &str) -> Result<()> {
        let tag = self
            .data
            .index
            .tag_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Tag(id)))?;
        name.clone_into(&mut tag.name);
        tag.updated_at = now_millis();
        self.persist()
    }

    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn set_tag_color(&mut self, id: TagId, color: Rgba) -> Result<()> {
        let tag = self
            .data
            .index
            .tag_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Tag(id)))?;
        tag.color = color;
        tag.updated_at = now_millis();
        self.persist()
    }

    /// Remove `id` and sweep it out of every item's tag list.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn delete_tag(&mut self, id: TagId) -> Result<()> {
        let before = self.data.index.tags.len();
        self.data.index.tags.retain(|t| t.id != id);
        if self.data.index.tags.len() == before {
            return Err(LibraryError::NotFound(NotFoundKind::Tag(id)));
        }
        for item in &mut self.data.index.items {
            item.tags.retain(|t| *t != id);
        }
        graveyard::bury(graveyard::Buried::Tag, id.0, now_millis());
        self.persist()
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
        if self.data.index.item(id).is_none() {
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
        if self.data.index.item(id).is_none() {
            return Err(LibraryError::NotFound(NotFoundKind::Item(id)));
        }
        bodies::write_doc(id, doc)?;
        refs::record_doc(id, doc);
        self.touch_item(id).await
    }

    /// Persist snapshot bytes taken earlier with [`Doc::snapshot_bytes`]
    /// together with the references captured at the same moment.
    /// Lets a worker export under the board lock and write without it.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] / body I/O.
    pub async fn save_doc_snapshot(
        &mut self,
        id: ItemId,
        bytes: &[u8],
        captured: refs::Captured,
    ) -> Result<()> {
        if self.data.index.item(id).is_none() {
            return Err(LibraryError::NotFound(NotFoundKind::Item(id)));
        }
        bodies::write_doc_bytes(id, bytes)?;
        refs::record_captured(id, captured);
        self.touch_item(id).await
    }

    async fn touch_item(&mut self, id: ItemId) -> Result<()> {
        let now = now_millis();
        if let Some(item) = self.data.index.item_mut(id) {
            item.updated_at = now;
        }
        self.persist()
    }

    // ---------- Links --------------------------------------------------

    /// Relate `from` and `to`. Returns the existing link id when the
    /// same edge is already live.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] when either item is unknown,
    /// [`LibraryError::SelfLink`] when both ends are the same item.
    pub async fn create_link(
        &mut self,
        from: ItemId,
        to: ItemId,
        kind: LinkKind,
    ) -> Result<LinkId> {
        for id in [from, to] {
            if self.data.index.item(id).is_none() {
                return Err(LibraryError::NotFound(NotFoundKind::Item(id)));
            }
        }
        let id = self
            .data
            .links
            .insert(from, to, kind, now_millis())
            .ok_or(LibraryError::SelfLink)?;
        self.persist_links()?;
        Ok(id)
    }

    /// Remove a link.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] when no live link has that id.
    pub async fn delete_link(&mut self, id: LinkId) -> Result<()> {
        if !self.data.links.remove(id, now_millis()) {
            return Err(LibraryError::NotFound(NotFoundKind::Link(id)));
        }
        self.persist_links()
    }

    fn persist_links(&self) -> Result<()> {
        index::store(
            &self.client,
            index::LINKS_KEY,
            index::encode(&self.data.links)?,
        );
        crate::sync::local_changed();
        shared::notify();
        Ok(())
    }

    /// Attach raw PDF bytes to `id`. Overwrites any prior attachment.
    /// Which plugins read the attachment is their business; the
    /// library only stores it next to the item.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] when the item is unknown, plus body
    /// I/O failures.
    pub async fn attach_pdf(&mut self, id: ItemId, bytes: &[u8]) -> Result<()> {
        if self.data.index.item(id).is_none() {
            return Err(LibraryError::NotFound(NotFoundKind::Item(id)));
        }
        bodies::write_pdf(id, bytes)?;
        let now = now_millis();
        crate::sync::attachment_changed(id);
        if let Some(m) = self.data.index.item_mut(id) {
            m.updated_at = now;
        }
        self.persist()
    }

    /// Read the PDF attachment as raw bytes.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] / body I/O.
    #[allow(clippy::unused_async)]
    pub async fn read_pdf(&self, id: ItemId) -> Result<Vec<u8>> {
        if self.data.index.item(id).is_none() {
            return Err(LibraryError::NotFound(NotFoundKind::Item(id)));
        }
        bodies::read_pdf(id)
    }

    // ---------- Workspaces ---------------------------------------------

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
        let id = WorkspaceId::new_v4();
        let mut workspace = Workspace::fresh(id, name.trim().to_owned(), color, now);
        workspace.named = !workspace.name.is_empty();
        workspace.device = crate::sync::snapshot().device;
        self.data.workspaces.workspaces.push(workspace);
        if self.data.workspaces.active.is_none() {
            self.data.workspaces.active = Some(id);
        }
        self.persist_workspaces()?;
        Ok(id)
    }

    /// Copy a workspace — layout, tabs and colour — under a new id
    /// named "Copia de <name>". The copy is not activated.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn duplicate_workspace(&mut self, id: WorkspaceId) -> Result<WorkspaceId> {
        let now = now_millis();
        let source = self
            .data
            .workspaces
            .workspace(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(id)))?;
        let copy = Workspace {
            id: WorkspaceId::new_v4(),
            name: tr!("library-desk-copy-name", "name" => source.name.clone()),
            named: true,
            pinned: false,
            color: source.color,
            layout: source.layout.clone(),
            focused_pane: source.focused_pane,
            created_at: now,
            updated_at: now,
            device: crate::sync::snapshot().device,
        };
        let new_id = copy.id;
        self.data.workspaces.workspaces.push(copy);
        self.persist_workspaces()?;
        Ok(new_id)
    }

    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn rename_workspace(&mut self, id: WorkspaceId, name: &str) -> Result<()> {
        let now = now_millis();
        let w = self
            .data
            .workspaces
            .workspace_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(id)))?;
        name.trim().clone_into(&mut w.name);
        w.named = !w.name.is_empty();
        w.updated_at = now;
        self.persist_workspaces()
    }

    /// Show (or stop showing) a mesa first in Home.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`]; persistence errors.
    pub async fn set_workspace_pinned(&mut self, id: WorkspaceId, pinned: bool) -> Result<()> {
        let now = now_millis();
        let w = self
            .data
            .workspaces
            .workspace_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(id)))?;
        w.pinned = pinned;
        w.updated_at = now;
        self.persist_workspaces()
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
            .data
            .workspaces
            .workspace_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(id)))?;
        w.color = color;
        w.updated_at = now;
        self.persist_workspaces()
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
            .data
            .workspaces
            .workspace_mut(id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(id)))?;
        w.layout = layout;
        w.updated_at = now;
        self.persist_workspaces()
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
            .data
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
        self.persist_workspaces()?;
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
            .data
            .workspaces
            .workspace_mut(workspace)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        if !w.layout.set_ratio_at(path, ratio) {
            return Err(LibraryError::NotFound(NotFoundKind::Workspace(workspace)));
        }
        w.updated_at = now;
        self.persist_workspaces()
    }

    /// Move the tab at `src_index` in pane `src` to the end of pane
    /// `dst`, flipping focus to that pane. A source pane left with no
    /// tabs closes (unless it is the only one). Same-pane and
    /// missing-pane hits are rejected as [`LibraryError::NotFound`].
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
            .data
            .workspaces
            .workspace_mut(workspace)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        if !w.layout.move_tab(src, src_index, dst) {
            return Err(LibraryError::NotFound(NotFoundKind::Workspace(workspace)));
        }
        w.layout.prune_empty_leaf(src);
        w.focused_pane = Some(dst);
        w.updated_at = now;
        self.persist_workspaces()
    }

    /// Split pane `target` along `orientation` and move the tab at
    /// `src_index` of pane `src` into the fresh pane on `new_side`,
    /// which takes focus. A source pane left with no tabs closes. One
    /// persisted step: a tab dropped on a pane edge either lands in its
    /// new pane or nothing changes.
    ///
    /// # Errors
    /// * [`LibraryError::NotFound`] when the workspace, either pane or
    ///   the source index is unknown.
    pub async fn move_workspace_tab_to_split(
        &mut self,
        workspace: WorkspaceId,
        src: PaneId,
        src_index: usize,
        target: PaneId,
        orientation: SplitOrientation,
        new_side: SplitSide,
    ) -> Result<PaneId> {
        let now = now_millis();
        let w = self
            .data
            .workspaces
            .workspace_mut(workspace)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        let new_id = w.alloc_pane_id();
        if !w
            .layout
            .move_tab_to_split(src, src_index, target, orientation, new_id, new_side)
        {
            return Err(LibraryError::NotFound(NotFoundKind::Workspace(workspace)));
        }
        w.layout.prune_empty_leaf(src);
        w.focused_pane = Some(new_id);
        w.updated_at = now;
        self.persist_workspaces()?;
        Ok(new_id)
    }

    /// Move the tab at `from` inside `pane` to slot `to` (clamped to the
    /// last tab). The pane takes focus.
    ///
    /// # Errors
    /// * [`LibraryError::NotFound`] when the workspace or pane is
    ///   unknown, or `from` is out of range.
    pub async fn reorder_pane_tab(
        &mut self,
        workspace: WorkspaceId,
        pane: PaneId,
        from: usize,
        to: usize,
    ) -> Result<()> {
        let now = now_millis();
        let w = self
            .data
            .workspaces
            .workspace_mut(workspace)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        let in_range = w
            .layout
            .find_leaf(pane)
            .is_some_and(|leaf| from < leaf.tabs.len());
        if !in_range {
            return Err(LibraryError::NotFound(NotFoundKind::Workspace(workspace)));
        }
        if !w.layout.reorder_tab(pane, from, to) {
            return Ok(());
        }
        w.focused_pane = Some(pane);
        w.updated_at = now;
        self.persist_workspaces()
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
            .data
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
        self.persist_workspaces()
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
        self.push_tab(workspace, pane, PaneTab::new(item)).await
    }

    /// Open a scratch canvas tab in `pane` and focus it.
    ///
    /// # Errors
    /// * [`LibraryError::NotFound`] when the workspace or pane is
    ///   unknown.
    pub async fn add_scratch_tab(&mut self, workspace: WorkspaceId, pane: PaneId) -> Result<()> {
        self.push_tab(workspace, pane, PaneTab::scratch()).await
    }

    /// Open a tab showing `target` in `pane` and focus it.
    ///
    /// # Errors
    /// * [`LibraryError::NotFound`] when the workspace or pane is
    ///   unknown.
    pub async fn add_tab(
        &mut self,
        workspace: WorkspaceId,
        pane: PaneId,
        target: TabTarget,
    ) -> Result<()> {
        self.push_tab(workspace, pane, PaneTab::showing(target))
            .await
    }

    async fn push_tab(&mut self, workspace: WorkspaceId, pane: PaneId, tab: PaneTab) -> Result<()> {
        let now = now_millis();
        let w = self
            .data
            .workspaces
            .workspace_mut(workspace)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        let leaf = w
            .layout
            .find_leaf_mut(pane)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(workspace)))?;
        leaf.tabs.push(tab);
        leaf.active_tab = u32::try_from(leaf.tabs.len().saturating_sub(1)).unwrap_or(u32::MAX);
        w.updated_at = now;
        self.persist_workspaces()
    }

    /// Close the tab at `index` within the given pane. The next tab
    /// up (or down when the removed one was last) takes focus. A pane
    /// left with no tabs closes too, unless it is the only one; focus
    /// then moves to the first remaining pane.
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
            .data
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
        if w.layout.prune_empty_leaf(pane) && w.focused_pane == Some(pane) {
            w.focused_pane = w.layout.leaves().first().map(|p| p.id);
        }
        w.updated_at = now;
        self.persist_workspaces()
    }

    /// Close every tab showing one of `items`, in every workspace. A
    /// pane left without tabs stays alive. Nothing is written when no
    /// tab matched.
    ///
    /// # Errors
    /// Runtime / codec / backend failure.
    pub async fn close_item_tabs(&mut self, items: &[ItemId]) -> Result<()> {
        let now = now_millis();
        let mut touched = false;
        for workspace in &mut self.data.workspaces.workspaces {
            for pane in workspace.layout.leaves_mut() {
                let before = pane.tabs.len();
                pane.tabs.retain(
                    |tab| !matches!(tab.target, TabTarget::Item(id) if items.contains(&id)),
                );
                if pane.tabs.len() == before {
                    continue;
                }
                let last = pane.tabs.len().saturating_sub(1);
                if pane.active_tab as usize > last {
                    pane.active_tab = u32::try_from(last).unwrap_or(u32::MAX);
                }
                workspace.updated_at = now;
                touched = true;
            }
        }
        if touched {
            self.persist_workspaces()?;
        }
        Ok(())
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
            .data
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
        self.persist_workspaces()
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
            .data
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
        self.persist_workspaces()
    }

    /// Flip the active workspace. `None` clears the selection — the
    /// shell uses that to force the switcher to its empty state after
    /// deleting the last workspace.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] when `id` is not in the index.
    pub async fn set_active_workspace(&mut self, id: Option<WorkspaceId>) -> Result<()> {
        if let Some(wid) = id
            && self.data.workspaces.workspace(wid).is_none()
        {
            return Err(LibraryError::NotFound(NotFoundKind::Workspace(wid)));
        }
        self.data.workspaces.active = id;
        self.persist_workspaces()
    }

    /// Remove a workspace. If it was the active one, the active
    /// pointer falls back to the first remaining workspace (or `None`
    /// when the list is now empty).
    ///
    /// # Errors
    /// [`LibraryError::NotFound`].
    pub async fn delete_workspace(&mut self, id: WorkspaceId) -> Result<()> {
        let pos = self
            .data
            .workspaces
            .workspaces
            .iter()
            .position(|w| w.id == id)
            .ok_or(LibraryError::NotFound(NotFoundKind::Workspace(id)))?;
        self.data.workspaces.workspaces.remove(pos);
        if self.data.workspaces.active == Some(id) {
            self.data.workspaces.active = self.data.workspaces.workspaces.first().map(|w| w.id);
        }
        graveyard::bury(graveyard::Buried::Workspace, id.0, now_millis());
        self.persist_workspaces()
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
            .chain(self.data.index.folders.iter().map(|f| f.id))
            .chain(std::iter::once(ROOT_FOLDER))
            .collect();

        for tag in &bundle.manifest.tags {
            if self.data.index.tag(tag.id).is_none() {
                self.data.index.tags.push(tag.clone());
            }
        }

        for folder in &bundle.manifest.folders {
            let mut f = folder.clone();
            if !known_folders.contains(&f.parent) {
                f.parent = ROOT_FOLDER;
            }
            self.data.index.folders.push(f);
        }

        for item in &bundle.manifest.items {
            let mut i = item.clone();
            if !known_folders.contains(&i.folder) {
                i.folder = ROOT_FOLDER;
            }
            keep_cover(i.id, i.thumbnail.as_deref());
            self.data.index.items.push(i);
        }

        for ws in &bundle.manifest.workspaces {
            self.data.workspaces.workspaces.push(ws.clone());
        }

        for incoming in &bundle.manifest.links {
            match self
                .data
                .links
                .links
                .iter_mut()
                .find(|l| l.id == incoming.id)
            {
                Some(existing) => {
                    if incoming.updated_at > existing.updated_at {
                        *existing = incoming.clone();
                    }
                }
                None => self.data.links.links.push(incoming.clone()),
            }
        }

        for (id, bytes) in &bundle.docs {
            bodies::write_doc_bytes(*id, bytes)?;
            if let Ok(doc) = bodies::read_doc(*id) {
                refs::record_doc(*id, &doc);
            }
        }
        for (id, bytes) in &bundle.pdfs {
            bodies::write_pdf(*id, bytes)?;
        }

        self.persist()?;
        self.persist_workspaces()?;
        self.persist_links()?;
        Ok(())
    }

    // ---------- Internal helpers ---------------------------------------

    /// Store the index. Covers stay out of it: each lives in its own file.
    fn persist(&mut self) -> Result<()> {
        let covers: Vec<_> = self
            .data
            .index
            .items
            .iter_mut()
            .map(|item| item.thumbnail.take())
            .collect();
        let bytes = index::encode(&self.data.index);
        for (item, cover) in self.data.index.items.iter_mut().zip(covers) {
            item.thumbnail = cover;
        }
        index::store(&self.client, index::INDEX_KEY, bytes?);
        crate::sync::local_changed();
        shared::notify();
        Ok(())
    }

    fn persist_workspaces(&self) -> Result<()> {
        index::store(
            &self.client,
            index::WORKSPACES_KEY,
            index::encode(&self.data.workspaces)?,
        );
        crate::sync::local_changed();
        shared::notify();
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
}

/// Read every cover from its file.
fn read_covers(index: &mut LibraryIndex) {
    for item in &mut index.items {
        item.thumbnail = bodies::read_cover(item.id).map(Arc::from);
    }
}

/// Store the cover a record arrived with; failures are logged.
fn keep_cover(id: ItemId, cover: Option<&[u8]>) {
    if let Err(err) = bodies::write_cover(id, cover) {
        log::error!("cover {id}: {err}");
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

impl LibraryData {
    /// Data assembled in memory, for screen previews.
    #[cfg(all(test, not(feature = "hotreload")))]
    pub(crate) const fn from_parts(
        index: LibraryIndex,
        workspaces: WorkspaceIndex,
        links: LinkIndex,
    ) -> Self {
        Self {
            index,
            workspaces,
            links,
        }
    }

    /// A copy of the in-memory state for a screen to render from.
    #[must_use]
    pub fn snapshot(&self) -> Self {
        self.clone()
    }

    /// Borrow the raw index — convenient for UIs that render the full
    /// tree in a single traversal.
    #[must_use]
    pub const fn index(&self) -> &LibraryIndex {
        &self.index
    }

    /// Direct children of `parent`. Order preserved from creation
    /// order; sort at the UI layer when the user picks a different
    /// column.
    #[must_use]
    pub fn folders_of(&self, parent: FolderId) -> Vec<&Folder> {
        self.index
            .folders
            .iter()
            .filter(|f| f.parent == parent && f.trashed.is_none())
            .collect()
    }

    /// Items whose parent folder is `folder`.
    #[must_use]
    pub fn items_of(&self, folder: FolderId) -> Vec<&Item> {
        self.index
            .items
            .iter()
            .filter(|i| i.folder == folder && i.trashed.is_none())
            .collect()
    }

    /// Items carrying `tag` in their tag list.
    #[must_use]
    pub fn items_with_tag(&self, tag: TagId) -> Vec<&Item> {
        self.index
            .items
            .iter()
            .filter(|i| i.tags.contains(&tag) && i.trashed.is_none())
            .collect()
    }

    /// Folders carrying `tag` in their tag list.
    #[must_use]
    pub fn folders_with_tag(&self, tag: TagId) -> Vec<&Folder> {
        self.index
            .folders
            .iter()
            .filter(|f| f.tags.contains(&tag) && f.trashed.is_none())
            .collect()
    }

    /// Every item outside the trash.
    pub fn live_items(&self) -> impl Iterator<Item = &Item> {
        self.index.items.iter().filter(|i| i.trashed.is_none())
    }

    /// Every folder outside the trash.
    pub fn live_folders(&self) -> impl Iterator<Item = &Folder> {
        self.index.folders.iter().filter(|f| f.trashed.is_none())
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

    /// Every live link touching `item`, either direction.
    pub fn links_of(&self, item: ItemId) -> impl Iterator<Item = &Link> {
        self.links.of(item)
    }

    /// Items that are notebooks of `item`.
    pub fn companions_of(&self, item: ItemId) -> impl Iterator<Item = ItemId> + '_ {
        self.links.companions_of(item)
    }

    /// Items that `item` is a notebook of.
    pub fn companion_parents_of(&self, item: ItemId) -> impl Iterator<Item = ItemId> + '_ {
        self.links.companion_parents_of(item)
    }

    /// Live link by id.
    #[must_use]
    pub fn link(&self, id: LinkId) -> Option<&Link> {
        self.links.get(id)
    }

    /// Borrow the raw link index.
    #[must_use]
    pub const fn links(&self) -> &LinkIndex {
        &self.links
    }

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
    pub const fn active_workspace(&self) -> Option<WorkspaceId> {
        self.workspaces.active
    }

    /// Name a mesa shows: what the user typed, or one derived from the
    /// documents open in it ("Romans commentary and 2 more").
    #[must_use]
    pub fn workspace_title(&self, id: WorkspaceId) -> String {
        let Some(w) = self.workspaces.workspace(id) else {
            return String::new();
        };
        if w.named {
            return w.name.clone();
        }
        let mut names = w
            .layout
            .leaves()
            .into_iter()
            .flat_map(|pane| pane.tabs.iter())
            .filter_map(crate::library::PaneTab::item)
            .filter_map(|item| self.item(item).map(|i| i.name.clone()));
        names.next().map_or_else(
            || tr!("library-empty-desk"),
            |first| match names.count() {
                0 => first,
                more => tr!("library-desk-title-more", "first" => first, "count" => more),
            },
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    // Direct index tests — sidesteps the DataStoreClient dep so the
    // core invariants can be exercised without a live runtime.

    #[test]
    fn folder_cycle_check_handles_root() {
        let mut idx = LibraryIndex::default();
        let a = FolderId::new_v4();
        idx.folders.push(Folder {
            id: a,
            parent: ROOT_FOLDER,
            name: "A".into(),
            color: None,
            icon: None,
            tags: Vec::new(),
            created_at: 0,
            updated_at: 0,
            trashed: None,
        });
        let b = FolderId::new_v4();
        idx.folders.push(Folder {
            id: b,
            parent: a,
            name: "B".into(),
            color: None,
            icon: None,
            tags: Vec::new(),
            created_at: 0,
            updated_at: 0,
            trashed: None,
        });
        assert!(idx.folder_has_ancestor(b, a));
        assert!(idx.folder_has_ancestor(b, ROOT_FOLDER));
        assert!(!idx.folder_has_ancestor(a, b));
    }

    #[test]
    fn round_trip_bincode() {
        use bincode::config;
        let mut idx = LibraryIndex::default();
        let tag = TagId::new_v4();
        idx.tags.push(Tag {
            id: tag,
            name: "urgent".into(),
            color: [255, 0, 0, 255],
            updated_at: 0,
        });
        let folder = FolderId::new_v4();
        idx.folders.push(Folder {
            id: folder,
            parent: ROOT_FOLDER,
            name: "Sketches".into(),
            color: Some([12, 34, 56, 255]),
            icon: None,
            tags: Vec::new(),
            created_at: 1,
            updated_at: 2,
            trashed: None,
        });
        let item = ItemId::new_v4();
        idx.items.push(Item {
            id: item,
            folder,
            kind: DocKind::plugin(crate::plugins::builtin::canvas::KIND),
            name: "Untitled".into(),
            color: None,
            tags: vec![tag],
            background: BackgroundStyle::default(),
            created_at: 3,
            updated_at: 4,
            thumbnail: None,
            trashed: None,
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
        let a = ItemId::new_v4();
        let b = ItemId::new_v4();
        assert_ne!(a, b);
        assert!(!a.is_nil());
        assert!(!b.is_nil());
    }

    #[test]
    fn workspace_index_round_trip() {
        use bincode::config;
        let mut idx = WorkspaceIndex::default();
        let a = WorkspaceId::new_v4();
        let left = mk_pane(1);
        let right = mk_pane(2);
        let layout = PaneLayout::Split {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.6,
            first: Box::new(PaneLayout::Leaf(Pane {
                id: left,
                tabs: vec![PaneTab::new(mk_item(42)), PaneTab::new(mk_item(7))],
                active_tab: 1,
            })),
            second: Box::new(PaneLayout::Leaf(Pane {
                id: right,
                tabs: vec![PaneTab::new(mk_item(99))],
                active_tab: 0,
            })),
        };
        idx.workspaces.push(Workspace {
            id: a,
            name: "Draft".into(),
            named: true,
            pinned: false,
            color: Some([200, 50, 50, 255]),
            layout,
            focused_pane: Some(right),
            created_at: 10,
            updated_at: 20,
            device: Some(notein_sync::DeviceId([7; 32])),
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
        assert!(layout.split_leaf(
            root,
            SplitOrientation::Horizontal,
            new_id,
            SplitSide::Second
        ));
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
            leaf.tabs.push(PaneTab::new(mk_item(10)));
            leaf.tabs.push(PaneTab::new(mk_item(11)));
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
            assert_eq!(leaf.tabs[0].item(), Some(mk_item(11)));
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
        let a = WorkspaceId::new_v4();
        let b = WorkspaceId::new_v4();
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

        let PaneLayout::Split { ratio, second, .. } = layout else {
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
                tabs: vec![PaneTab::new(mk_item(10)), PaneTab::new(mk_item(11))],
                active_tab: 1,
            })),
            second: Box::new(PaneLayout::Leaf(Pane {
                id: right,
                tabs: vec![PaneTab::new(mk_item(99))],
                active_tab: 0,
            })),
        };
        assert!(layout.move_tab(left, 1, right));
        let src = layout.find_leaf(left).unwrap();
        assert_eq!(src.tabs.len(), 1);
        assert_eq!(src.tabs[0].item(), Some(mk_item(10)));
        // The removed tab was the active one; clamp drops to the only
        // remaining tab.
        assert_eq!(src.active_tab, 0);
        let dst = layout.find_leaf(right).unwrap();
        assert_eq!(dst.tabs.len(), 2);
        assert_eq!(dst.tabs[1].item(), Some(mk_item(11)));
        // Destination focuses the just-arrived tab.
        assert_eq!(dst.active_tab, 1);
    }

    #[test]
    fn move_tab_rejects_same_pane_and_missing_targets() {
        let only = mk_pane(1);
        let mut layout = PaneLayout::Leaf(Pane {
            id: only,
            tabs: vec![PaneTab::new(mk_item(10))],
            active_tab: 0,
        });
        // Same pane is a no-op reject (ordering is a different op).
        assert!(!layout.move_tab(only, 0, only));
        // Missing destination keeps the source intact.
        assert!(!layout.move_tab(only, 0, mk_pane(99)));
        let src = layout.find_leaf(only).unwrap();
        assert_eq!(src.tabs.len(), 1);
        assert_eq!(src.tabs[0].item(), Some(mk_item(10)));
    }

    #[test]
    fn move_tab_to_split_lands_the_tab_in_the_fresh_pane() {
        let only = mk_pane(1);
        let fresh = mk_pane(2);
        let mut layout = PaneLayout::Leaf(Pane {
            id: only,
            tabs: vec![PaneTab::new(mk_item(10)), PaneTab::new(mk_item(11))],
            active_tab: 1,
        });
        assert!(layout.move_tab_to_split(
            only,
            0,
            only,
            SplitOrientation::Horizontal,
            fresh,
            SplitSide::Second,
        ));
        let PaneLayout::Split { first, second, .. } = &layout else {
            panic!("no split");
        };
        let PaneLayout::Leaf(left) = &**first else {
            panic!("left is not a leaf");
        };
        let PaneLayout::Leaf(right) = &**second else {
            panic!("right is not a leaf");
        };
        assert_eq!(left.id, only);
        assert_eq!(left.tabs.len(), 1);
        assert_eq!(left.tabs[0].item(), Some(mk_item(11)));
        assert_eq!(left.active_tab, 0);
        assert_eq!(right.id, fresh);
        assert_eq!(right.tabs.len(), 1);
        assert_eq!(right.tabs[0].item(), Some(mk_item(10)));
    }

    #[test]
    fn move_tab_to_split_leaves_the_layout_alone_on_a_bad_drop() {
        let only = mk_pane(1);
        let mut layout = PaneLayout::Leaf(Pane {
            id: only,
            tabs: vec![PaneTab::new(mk_item(10))],
            active_tab: 0,
        });
        assert!(!layout.move_tab_to_split(
            only,
            5,
            only,
            SplitOrientation::Horizontal,
            mk_pane(2),
            SplitSide::Second,
        ));
        assert!(!layout.move_tab_to_split(
            only,
            0,
            mk_pane(99),
            SplitOrientation::Horizontal,
            mk_pane(2),
            SplitSide::Second,
        ));
        assert!(matches!(layout, PaneLayout::Leaf(_)));
        assert_eq!(layout.leaves()[0].tabs.len(), 1);
    }

    #[test]
    fn prune_empty_leaf_collapses_a_drained_pane_but_never_the_root() {
        let left = mk_pane(1);
        let right = mk_pane(2);
        let mut layout = PaneLayout::Split {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.5,
            first: Box::new(PaneLayout::Leaf(Pane {
                id: left,
                tabs: vec![PaneTab::new(mk_item(10))],
                active_tab: 0,
            })),
            second: Box::new(PaneLayout::leaf(right)),
        };
        // Still has a tab: nothing happens.
        assert!(!layout.prune_empty_leaf(left));
        assert!(layout.move_tab(left, 0, right));
        assert!(layout.prune_empty_leaf(left));
        let PaneLayout::Leaf(only) = &layout else {
            panic!("did not collapse");
        };
        assert_eq!(only.id, right);
        assert_eq!(only.tabs.len(), 1);
        // Root leaf, even empty, is kept.
        let mut root = PaneLayout::leaf(left);
        assert!(!root.prune_empty_leaf(left));
        assert!(matches!(root, PaneLayout::Leaf(_)));
    }

    #[test]
    fn reorder_tab_moves_within_the_pane_and_keeps_the_active_tab() {
        let pane = mk_pane(1);
        let mut layout = PaneLayout::Leaf(Pane {
            id: pane,
            tabs: vec![
                PaneTab::new(mk_item(10)),
                PaneTab::new(mk_item(11)),
                PaneTab::new(mk_item(12)),
            ],
            active_tab: 1,
        });
        let items = |layout: &PaneLayout| -> Vec<Option<ItemId>> {
            layout.leaves()[0].tabs.iter().map(PaneTab::item).collect()
        };
        // Drag the first tab onto the last: it lands after it, and the
        // active tab (11) slides left to keep its identity.
        assert!(layout.reorder_tab(pane, 0, 2));
        assert_eq!(
            items(&layout),
            vec![Some(mk_item(11)), Some(mk_item(12)), Some(mk_item(10))]
        );
        assert_eq!(layout.leaves()[0].active_tab, 0);
        // Drag the active tab itself to the end.
        assert!(layout.reorder_tab(pane, 0, 2));
        assert_eq!(
            items(&layout),
            vec![Some(mk_item(12)), Some(mk_item(10)), Some(mk_item(11))]
        );
        assert_eq!(layout.leaves()[0].active_tab, 2);
        // Dragging back to the left pushes the others right.
        assert!(layout.reorder_tab(pane, 2, 0));
        assert_eq!(
            items(&layout),
            vec![Some(mk_item(11)), Some(mk_item(12)), Some(mk_item(10))]
        );
        assert_eq!(layout.leaves()[0].active_tab, 0);
        // Same slot or out-of-range source: nothing changes.
        assert!(!layout.reorder_tab(pane, 1, 1));
        assert!(!layout.reorder_tab(pane, 7, 0));
        // Out-of-range destination clamps to the last slot.
        assert!(layout.reorder_tab(pane, 0, 99));
        assert_eq!(items(&layout)[2], Some(mk_item(11)));
    }
}
