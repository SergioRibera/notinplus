//! Hierarchical table of contents shared by canvas backgrounds and
//! plugin canvases.
//!
//! The shell renders it in the sidebar "Contents" tab. Backgrounds hand
//! node ids back through `CanvasBackground::navigate_to_outline`; plugin
//! canvases turn them into an anchor payload with
//! [`crate::CanvasKind::outline_anchor`], delivered to the open page the
//! same way a chip tap is (`Host::queue_source_anchor`).

use crate::IconType;

/// Opaque identifier for a node inside an [`Outline`]. Producers choose
/// their own numbering; the host treats these as tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutlineNodeId(pub u64);

/// Semantic hint the host uses to pick an icon for an outline row.
/// Shell-known variants map to bundled icons; `Custom` carries
/// plugin-supplied icon bytes the shell renders directly.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OutlineIcon {
    /// Non-navigable grouping row (e.g. "Pages", "Layers"). Default.
    #[default]
    Section,
    /// A page / sheet / card. For PDFs this is one PDF page; for
    /// stacked-paper canvases this is one notebook page.
    Page,
    /// A heading / section anchor inside a text-like document.
    Heading,
    /// A canvas layer entry.
    Layer,
    /// A pinned location (user bookmark).
    Bookmark,
    /// Plugin-supplied icon — the shell renders the bytes directly.
    Custom(IconType),
}

/// One node in an outline tree. Hierarchical — the host flattens the
/// tree depth-first and uses nesting depth for indentation.
#[derive(Debug, Clone)]
pub struct OutlineNode {
    /// Producer-assigned token handed back on activation.
    pub id: OutlineNodeId,
    /// Human label shown as the row title.
    pub label: String,
    /// Optional secondary text (e.g. "p. 12", stroke count).
    pub subtitle: Option<String>,
    /// Semantic icon hint.
    pub icon: OutlineIcon,
    /// Nested children. Empty for leaf nodes.
    pub children: Vec<Self>,
}

impl OutlineNode {
    /// Leaf node with a [`OutlineIcon::Heading`] icon.
    #[must_use]
    pub fn new(id: OutlineNodeId, label: impl Into<String>) -> Self {
        Self {
            id,
            label: label.into(),
            subtitle: None,
            icon: OutlineIcon::Heading,
            children: Vec::new(),
        }
    }
}

/// Hierarchical table of contents.
///
/// Producers that have nothing to show return [`Outline::default`]; the
/// host then falls back to whatever else it can list (pages, layers).
#[derive(Debug, Clone, Default)]
pub struct Outline {
    /// Top-level nodes. Depth-first flattening is a host concern.
    pub nodes: Vec<OutlineNode>,
    /// Monotonic token the host can compare across polls to skip
    /// re-rendering when the tree has not changed. Producers with a
    /// static outline leave this at `0`.
    pub revision: u64,
}

impl Outline {
    /// `true` when there are no nodes to render.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Number of nodes in the whole tree.
    #[must_use]
    pub fn len(&self) -> usize {
        fn count(nodes: &[OutlineNode]) -> usize {
            nodes.iter().map(|node| 1 + count(&node.children)).sum()
        }
        count(&self.nodes)
    }
}
