//! Persisted data model for the local `.notinplus` library.
//!
//! Every mutation reads/writes the entire [`LibraryIndex`] blob to
//! `data-store`. The blob is intentionally small — folders / items /
//! tags only. Doc bodies live on the filesystem alongside optional PDF
//! attachments (see [`super::bodies`]).

use core::str::FromStr;

/// Reserved id for the implicit root folder that every fresh index
/// carries. User-created folders use random UUID v4 bytes.
pub const ROOT_FOLDER: FolderId = FolderId([0u8; 16]);

/// Schema version encoded into the index blob. Bump on any breaking
/// change to the surrounding types.
pub const INDEX_VERSION: u32 = 4;

macro_rules! uuid_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[istmo::message]
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
        pub struct $name(pub [u8; 16]);

        impl $name {
            #[must_use]
            pub fn new_v4() -> Self {
                Self(*uuid::Uuid::new_v4().as_bytes())
            }

            #[must_use]
            pub const fn nil() -> Self {
                Self([0u8; 16])
            }

            #[must_use]
            pub const fn is_nil(&self) -> bool {
                let mut i = 0;
                while i < 16 {
                    if self.0[i] != 0 {
                        return false;
                    }
                    i += 1;
                }
                true
            }

            #[must_use]
            pub const fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(bytes)
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                uuid::Uuid::from_bytes(self.0).fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                uuid::Uuid::parse_str(s).map(|u| Self(*u.as_bytes()))
            }
        }
    };
}

uuid_newtype!(
    /// Stable identifier for a canvas / PDF-backed doc.
    ItemId
);
uuid_newtype!(
    /// Stable identifier for a folder. [`ROOT_FOLDER`] uses the nil
    /// UUID; user-created folders carry random UUID v4 bytes.
    FolderId
);
uuid_newtype!(
    /// Stable identifier for a user-defined tag.
    TagId
);
uuid_newtype!(
    /// Stable identifier for a persisted workspace session.
    WorkspaceId
);
uuid_newtype!(
    /// Stable identifier for an explicit link between two items.
    LinkId
);
uuid_newtype!(
    /// Stable identifier for a pane within a single workspace. Scoped
    /// per workspace — a [`PaneId`] from workspace A has no meaning in
    /// B. Default is the nil UUID used by transient layout snapshots
    /// that never persist; the [`Workspace`] allocator rewrites it on
    /// insertion.
    PaneId
);

/// sRGB colour swatch. Alpha included so future themes can honour
/// user-defined translucency.
pub type Rgba = [u8; 4];

/// Which plugin renders an [`Item`] and the opaque payload that plugin
/// receives when the item opens.
#[istmo::message]
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct DocKind {
    /// Id of the plugin that owns the item's face.
    pub plugin_id: String,
    /// Opaque bytes handed to the plugin on open (chosen translation,
    /// document id, …). Empty for plugins that need nothing.
    pub init_bytes: Vec<u8>,
}

