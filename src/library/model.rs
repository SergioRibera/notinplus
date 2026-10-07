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

/// Stable identifier for a persisted workspace session.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct WorkspaceId(pub u32);

/// Stable identifier for a pane within a single workspace. Scoped per
/// workspace — a `PaneId` from workspace A has no meaning in B. Ids
/// never alias within a workspace (allocator is monotonic + append-
/// only, same discipline as [`ItemId`]).
///
/// `PaneId(0)` is the reserved "unallocated" sentinel used by the
/// default [`Pane`] shape; the workspace allocator always hands out
/// values ≥ 1.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct PaneId(pub u32);

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

/// Paper pattern rendered underneath the canvas strokes. Only the
/// selection lives here — the renderer in `freya-canvas-bg` decides
/// how each variant is drawn.
#[istmo::message]
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum BackgroundStyle {
    /// No pattern, solid background color.
    #[default]
    Blank,
    /// Horizontal ruled lines.
    Line,
    /// Square grid.
    Grid,
    /// Dotted grid.
    DotGrid,
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
    pub background: BackgroundStyle,
    pub created_at: u64,
    pub updated_at: u64,
    /// Optional PNG thumbnail bytes. Populated by the UI layer when it
    /// re-renders a preview; the library never generates one itself.
    pub thumbnail: Option<Vec<u8>>,
}

/// One open tab inside a pane. References the underlying item by id;
/// per-tab viewport state continues to live in `prefs::DocView`.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PaneTab {
    pub item: ItemId,
}

/// Leaf of a [`PaneLayout`] — carries its tab strip plus the index of
/// the currently-visible tab.
///
/// `id` is unique within its owning [`Workspace`]. The default
/// constructor yields `PaneId(0)` so transient layout snapshots that
/// never persist stay cheap; the [`Workspace`] allocator rewrites it
/// on insertion.
#[istmo::message]
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct Pane {
    pub id: PaneId,
    pub tabs: Vec<PaneTab>,
    /// Index into `tabs`. Out-of-range → the UI falls back to 0; the
    /// shell clamps when it reads so an older build cannot wedge on a
    /// stale value.
    pub active_tab: u32,
}

/// Direction the divider inside a [`PaneLayout::Split`] runs.
///
/// * `Horizontal` → the two children sit side-by-side; the divider is
///   a vertical bar the user drags left/right.
/// * `Vertical` → the two children stack top/bottom; the divider is a
///   horizontal bar the user drags up/down.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SplitOrientation {
    Horizontal,
    Vertical,
}

/// Which child of a [`PaneLayout::Split`] a traversal step walks into.
/// Together with the orientation (implicit, read from the node itself)
/// this is enough to address any split node in the tree.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SplitSide {
    First,
    Second,
}

/// Recursive binary tree describing a workspace's pane layout. Every
/// leaf is a [`Pane`]; every internal node is a split along one axis
/// with the drag position stored as `ratio` (fraction of the parent's
/// extent allocated to the first child, clamped to `0.05..=0.95` by
/// the shell).
///
/// Not `Eq` / `Hash` because the `ratio` field is `f32`; the layout is
/// compared structurally only via `PartialEq`.
#[istmo::message]
#[derive(Clone, PartialEq, Debug)]
pub enum PaneLayout {
    Leaf(Pane),
    Split {
        orientation: SplitOrientation,
        /// Fraction of the parent extent given to `first` — the shell
        /// clamps on load so a corrupted ratio cannot push the handle
        /// off-screen.
        ratio: f32,
        first: Box<PaneLayout>,
        second: Box<PaneLayout>,
    },
}

impl PaneLayout {
    /// Convenience: wrap a leaf with the given id. Used by
    /// [`Workspace::new_leaf`] when minting a fresh layout.
    #[must_use]
    pub fn leaf(id: PaneId) -> Self {
        Self::Leaf(Pane {
            id,
            tabs: Vec::new(),
            active_tab: 0,
        })
    }

    /// Empty leaf whose id is `PaneId(0)`. The workspace allocator is
    /// expected to rewrite the id before persisting.
    #[must_use]
    pub fn empty() -> Self {
        Self::Leaf(Pane::default())
    }

