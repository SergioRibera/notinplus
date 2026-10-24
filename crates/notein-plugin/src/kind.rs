//! Document kinds: the typed contract between a plugin and the shell.
//!
//! A [`DocumentKind`] owns one kind of document (Lienzo, PDF, Nota,
//! Biblia, …). Its anchors and stored content are plain Rust types;
//! its search results borrow the plugin's own index through a generic
//! associated type, so a plugin can hand out hits straight from a
//! locked index without collecting them first.

use std::borrow::Cow;

use bytes::Bytes;

use crate::create::{Created, CreationEntry, CreationError, CreationRequest};
use crate::doc::{DocId, DocMeta};
use crate::payload::Payload;

/// Colour family of a kind. The shell resolves it against the active
/// theme, so a PDF stays coral in every appearance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KindColor {
    Canvas,
    Pdf,
    Note,
    Bible,
    Folder,
    Scratch,
    /// Third-party kinds pick their own colour (sRGB).
    Custom([u8; 3]),
}

/// A file the kind keeps next to its content (the original PDF).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    /// Extensions accepted when the file is replaced.
    pub extensions: Vec<Cow<'static, str>>,
}

/// How a kind presents itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindInfo {
    /// Stable id stored with every document of this kind.
    pub id: Cow<'static, str>,
    /// Singular noun for cards and filters ("Lienzo", "PDF").
    pub label: Cow<'static, str>,
    /// Plural for the filter chip ("Lienzos").
    pub plural: Cow<'static, str>,
    /// Header of this kind's group in search results ("En la Biblia").
    pub search_group: Cow<'static, str>,
    /// Lucide-style single colour SVG.
    pub icon: Bytes,
    pub color: KindColor,
    /// Strokes can be drawn over the document.
    pub supports_ink: bool,
    /// Has a paper background the user picks (create / edit sheets).
    pub has_paper: bool,
    pub attachment: Option<Attachment>,
    /// One document of this kind is enough (the Bible): the create menu
    /// opens the existing one instead of making another.
    pub singleton: bool,
}

/// One full-text match.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit<A> {
    /// Document the hit lives in; `None` for kinds whose content is
    /// shared by every document (a Bible translation), which the shell
    /// opens in the user's document of that kind.
    pub doc: Option<DocId>,
    pub anchor: A,
    /// Where the hit is ("Romanos 8:15", "p. 118").
    pub title: String,
    /// Text around the match.
    pub snippet: String,
    /// Higher is better; only compared within one kind.
    pub score: f32,
}

/// A query for [`DocumentKind::search`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchQuery {
    /// What the user typed, trimmed.
    pub text: String,
    /// Most hits the shell will show for this kind.
    pub limit: usize,
}

impl SearchQuery {
    #[must_use]
    pub fn new(text: impl Into<String>, limit: usize) -> Self {
        Self {
            text: text.into().trim().to_owned(),
            limit,
        }
    }

    /// Lower-cased text for simple containment matching.
    #[must_use]
    pub fn folded(&self) -> String {
        self.text.to_lowercase()
    }
}

/// Icon of an outline row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OutlineIcon {
    #[default]
    Section,
    Page,
    Heading,
    Layer,
    Book,
}

/// One row of a document's table of contents.
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineNode<A> {
    pub label: String,
    /// Right-aligned meta ("p. 112").
    pub meta: Option<String>,
    pub icon: OutlineIcon,
    /// Where tapping the row goes; `None` makes it a group header.
    pub anchor: Option<A>,
    pub children: Vec<Self>,
}

impl<A> OutlineNode<A> {
    #[must_use]
    pub fn new(label: impl Into<String>, anchor: Option<A>) -> Self {
        Self {
            label: label.into(),
            meta: None,
            icon: OutlineIcon::Heading,
            anchor,
            children: Vec::new(),
        }
    }

    #[must_use]
    pub fn meta(mut self, meta: impl Into<String>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    #[must_use]
    pub const fn icon(mut self, icon: OutlineIcon) -> Self {
        self.icon = icon;
        self
    }

    #[must_use]
    pub fn children(mut self, children: Vec<Self>) -> Self {
        self.children = children;
        self
    }

    /// Convert the anchors, keeping the tree.
    pub fn map<B>(self, f: &mut impl FnMut(A) -> B) -> OutlineNode<B> {
        OutlineNode {
            label: self.label,
            meta: self.meta,
            icon: self.icon,
            anchor: self.anchor.map(&mut *f),
            children: self.children.into_iter().map(|c| c.map(f)).collect(),
        }
    }
}

/// A bookmark, highlight or annotation listed in the panel's
/// Marcadores tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark<A> {
    pub anchor: A,
    pub title: String,
    /// Body excerpt.
    pub snippet: String,
    /// Highlighter or bookmark colour (sRGB).
    pub color: Option<[u8; 3]>,
    pub updated_at: u64,
}

/// How a reference chip looks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChipView {
    pub label: String,
    /// The target no longer exists.
    pub broken: bool,
}

impl ChipView {
    #[must_use]
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            broken: false,
        }
    }

    #[must_use]
    pub fn broken(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            broken: true,
        }
    }
}