impl DocKind {
    #[must_use]
    pub fn plugin(plugin_id: impl Into<String>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            init_bytes: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_init(mut self, bytes: Vec<u8>) -> Self {
        self.init_bytes = bytes;
        self
    }

    /// Whether `plugin_id` owns this kind.
    #[must_use]
    pub fn is(&self, plugin_id: &str) -> bool {
        self.plugin_id == plugin_id
    }
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

/// When and from where something was moved to the trash.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Trashed {
    /// Unix millis of the move.
    pub at: u64,
    /// Folder it lived in; restoring puts it back there, or at the
    /// root when that folder is gone.
    pub from: FolderId,
    /// Shared by everything trashed by one action (a folder and its
    /// content, a document and its notebooks) so they restore together.
    pub batch: u64,
}

/// User-defined tag. Colour is chosen at creation time and can be
/// updated independently of the name.
#[istmo::message]
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Tag {
    pub id: TagId,
    pub name: String,
    pub color: Rgba,
    pub updated_at: u64,
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
    /// `Some` while the folder sits in the trash.
    pub trashed: Option<Trashed>,
}

/// Item metadata. The corresponding doc body lives on disk at
/// [`super::bodies::doc_path`] and, when the plugin carries one, a
/// sibling PDF at [`super::bodies::pdf_path`].
#[istmo::message]
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Item {
    pub id: ItemId,
    pub folder: FolderId,
    pub kind: DocKind,
    pub name: String,
    pub color: Option<Rgba>,
    pub tags: Vec<TagId>,
    pub background: BackgroundStyle,
    pub created_at: u64,
    pub updated_at: u64,
    /// Optional PNG thumbnail bytes. Populated by the UI layer when it
    /// re-renders a preview; the library never generates one itself.
    pub thumbnail: Option<Vec<u8>>,
    /// `Some` while the item sits in the trash.
    pub trashed: Option<Trashed>,
}

/// One open tab inside a pane. Names what it shows via [`TabTarget`];
/// per-tab viewport state continues to live in `prefs::DocView`.
///
/// `plugin_id` names which plugin renders this leaf when the tab mounts.
/// `None` → shell uses the plugin named by the item's [`DocKind`].
/// `Some(id)` is an explicit override — the same item id can appear in
/// sibling leaves rendered by different plugins.
#[istmo::message]
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PaneTab {
    pub target: TabTarget,
    pub plugin_id: Option<String>,
}

/// Identity of one scratch canvas. Two scratch tabs are two different
/// surfaces, so each carries its own id.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ScratchId(pub u64);

impl ScratchId {
    /// Random id for a new scratch canvas.
    #[must_use]
    pub fn fresh() -> Self {
        let bytes = uuid::Uuid::new_v4().into_bytes();
        let mut raw = [0u8; 8];
        raw.copy_from_slice(&bytes[..8]);
        Self(u64::from_le_bytes(raw))
    }
}

/// Where a tab lives: which pane of which workspace. Keys per-view
/// state (viewport) so the same document can sit on two mesas, or two
/// panes, with independent pan and zoom.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ViewSlot {
    pub workspace: WorkspaceId,
    pub pane: PaneId,
}

/// What a tab shows.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TabTarget {
    /// A library item.
    Item(ItemId),
    /// A scratch canvas that never reaches the library. Closing the
    /// tab discards it; a persisted workspace keeps the tab but the
    /// canvas starts blank on the next mount.
    Scratch(ScratchId),
    /// An annotation opened from its popover with the full editor. Lives
    /// only while the app runs; reopening a mesa drops these tabs.
    Annotation(AnnotationId),
}

/// Identifies an annotation being edited (see `crate::annotation`).
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct AnnotationId(pub u64);

impl PaneTab {
    #[must_use]
    pub const fn new(item: ItemId) -> Self {
        Self {
            target: TabTarget::Item(item),
            plugin_id: None,
        }
    }

    #[must_use]
    pub fn scratch() -> Self {
        Self::showing(TabTarget::Scratch(ScratchId::fresh()))
    }

    #[must_use]
    pub const fn showing(target: TabTarget) -> Self {
        Self {
            target,
            plugin_id: None,
        }
    }

    #[must_use]
    pub fn with_plugin(item: ItemId, plugin_id: impl Into<String>) -> Self {
        Self {
            target: TabTarget::Item(item),
            plugin_id: Some(plugin_id.into()),
        }
    }

    /// Library item behind the tab; `None` for a scratch canvas.
    #[must_use]
    pub const fn item(&self) -> Option<ItemId> {
        match self.target {
            TabTarget::Item(id) => Some(id),
            TabTarget::Scratch(_) | TabTarget::Annotation(_) => None,
        }
    }
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

    /// Every leaf, mutably, in the same order as [`Self::leaves`].
    pub fn leaves_mut(&mut self) -> Vec<&mut Pane> {
        let mut out = Vec::new();
        self.collect_leaves_mut(&mut out);
        out
    }

    fn collect_leaves_mut<'a>(&'a mut self, out: &mut Vec<&'a mut Pane>) {
        match self {
            Self::Leaf(p) => out.push(p),
            Self::Split { first, second, .. } => {
                first.collect_leaves_mut(out);
                second.collect_leaves_mut(out);
            }
        }
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
            Self::Split { first, second, .. } => {
                first.find_leaf(id).or_else(|| second.find_leaf(id))
            }
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