    /// Walk every leaf, yielding a reference to each pane in a stable
    /// depth-first (first-then-second) order. The shell uses this to
    /// hand ids to the renderer / event router.
    pub fn leaves(&self) -> Vec<&Pane> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves<'a>(&'a self, out: &mut Vec<&'a Pane>) {
        match self {
            Self::Leaf(p) => out.push(p),
            Self::Split { first, second, .. } => {
                first.collect_leaves(out);
                second.collect_leaves(out);
            }
        }
    }

    /// Look up a leaf by id.
    #[must_use]
    pub fn find_leaf(&self, id: PaneId) -> Option<&Pane> {
        match self {
            Self::Leaf(p) if p.id == id => Some(p),
            Self::Leaf(_) => None,
            Self::Split { first, second, .. } => first.find_leaf(id).or_else(|| second.find_leaf(id)),
        }
    }

    /// Look up a leaf by id (mut).
    pub fn find_leaf_mut(&mut self, id: PaneId) -> Option<&mut Pane> {
        match self {
            Self::Leaf(p) if p.id == id => Some(p),
            Self::Leaf(_) => None,
            Self::Split { first, second, .. } => {
                if let Some(p) = first.find_leaf_mut(id) {
                    Some(p)
                } else {
                    second.find_leaf_mut(id)
                }
            }
        }
    }

    /// Split the leaf with id `target` in two: the existing leaf
    /// becomes the `first` child, a fresh empty leaf (id `new_id`)
    /// becomes the `second` child, under a `Split` with the given
    /// `orientation` and ratio `0.5`.
    ///
    /// Returns `true` if the split happened. `false` means no leaf
    /// carried that id — the caller may show a warning or ignore.
    pub fn split_leaf(
        &mut self,
        target: PaneId,
        orientation: SplitOrientation,
        new_id: PaneId,
    ) -> bool {
        match self {
            Self::Leaf(p) if p.id == target => {
                let first = Self::Leaf(p.clone());
                let second = Self::leaf(new_id);
                *self = Self::Split {
                    orientation,
                    ratio: 0.5,
                    first: Box::new(first),
                    second: Box::new(second),
                };
                true
            }
            Self::Leaf(_) => false,
            Self::Split { first, second, .. } => {
                first.split_leaf(target, orientation, new_id)
                    || second.split_leaf(target, orientation, new_id)
            }
        }
    }

    /// Remove the leaf with id `target`. The sibling collapses into
    /// the parent slot — a `Split` with only one leaf is nonsense, so
    /// the pair is replaced by whichever child survived.
    ///
    /// Returns `true` if the removal happened. `false` means the id
    /// did not match any leaf **or** the target was the root (the
    /// shell always keeps at least one leaf; use workspace delete to
    /// get rid of the last pane).
    pub fn remove_leaf(&mut self, target: PaneId) -> bool {
        // Root leaf cannot be removed from here — handled by caller.
        if let Self::Leaf(p) = self {
            if p.id == target {
                return false;
            }
            return false;
        }

        // Walk the tree; when we land on a Split whose immediate child
        // is the target, replace the Split with the surviving sibling.
        let mut replacement: Option<Self> = None;
        let found = if let Self::Split { first, second, .. } = self {
            match (&**first, &**second) {
                (Self::Leaf(p), _) if p.id == target => {
                    replacement = Some((**second).clone());
                    true
                }
                (_, Self::Leaf(p)) if p.id == target => {
                    replacement = Some((**first).clone());
                    true
                }
                _ => first.remove_leaf(target) || second.remove_leaf(target),
            }
        } else {
            false
        };
        if let Some(child) = replacement {
            *self = child;
        }
        found
    }

    /// Overwrite the `ratio` of the split node addressed by `path`.
    /// Each entry in `path` says which child to descend into; the final
    /// node reached must be a [`PaneLayout::Split`]. Ratio is clamped
    /// into `0.05..=0.95` to match [`Self::clamp_ratios`].
    ///
    /// Returns `true` on a hit. `false` means the path either walked
    /// off a leaf early or landed on something that was not a split.
    pub fn set_ratio_at(&mut self, path: &[SplitSide], ratio: f32) -> bool {
        let clamped = ratio.clamp(0.05, 0.95);
        let mut cursor: &mut Self = self;
        for step in path {
            match cursor {
                Self::Leaf(_) => return false,
                Self::Split { first, second, .. } => {
                    cursor = match step {
                        SplitSide::First => first.as_mut(),
                        SplitSide::Second => second.as_mut(),
                    };
                }
            }
        }
        match cursor {
            Self::Split { ratio: r, .. } => {
                *r = clamped;
                true
            }
            Self::Leaf(_) => false,
        }
    }

    /// Move the tab at `src_index` within pane `src` to the end of
    /// pane `dst` and mark it active there. No-op when `src == dst`
    /// (ordering within a single pane is a separate op) or when either
    /// pane / index is missing.
    ///
    /// Returns `true` on a hit so the caller can decide whether to
    /// persist. `src` keeps the usual active-index clamp after removal.
    pub fn move_tab(&mut self, src: PaneId, src_index: usize, dst: PaneId) -> bool {
        if src == dst {
            return false;
        }
        let tab = {
            let leaf = match self.find_leaf_mut(src) {
                Some(leaf) => leaf,
                None => return false,
            };
            if src_index >= leaf.tabs.len() {
                return false;
            }
            let tab = leaf.tabs.remove(src_index);
            let new_len = leaf.tabs.len();
            let active = leaf.active_tab as usize;
            leaf.active_tab = if new_len == 0 {
                0
            } else if active >= new_len {
                u32::try_from(new_len.saturating_sub(1)).unwrap_or(u32::MAX)
            } else {
                leaf.active_tab
            };
            tab
        };
        let Some(dst_leaf) = self.find_leaf_mut(dst) else {
            // Reinsert at the original slot so a missing destination is
            // observable but not destructive. Index may have shifted
            // past the end — push instead of insert in that case.
            if let Some(src_leaf) = self.find_leaf_mut(src) {
                if src_index <= src_leaf.tabs.len() {
                    src_leaf.tabs.insert(src_index, tab);
                } else {
                    src_leaf.tabs.push(tab);
                }
                src_leaf.active_tab =
                    u32::try_from(src_index.min(src_leaf.tabs.len().saturating_sub(1)))
                        .unwrap_or(u32::MAX);
            }
            return false;
        };
        dst_leaf.tabs.push(tab);
        dst_leaf.active_tab =
            u32::try_from(dst_leaf.tabs.len().saturating_sub(1)).unwrap_or(u32::MAX);
        true
    }

    /// Clamp every split ratio into `0.05..=0.95` in-place so a bogus
    /// payload cannot drive the divider off-screen.
    pub fn clamp_ratios(&mut self) {
        if let Self::Split {
            ratio, first, second, ..
        } = self
        {
            *ratio = ratio.clamp(0.05, 0.95);
            first.clamp_ratios();
            second.clamp_ratios();
        }
    }
}

