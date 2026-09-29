//! Persisted data model for the local `.notinplus` library.
//!
//! Every mutation reads/writes the entire [`LibraryIndex`] blob to
//! `data-store`. The blob is intentionally small — folders / items /
//! tags only. Doc bodies live on the filesystem alongside optional PDF
//! attachments (see [`super::bodies`]).

/// Reserved id for the implicit root folder that every fresh index
/// carries. User-created folders start at `1`.
pub const ROOT_FOLDER: FolderId = FolderId(0);

/// Schema version encoded into the index blob. Bump on any breaking
/// change to the surrounding types.
pub const INDEX_VERSION: u32 = 1;

/// Stable identifier for a canvas / PDF-backed doc.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ItemId(pub u64);

/// Stable identifier for a folder. `FolderId(0)` is the root.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FolderId(pub u64);

/// Stable identifier for a user-defined tag.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TagId(pub u32);

/// sRGB colour swatch. Alpha included so future themes can honour
/// user-defined translucency.
pub type Rgba = [u8; 4];

/// What kind of doc an [`Item`] represents. Drives which body files
/// the library expects to find on disk for the id.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ItemKind {
    /// Blank infinite canvas.
    Canvas,
    /// Canvas overlaid on a PDF; the PDF blob lives at
    /// [`super::bodies::pdf_path`].
    PdfCanvas,
}

/// User-defined tag. Colour is chosen at creation time and can be
/// updated independently of the name.
#[istmo::message]
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Tag {
    pub id: TagId,
    pub name: String,
    pub color: Rgba,
}

/// Folder metadata. `parent == ROOT_FOLDER` marks a top-level folder.
#[istmo::message]
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Folder {
    pub id: FolderId,
    pub parent: FolderId,
    pub name: String,
    pub color: Option<Rgba>,
    pub icon: Option<String>,
    pub tags: Vec<TagId>,
    pub created_at: u64,
    pub updated_at: u64,
}

/// Item metadata. The corresponding doc body lives on disk at
/// [`super::bodies::doc_path`] and (for [`ItemKind::PdfCanvas`]) a
/// sibling PDF at [`super::bodies::pdf_path`].
#[istmo::message]
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Item {
    pub id: ItemId,
    pub folder: FolderId,
    pub kind: ItemKind,
    pub name: String,
    pub color: Option<Rgba>,
    pub tags: Vec<TagId>,
    pub created_at: u64,
    pub updated_at: u64,
    /// Optional PNG thumbnail bytes. Populated by the UI layer when it
    /// re-renders a preview; the library never generates one itself.
    pub thumbnail: Option<Vec<u8>>,
}

/// The single blob persisted under `library.index.v1` in data-store.
///
/// Kept append-only for ids — [`Self::next_*`] counters only ever go
/// up, so a deleted id is never re-used. Simplifies undo / redo and
/// keeps stale references (e.g. a stashed [`ItemId`] in the UI) from
/// silently aliasing a new doc.
#[istmo::message]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LibraryIndex {
    pub version: u32,
    pub next_item_id: u64,
    pub next_folder_id: u64,
    pub next_tag_id: u32,
    pub folders: Vec<Folder>,
    pub items: Vec<Item>,
    pub tags: Vec<Tag>,
}

impl Default for LibraryIndex {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
            next_item_id: 1,
            next_folder_id: 1,
            next_tag_id: 1,
            folders: Vec::new(),
            items: Vec::new(),
            tags: Vec::new(),
        }
    }
}

impl LibraryIndex {
    #[must_use]
    pub fn folder(&self, id: FolderId) -> Option<&Folder> {
        self.folders.iter().find(|f| f.id == id)
    }

    pub fn folder_mut(&mut self, id: FolderId) -> Option<&mut Folder> {
        self.folders.iter_mut().find(|f| f.id == id)
    }

    #[must_use]
    pub fn item(&self, id: ItemId) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn item_mut(&mut self, id: ItemId) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    #[must_use]
    pub fn tag(&self, id: TagId) -> Option<&Tag> {
        self.tags.iter().find(|t| t.id == id)
    }

    pub fn tag_mut(&mut self, id: TagId) -> Option<&mut Tag> {
        self.tags.iter_mut().find(|t| t.id == id)
    }

    /// Walk `id`'s parent chain to detect whether `ancestor` sits
    /// above it. Returns `true` when `id == ancestor`. Used to reject
    /// folder moves that would form a cycle.
    #[must_use]
    pub fn folder_has_ancestor(&self, id: FolderId, ancestor: FolderId) -> bool {
        if id == ancestor {
            return true;
        }
        let mut cursor = id;
        // Bound the walk by the folder count so a corrupted parent
        // chain cannot spin forever.
        for _ in 0..self.folders.len().saturating_add(1) {
            let Some(f) = self.folder(cursor) else {
                return false;
            };
            if f.parent == ancestor {
                return true;
            }
            if f.parent == ROOT_FOLDER {
                return ancestor == ROOT_FOLDER && cursor != ROOT_FOLDER;
            }
            cursor = f.parent;
        }
        false
    }

    pub(super) const fn alloc_folder_id(&mut self) -> FolderId {
        let id = FolderId(self.next_folder_id);
        self.next_folder_id = self.next_folder_id.saturating_add(1);
        id
    }

    pub(super) const fn alloc_item_id(&mut self) -> ItemId {
        let id = ItemId(self.next_item_id);
        self.next_item_id = self.next_item_id.saturating_add(1);
        id
    }

    pub(super) const fn alloc_tag_id(&mut self) -> TagId {
        let id = TagId(self.next_tag_id);
        self.next_tag_id = self.next_tag_id.saturating_add(1);
        id
    }
}