    /// Split the leaf with id `target` in two under a new split node
    /// of the given `orientation`. `new_side` says which child the
    /// fresh empty leaf (id `new_id`) takes — the existing leaf fills
    /// the other slot. Ratio starts at `0.5`.
    ///
    /// Returns `true` if the split happened. `false` means no leaf
    /// carried that id — the caller may show a warning or ignore.
    pub fn split_leaf(
        &mut self,
        target: PaneId,
        orientation: SplitOrientation,
        new_id: PaneId,
        new_side: SplitSide,
    ) -> bool {
        match self {
            Self::Leaf(p) if p.id == target => {
                let existing = Self::Leaf(p.clone());
                let fresh = Self::leaf(new_id);
                let (first, second) = match new_side {
                    SplitSide::First => (fresh, existing),
                    SplitSide::Second => (existing, fresh),
                };
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
                first.split_leaf(target, orientation, new_id, new_side)
                    || second.split_leaf(target, orientation, new_id, new_side)
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

    /// Split the leaf `target` and carry the tab at `src_index` of pane
    /// `src` into the fresh leaf (id `new_id`) in one step — what a tab
    /// dropped on a pane edge asks for. Nothing changes unless both the
    /// source tab and the target leaf exist, so a bad drop never leaves
    /// an empty pane behind.
    ///
    /// Returns `true` if the layout changed.
    pub fn move_tab_to_split(
        &mut self,
        src: PaneId,
        src_index: usize,
        target: PaneId,
        orientation: SplitOrientation,
        new_id: PaneId,
        new_side: SplitSide,
    ) -> bool {
        let has_tab = self
            .find_leaf(src)
            .is_some_and(|leaf| src_index < leaf.tabs.len());
        if !has_tab || self.find_leaf(target).is_none() || self.find_leaf(new_id).is_some() {
            return false;
        }
        self.split_leaf(target, orientation, new_id, new_side)
            && self.move_tab(src, src_index, new_id)
    }

    /// Remove the leaf `pane` if it holds no tabs — the usual aftermath
    /// of dragging a pane's last tab elsewhere. The root leaf stays
    /// whatever its state: the shell always keeps one pane.
    ///
    /// Returns `true` if a leaf went away.
    pub fn prune_empty_leaf(&mut self, pane: PaneId) -> bool {
        let empty = self
            .find_leaf(pane)
            .is_some_and(|leaf| leaf.tabs.is_empty());
        empty && self.remove_leaf(pane)
    }

    /// Move the tab at `from` inside `pane` so it sits at `to` (clamped
    /// to the last slot). The active tab stays the same tab, whatever
    /// index it ends up at.
    ///
    /// Returns `true` if the order changed.
    pub fn reorder_tab(&mut self, pane: PaneId, from: usize, to: usize) -> bool {
        let Some(leaf) = self.find_leaf_mut(pane) else {
            return false;
        };
        if from >= leaf.tabs.len() {
            return false;
        }
        let to = to.min(leaf.tabs.len() - 1);
        if from == to {
            return false;
        }
        let tab = leaf.tabs.remove(from);
        leaf.tabs.insert(to, tab);
        let active = leaf.active_tab as usize;
        let active = if active == from {
            to
        } else if from < active && active <= to {
            active - 1
        } else if to <= active && active < from {
            active + 1
        } else {
            active
        };
        leaf.active_tab = u32::try_from(active).unwrap_or(u32::MAX);
        true
    }

    /// Clamp every split ratio into `0.05..=0.95` in-place so a bogus
    /// payload cannot drive the divider off-screen.
    pub fn clamp_ratios(&mut self) {
        if let Self::Split {
            ratio,
            first,
            second,
            ..
        } = self
        {
            *ratio = ratio.clamp(0.05, 0.95);
            first.clamp_ratios();
            second.clamp_ratios();
        }
    }
}

impl PaneLayout {
    /// Point tabs at new item ids (an imported copy); tabs of items not in
    /// `ids` close.
    pub fn retarget_items(&mut self, ids: &std::collections::HashMap<ItemId, ItemId>) {
        for pane in self.leaves_mut() {
            pane.tabs.retain_mut(|tab| match tab.target {
                TabTarget::Item(old) => ids.get(&old).is_some_and(|new| {
                    tab.target = TabTarget::Item(*new);
                    true
                }),
                TabTarget::Scratch(_) => true,
                TabTarget::Annotation(_) => false,
            });
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
    /// The user typed `name`. Until then the shell derives it from the
    /// documents open in the mesa.
    pub named: bool,
    /// Shown first in Home.
    pub pinned: bool,
    pub color: Option<Rgba>,
    pub layout: PaneLayout,
    /// Which leaf has keyboard / input focus. The shell clamps on
    /// read when the pane disappears (close / delete).
    pub focused_pane: Option<PaneId>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl Workspace {
    /// Pane and tab index showing `target`, if any leaf does.
    #[must_use]
    pub fn find_tab(&self, target: TabTarget) -> Option<(PaneId, usize)> {
        self.layout.leaves().iter().find_map(|pane| {
            pane.tabs
                .iter()
                .position(|t| t.target == target)
                .map(|idx| (pane.id, idx))
        })
    }

    /// Fresh single-leaf workspace with a freshly-minted root pane
    /// focused.
    #[must_use]
    pub fn fresh(id: WorkspaceId, name: String, color: Option<Rgba>, now: u64) -> Self {
        let root = PaneId::new_v4();
        Self {
            id,
            name,
            named: false,
            pinned: false,
            color,
            layout: PaneLayout::leaf(root),
            focused_pane: Some(root),
            created_at: now,
            updated_at: now,
        }
    }

    /// Mint a fresh [`PaneId`]. Kept as a method on [`Workspace`] so
    /// call sites that split a pane do not need to import
    /// [`PaneId::new_v4`] directly.
    #[must_use]
    #[allow(clippy::unused_self)]
    pub fn alloc_pane_id(&self) -> PaneId {
        PaneId::new_v4()
    }
}

/// Blob persisted under `library.workspaces.v2`. Kept out of the main
/// `LibraryIndex` so adding workspaces does not break the v1 index
/// decode path on upgrading installs.
#[istmo::message]
#[derive(Clone, PartialEq, Debug)]
pub struct WorkspaceIndex {
    pub version: u32,
    pub active: Option<WorkspaceId>,
    pub workspaces: Vec<Workspace>,
}

impl Default for WorkspaceIndex {
    fn default() -> Self {
        Self {
            version: WORKSPACE_INDEX_VERSION,
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

    pub(super) fn alloc_workspace_id(&self) -> WorkspaceId {
        WorkspaceId::new_v4()
    }
}

/// Schema version for [`WorkspaceIndex`]. Bump on any breaking change
/// to the surrounding types.
pub const WORKSPACE_INDEX_VERSION: u32 = 4;

/// The single blob persisted under `library.index.v2` in data-store.
///
/// Every id is a random UUID v4 so creating + deleting items never
/// re-uses the same byte sequence. Simplifies undo / redo and keeps
/// stale references (e.g. a stashed [`ItemId`] in the UI) from
/// silently aliasing a new doc.
#[istmo::message]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LibraryIndex {
    pub version: u32,
    pub folders: Vec<Folder>,
    pub items: Vec<Item>,
    pub tags: Vec<Tag>,
    /// Pinned items, in the order Home shows them.
    pub pins: Vec<ItemId>,
}

impl Default for LibraryIndex {
    fn default() -> Self {
        Self {
            version: INDEX_VERSION,
            folders: Vec::new(),
            items: Vec::new(),
            tags: Vec::new(),
            pins: Vec::new(),
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

    pub(super) fn alloc_folder_id(&self) -> FolderId {
        FolderId::new_v4()
    }

    pub(super) fn alloc_item_id(&self) -> ItemId {
        ItemId::new_v4()
    }

    pub(super) fn alloc_tag_id(&self) -> TagId {
        TagId::new_v4()
    }
}