impl Default for PaneLayout {
    fn default() -> Self {
        Self::empty()
    }
}

/// Persisted workspace. The user flips between workspaces from the
/// shell switcher — each one keeps its own pane layout and tab set.
///
/// Not `Eq` / `Hash` because [`PaneLayout`] carries an `f32` ratio;
/// comparisons through `PartialEq` only.
#[istmo::message]
#[derive(Clone, PartialEq, Debug)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub color: Option<Rgba>,
    pub layout: PaneLayout,
    /// Monotonic counter the workspace hands out when a new leaf is
    /// minted (e.g. on split). Starts at `1` so `PaneId(0)` reads as
    /// "unallocated" everywhere.
    pub next_pane_id: u32,
    /// Which leaf has keyboard / input focus. The shell clamps on
    /// read when the pane disappears (close / delete).
    pub focused_pane: Option<PaneId>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl Workspace {
    /// Fresh single-leaf workspace with id `PaneId(1)` focused.
    #[must_use]
    pub fn fresh(id: WorkspaceId, name: String, color: Option<Rgba>, now: u64) -> Self {
        let root = PaneId(1);
        Self {
            id,
            name,
            color,
            layout: PaneLayout::leaf(root),
            next_pane_id: 2,
            focused_pane: Some(root),
            created_at: now,
            updated_at: now,
        }
    }

    /// Allocate the next [`PaneId`] for this workspace.
    pub fn alloc_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_pane_id);
        self.next_pane_id = self.next_pane_id.saturating_add(1);
        id
    }
}

/// Blob persisted under `library.workspaces.v2`. Kept out of the main
/// `LibraryIndex` so adding workspaces does not break the v1 index
/// decode path on upgrading installs.
#[istmo::message]
#[derive(Clone, PartialEq, Debug)]
pub struct WorkspaceIndex {
    pub version: u32,
    pub next_workspace_id: u32,
    pub active: Option<WorkspaceId>,
    pub workspaces: Vec<Workspace>,
}

impl Default for WorkspaceIndex {
    fn default() -> Self {
        Self {
            version: WORKSPACE_INDEX_VERSION,
            next_workspace_id: 1,
            active: None,
            workspaces: Vec::new(),
        }
    }
}

impl WorkspaceIndex {
    #[must_use]
    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.iter().find(|w| w.id == id)
    }

    pub fn workspace_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.workspaces.iter_mut().find(|w| w.id == id)
    }

    pub(super) const fn alloc_workspace_id(&mut self) -> WorkspaceId {
        let id = WorkspaceId(self.next_workspace_id);
        self.next_workspace_id = self.next_workspace_id.saturating_add(1);
        id
    }
}

/// Schema version for [`WorkspaceIndex`]. Bump on any breaking change
/// to the surrounding types.
pub const WORKSPACE_INDEX_VERSION: u32 = 1;

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