/// What the selection bar offers inside this kind's documents.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent capabilities of a kind
pub struct SelectionSupport {
    /// The five highlighter swatches.
    pub highlight: bool,
    /// "Nota": opens the annotation popover.
    pub annotate: bool,
    /// "Copiar ref" / "Copiar como ref".
    pub copy_reference: bool,
    /// "Al cuaderno": insert a reference in the notebook beside.
    pub to_notebook: bool,
}

impl SelectionSupport {
    /// Nothing can be selected.
    pub const NONE: Self = Self {
        highlight: false,
        annotate: false,
        copy_reference: false,
        to_notebook: false,
    };

    /// Text that can be highlighted, annotated and cited.
    pub const TEXT: Self = Self {
        highlight: true,
        annotate: true,
        copy_reference: true,
        to_notebook: true,
    };
}

/// Text the shell indexes for a document of this kind, produced by
/// [`DocumentKind::index`].
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct IndexReport {
    /// Units indexed so far ("41" of 96 pages).
    pub done: u32,
    /// Units in total; equal to `done` once complete.
    pub total: u32,
}

impl IndexReport {
    #[must_use]
    pub const fn complete(units: u32) -> Self {
        Self {
            done: units,
            total: units,
        }
    }

    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.done >= self.total
    }
}

/// An action only documents of one kind have, listed in the document
/// menu in a group with the kind's colour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindAction {
    pub id: Cow<'static, str>,
    pub label: Cow<'static, str>,
    pub icon: Bytes,
}

/// What the shell does after a kind action ran.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionEffect {
    /// Nothing more.
    Done,
    /// Put text on the clipboard and say so.
    Copy { text: String, toast: String },
    /// Hand a file to the system share sheet.
    Share {
        file_name: String,
        mime: String,
        bytes: Vec<u8>,
    },
    /// Open the document's edit sheet.
    Edit,
}

/// A kind of document.
///
/// Implement this on a plugin type and register it through
/// [`crate::Registrar::kind`]. Anchors and content are typed; the
/// shell stores their bytes and hands them back decoded.
pub trait DocumentKind: Send + Sync + 'static {
    /// A place inside a document (a verse range, a page, a heading).
    type Anchor: Payload + PartialEq;
    /// What the shell stores for each document of this kind.
    type Content: Payload + Default;
    /// Search results, possibly borrowing the plugin's index.
    type Hits<'a>: Iterator<Item = Hit<Self::Anchor>> + 'a
    where
        Self: 'a;

    fn info(&self) -> KindInfo;

    /// Search every document of this kind.
    fn search<'a>(&'a self, query: &'a SearchQuery) -> Self::Hits<'a>;

    /// Refresh the search index for one document after it changed or
    /// was imported. `attachment` is the kept file, if any.
    fn index(
        &self,
        doc: DocMeta<'_>,
        content: &Self::Content,
        attachment: Option<&[u8]>,
    ) -> IndexReport {
        let _ = (doc, content, attachment);
        IndexReport::default()
    }

    /// Documents whose index is still being built (PDF text).
    fn index_status(&self) -> Vec<(DocId, IndexReport)> {
        Vec::new()
    }

    /// Forget a document that left the library.
    fn forget(&self, doc: DocId) {
        let _ = doc;
    }

    /// Table of contents for the panel's Contenido tab.
    fn outline(&self, content: &Self::Content) -> Vec<OutlineNode<Self::Anchor>> {
        let _ = content;
        Vec::new()
    }

    /// Bookmarks and annotations for the panel's Marcadores tab.
    fn marks(&self, content: &Self::Content) -> Vec<Mark<Self::Anchor>> {
        let _ = content;
        Vec::new()
    }

    /// Card meta line ("248 páginas", "14 resaltados"). `None` falls
    /// back to the kind label.
    fn summary(&self, content: &Self::Content) -> Option<String> {
        let _ = content;
        None
    }

    /// Label of a reference into `doc`.
    fn chip(&self, doc: DocMeta<'_>, anchor: &Self::Anchor) -> ChipView {
        let _ = anchor;
        ChipView::new(doc.name)
    }

    /// What the selection bar offers.
    fn selection(&self) -> SelectionSupport {
        SelectionSupport::NONE
    }

    /// Actions only this kind offers in the document menu.
    fn actions(&self) -> Vec<KindAction> {
        Vec::new()
    }

    /// Run action `id` on a document.
    fn run_action(&self, id: &str, doc: DocMeta<'_>, content: &Self::Content) -> ActionEffect {
        let _ = (id, doc, content);
        ActionEffect::Done
    }

    /// Entries this kind adds to the create menu.
    fn creation_entries(&self) -> Vec<CreationEntry> {
        Vec::new()
    }

    /// Produce a new document for `entry`.
    ///
    /// # Errors
    /// [`CreationError`] when the entry is unknown or the import
    /// cannot be read.
    fn create(
        &self,
        entry: &str,
        request: CreationRequest<'_>,
    ) -> Result<Created<Self::Content>, CreationError> {
        let _ = request;
        Err(CreationError::UnknownEntry(entry.to_owned()))
    }
}
