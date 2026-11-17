//! Document kinds: the typed contract between a plugin and the shell.
//!
//! A [`DocumentKind`] owns one kind of document (Lienzo, PDF, Nota,
//! Biblia, …). Its anchors and stored content are plain Rust types;
//! its search results borrow the plugin's own index through a generic
//! associated type, so a plugin can hand out hits straight from a
//! locked index without collecting them first.

use std::borrow::Cow;

use bytes::Bytes;

use crate::bookmark::SourceRef;
use crate::config::{ConfigField, ConfigValue};
use crate::create::{
    Created, CreationChoice, CreationEntry, CreationError, CreationRequest, ImportPreview,
    ImportedFile,
};
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
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent capabilities of a kind"
)]
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
    /// Any document of this kind answers a reference: one whose document
    /// is gone opens the most recently used one instead of reading as
    /// broken (a verse reads the same in every Bible).
    pub shared_refs: bool,
    /// The user can tint documents of this kind (the card and the
    /// paper colour).
    pub tint: bool,
    /// Documents of this kind serve as the notebook written beside
    /// another document.
    pub notebook: bool,
    /// Documents of this kind keep a body the shell syncs and merges
    /// itself (strokes, collaborative text) next to their content.
    pub body: bool,
    /// Documents of this kind keep [`Records`](crate::services::Records):
    /// keyed values the shell merges per key with other devices and
    /// people, live.
    pub records: bool,
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

impl<A> Hit<A> {
    /// The same hit with its anchor turned into another type.
    #[must_use]
    pub fn map_anchor<B>(self, to: impl FnOnce(A) -> B) -> Hit<B> {
        Hit {
            doc: self.doc,
            anchor: to(self.anchor),
            title: self.title,
            snippet: self.snippet,
            score: self.score,
        }
    }
}

/// A query for [`DocumentKind::search`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchQuery {
    /// What the user typed, trimmed.
    pub text: String,
    /// Most hits the shell will show for this kind.
    pub limit: usize,
    /// Places the user is working near (what the document being written
    /// already cites, what is open beside it), closest first. A kind may
    /// rank hits at or around them first; ignoring it is fine.
    pub near: Vec<SourceRef>,
}

impl SearchQuery {
    #[must_use]
    pub fn new(text: impl Into<String>, limit: usize) -> Self {
        Self {
            text: text.into().trim().to_owned(),
            limit,
            near: Vec::new(),
        }
    }

    /// The query with the places of [`Self::near`].
    #[must_use]
    pub fn near(mut self, near: Vec<SourceRef>) -> Self {
        self.near = near;
        self
    }

    /// Text without case or accents, for [`crate::Folded`] matching.
    #[must_use]
    pub fn folded(&self) -> String {
        crate::text::fold(&self.text)
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

/// Content a place inside a document offers to be quoted elsewhere.
///
/// Built with [`Quote::new`]; more fields may come, so it cannot be
/// written as a struct literal outside this crate.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Quote {
    /// Markdown of what the place holds. With [`Self::interlinear`], what
    /// goes under the words (a translation of them), possibly empty.
    pub body: String,
    /// Where it comes from, as a reader would cite it.
    pub source: String,
    /// The place as text read word by word, each word over how it reads
    /// and what it means (an interlinear Bible). The host lays it out;
    /// where it cannot (an old reader, a plain text export) it shows
    /// [`Interlinear::to_markdown`] followed by [`Self::body`].
    pub interlinear: Option<Interlinear>,
}

impl Quote {
    #[must_use]
    pub fn new(body: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            body: body.into(),
            source: source.into(),
            interlinear: None,
        }
    }

    /// Show the place as `interlinear`, with [`Self::body`] under it.
    #[must_use]
    pub fn with_interlinear(mut self, interlinear: Interlinear) -> Self {
        self.interlinear = Some(interlinear);
        self
    }

    /// The whole quote as markdown: the interlinear words as a line of
    /// text, then the body.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        match &self.interlinear {
            Some(interlinear) if self.body.trim().is_empty() => interlinear.to_markdown(),
            Some(interlinear) => format!("{}\n\n{}", interlinear.to_markdown(), self.body.trim()),
            None => self.body.clone(),
        }
    }
}

/// Text laid out word by word (see [`Quote::interlinear`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Interlinear {
    /// In reading order.
    pub words: Vec<GlossedWord>,
    /// Read from right to left (Hebrew, Arabic): the first word sits on
    /// the right and lines fill leftwards.
    pub right_to_left: bool,
    /// Font family the words are written in, one the plugin registered
    /// (see [`crate::Plugin::fonts`]); `None` uses the text around it.
    pub font_family: Option<String>,
}

impl Interlinear {
    #[must_use]
    pub const fn new(words: Vec<GlossedWord>) -> Self {
        Self {
            words,
            right_to_left: false,
            font_family: None,
        }
    }

    #[must_use]
    pub const fn right_to_left(mut self, right_to_left: bool) -> Self {
        self.right_to_left = right_to_left;
        self
    }

    #[must_use]
    pub fn font_family(mut self, family: impl Into<String>) -> Self {
        self.font_family = Some(family.into());
        self
    }

    /// One line of markdown: each word with how it reads in parentheses
    /// and what it means after it, `*word* (reading) meaning`.
    #[must_use]
    pub fn to_markdown(&self) -> String {
        self.words
            .iter()
            .map(GlossedWord::to_markdown)
            .collect::<Vec<_>>()
            .join("  ")
    }
}

/// One word of an [`Interlinear`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct GlossedWord {
    /// As written in the text.
    pub text: String,
    /// How it reads, in the reader's letters.
    pub reading: Option<String>,
    /// What it means here.
    pub gloss: Option<String>,
    /// Link a tap on the word follows (a `notein://` deep link to the word,
    /// for instance).
    pub link: Option<String>,
}

impl GlossedWord {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn reading(mut self, reading: impl Into<String>) -> Self {
        self.reading = Some(reading.into());
        self
    }

    #[must_use]
    pub fn gloss(mut self, gloss: impl Into<String>) -> Self {
        self.gloss = Some(gloss.into());
        self
    }

    #[must_use]
    pub fn link(mut self, link: impl Into<String>) -> Self {
        self.link = Some(link.into());
        self
    }

    fn to_markdown(&self) -> String {
        let mut out = format!("*{}*", self.text.replace('*', ""));
        if let Some(reading) = &self.reading {
            out.push_str(" (");
            out.push_str(reading);
            out.push(')');
        }
        if let Some(gloss) = &self.gloss {
            out.push(' ');
            out.push_str(gloss);
        }
        out
    }
}

/// A part of this kind's quotes the reader may show or hide (a
/// pronunciation, a grammar line, the verse in another translation).
///
/// The host lists them in the menu of every live quote of the kind and
/// keeps the reader's choice on the quote itself; [`DocumentKind::quote_with`]
/// receives it as [`QuoteParts`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuoteOption {
    /// Stable id, kept in documents: lowercase ASCII letters, digits and
    /// `-` only (others are refused by [`QuoteOption::new`]).
    pub id: Cow<'static, str>,
    /// What the menu says, in the reader's language.
    pub label: Cow<'static, str>,
    /// Shown until the reader hides it.
    pub default: bool,
}

impl QuoteOption {
    /// An option shown by default. `None` when `id` has other characters
    /// than lowercase ASCII letters, digits and `-`.
    #[must_use]
    pub fn new(
        id: impl Into<Cow<'static, str>>,
        label: impl Into<Cow<'static, str>>,
    ) -> Option<Self> {
        let id = id.into();
        let valid = !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        valid.then(|| Self {
            id,
            label: label.into(),
            default: true,
        })
    }

    /// Hidden until the reader shows it.
    #[must_use]
    pub const fn hidden_by_default(mut self) -> Self {
        self.default = false;
        self
    }
}

/// Which [`QuoteOption`]s one quote shows: the reader's changes over each
/// option's default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuoteParts {
    /// Option ids the reader hid although they show by default.
    hidden: Vec<String>,
    /// Option ids the reader showed although they hide by default.
    shown: Vec<String>,
}

impl QuoteParts {
    /// Parts kept as two comma separated id lists, as a document stores
    /// them. Empty items are skipped.
    #[must_use]
    pub fn from_lists(hidden: Option<&str>, shown: Option<&str>) -> Self {
        let list = |ids: Option<&str>| {
            ids.unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
                .collect()
        };
        Self {
            hidden: list(hidden),
            shown: list(shown),
        }
    }

    /// The two lists of [`Self::from_lists`], `None` when empty.
    #[must_use]
    pub fn to_lists(&self) -> (Option<String>, Option<String>) {
        let join = |ids: &[String]| (!ids.is_empty()).then(|| ids.join(","));
        (join(&self.hidden), join(&self.shown))
    }

    /// Whether the quote shows `option`.
    #[must_use]
    pub fn shows(&self, option: &QuoteOption) -> bool {
        self.shows_id(&option.id, option.default)
    }

    /// Whether the quote shows the option `id`, shown by default when
    /// `default`.
    #[must_use]
    pub fn shows_id(&self, id: &str, default: bool) -> bool {
        if default {
            !self.hidden.iter().any(|h| h == id)
        } else {
            self.shown.iter().any(|s| s == id)
        }
    }

    /// Show or hide `option`.
    pub fn set(&mut self, option: &QuoteOption, show: bool) {
        let id = option.id.as_ref();
        self.hidden.retain(|h| h != id);
        self.shown.retain(|s| s != id);
        match (option.default, show) {
            (true, false) => self.hidden.push(id.to_owned()),
            (false, true) => self.shown.push(id.to_owned()),
            (true, true) | (false, false) => {}
        }
    }
}

/// A place inside a document rendered as a picture, to be put in another
/// document as it looks now (a PDF page).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// PNG bytes.
    pub png: Vec<u8>,
    /// Pixel size of `png`.
    pub width: u32,
    pub height: u32,
    /// Where it comes from, as a reader would cite it.
    pub source: String,
}

/// What the selection bar offers inside this kind's documents.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // independent capabilities of a kind
pub struct SelectionSupport {
    /// The five highlighter swatches.
    pub highlight: bool,
    /// "Nota": opens the annotation popover.
    pub annotate: bool,
    /// "Al cuaderno": insert a reference in the notebook beside.
    pub to_notebook: bool,
}

impl SelectionSupport {
    /// Nothing can be selected.
    pub const NONE: Self = Self {
        highlight: false,
        annotate: false,
        to_notebook: false,
    };

    /// Text that can be highlighted, annotated and cited.
    pub const TEXT: Self = Self {
        highlight: true,
        annotate: true,
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

/// A document as a file people can open without NotIn Plus, for the
/// readable export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportFile {
    /// Extension without the dot ("pdf", "md").
    pub extension: Cow<'static, str>,
    /// Media type ("application/pdf"), for the save dialog and the share
    /// sheet.
    pub mime: Cow<'static, str>,
    /// Tells files of one export apart ("3" for page three); the shell
    /// appends it to the document name. Empty for a single file.
    pub tag: String,
    pub bytes: Vec<u8>,
}

impl ExportFile {
    #[must_use]
    pub fn new(
        extension: impl Into<Cow<'static, str>>,
        mime: impl Into<Cow<'static, str>>,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            extension: extension.into(),
            mime: mime.into(),
            tag: String::new(),
            bytes,
        }
    }

    /// The file with `tag` appended to its name.
    #[must_use]
    pub fn tagged(mut self, tag: impl Into<String>) -> Self {
        self.tag = tag.into();
        self
    }
}

/// Why an export with options could not be produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportError {
    /// The value of the option `key` is not usable ("page 40 of 12").
    BadOption { key: String, reason: String },
    /// Rendering failed; `reason` is for the person.
    Failed(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadOption { key, reason } => write!(f, "export option {key}: {reason}"),
            Self::Failed(reason) => write!(f, "export failed: {reason}"),
        }
    }
}

impl std::error::Error for ExportError {}

impl crate::failure::UserMessage for ExportError {
    fn user_message(&self) -> String {
        match self {
            Self::BadOption { reason, .. } | Self::Failed(reason) => reason.clone(),
        }
    }
}

/// What the person picked in the export sheet, by option key. Keys
/// missing here hold their [`ConfigField::default`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExportValues(pub Vec<(String, ConfigValue)>);

impl ExportValues {
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&ConfigValue> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// The string value of `key`, or `default` when unset or not text.
    #[must_use]
    pub fn str_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        match self.get(key) {
            Some(ConfigValue::String(s)) => s,
            _ => default,
        }
    }

    /// The switch `key`, or `default` when unset.
    #[must_use]
    pub fn bool_or(&self, key: &str, default: bool) -> bool {
        match self.get(key) {
            Some(ConfigValue::Bool(b)) => *b,
            _ => default,
        }
    }

    /// Every field of `fields` at its default: what [`DocumentKind::export`]
    /// hands to [`DocumentKind::export_with`].
    #[must_use]
    pub fn defaults(fields: &[ConfigField]) -> Self {
        Self(
            fields
                .iter()
                .map(|field| (field.key.clone(), field.default.clone()))
                .collect(),
        )
    }
}

/// A setting of one document the edit sheet offers as a list (the
/// translation a Bible reads).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KindChoice {
    /// Field label ("Traducción").
    pub label: Cow<'static, str>,
    pub options: Vec<CreationChoice>,
    /// Id of the option the document has now.
    pub current: Option<String>,
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

    /// How many units indexing `content` goes through (a PDF's pages);
    /// 0 when it is quick. The shell shows the progress of large ones.
    fn index_size(&self, content: &Self::Content) -> u32 {
        let _ = content;
        0
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

    /// Run an entry whose flow is [`CreationFlow::Launch`], or the
    /// `more` option of a [`CreationFlow::Choose`]: the plugin shows
    /// what it needs itself.
    ///
    /// [`CreationFlow::Launch`]: crate::create::CreationFlow::Launch
    /// [`CreationFlow::Choose`]: crate::create::CreationFlow::Choose
    fn launch(&self, entry: &str) {
        let _ = entry;
    }

    /// What the import sheet shows for a picked file: the name it
    /// suggests, a line about the file and a preview of its text.
    ///
    /// # Errors
    /// [`CreationError::Unreadable`] when the file is not one this kind
    /// opens; the sheet says why.
    fn inspect(&self, entry: &str, file: ImportedFile<'_>) -> Result<ImportPreview, CreationError> {
        let _ = entry;
        Ok(ImportPreview {
            name: file.name.to_owned(),
            details: Some(file.name.to_owned()),
            preview: None,
        })
    }

    /// The content after the user replaced the kept file with `file`,
    /// keeping what `old` says about the document (highlights, marks).
    ///
    /// # Errors
    /// [`CreationError`] when the kind keeps no file or `file` cannot be
    /// read.
    fn replace_attachment(
        &self,
        file: ImportedFile<'_>,
        old: &Self::Content,
    ) -> Result<Self::Content, CreationError> {
        let _ = (file, old);
        Err(CreationError::MissingFile)
    }

    /// A cover picture (PNG) drawn from the kept file, for the card.
    fn thumbnail(&self, content: &Self::Content, attachment: &[u8]) -> Option<Vec<u8>> {
        let _ = (content, attachment);
        None
    }

    /// The line about the kept file in the edit sheet ("Libro.pdf · 248
    /// páginas · 3 MB").
    fn details(&self, content: &Self::Content) -> Option<String> {
        let _ = content;
        None
    }

    /// The setting the edit sheet offers as a list, if the kind has one.
    fn choice(&self, content: &Self::Content) -> Option<KindChoice> {
        let _ = content;
        None
    }

    /// Apply option `id` of [`DocumentKind::choice`]; `false` when
    /// nothing changed.
    fn choose(&self, content: &mut Self::Content, id: &str) -> bool {
        let _ = (content, id);
        false
    }

    /// The first words of the document, for its drawn cover.
    fn excerpt(&self, content: &Self::Content) -> Option<String> {
        let _ = content;
        None
    }

    /// Everything the document links to, so the shell can show what
    /// points where. `None` for kinds whose links are not in this
    /// content (a canvas keeps them in its strokes).
    fn references(&self, content: &Self::Content) -> Option<Vec<SourceRef>> {
        let _ = content;
        None
    }

    /// Remove highlights, annotations and bookmarks, for a copy shared
    /// without them.
    fn without_marks(&self, content: &mut Self::Content) {
        let _ = content;
    }

    /// The document as a file for people without NotIn Plus, the way bulk
    /// exports want it: every option at its default, first file only. The
    /// shell writes the marks next to it on its own.
    fn export(
        &self,
        doc: DocMeta<'_>,
        content: &Self::Content,
        attachment: Option<&[u8]>,
    ) -> Option<ExportFile> {
        let values = ExportValues::defaults(&self.export_fields(doc, content, attachment));
        self.export_with(doc, content, attachment, &values)
            .ok()?
            .into_iter()
            .next()
    }

    /// Whether "Exportar como…" is offered on documents of this kind.
    fn exports(&self) -> bool {
        false
    }

    /// Options the export sheet shows, as settings fields (format, pages,
    /// background…). Declared per document, so a page picker can name the
    /// page count. Empty means the sheet shows only the file name.
    fn export_fields(
        &self,
        doc: DocMeta<'_>,
        content: &Self::Content,
        attachment: Option<&[u8]>,
    ) -> Vec<ConfigField> {
        let _ = (doc, content, attachment);
        Vec::new()
    }

    /// The document as files under `values`. Several files (one per page)
    /// are zipped by the shell.
    ///
    /// # Errors
    /// [`ExportError`] when an option is unusable or rendering fails.
    fn export_with(
        &self,
        doc: DocMeta<'_>,
        content: &Self::Content,
        attachment: Option<&[u8]>,
        values: &ExportValues,
    ) -> Result<Vec<ExportFile>, ExportError> {
        let _ = (doc, content, attachment, values);
        Ok(Vec::new())
    }

    /// Whether [`Self::quote`] can return content. Kinds that cannot never
    /// get asked, and nobody is offered to quote them.
    fn quotes(&self) -> bool {
        false
    }

    /// What `anchor` inside `doc` holds, to be quoted. `None` when the
    /// place is gone.
    fn quote(&self, doc: DocMeta<'_>, anchor: &Self::Anchor) -> Option<Quote> {
        let _ = (doc, anchor);
        None
    }

    /// Parts of a quote of `anchor` the reader may show or hide; empty (the
    /// default) when it has none. The host shows them in each live
    /// quote's menu, next to its text colour.
    fn quote_options(&self, _anchor: &Self::Anchor) -> Vec<QuoteOption> {
        Vec::new()
    }

    /// [`Self::quote`] showing only the parts `parts` asks for (see
    /// [`Self::quote_options`]). Kinds without options keep the default,
    /// which ignores `parts`.
    fn quote_with(
        &self,
        doc: DocMeta<'_>,
        anchor: &Self::Anchor,
        _parts: &QuoteParts,
    ) -> Option<Quote> {
        self.quote(doc, anchor)
    }

    /// Whether a quote of this kind may keep following its source. Kinds
    /// whose content never changes once written (a PDF file) answer
    /// `false`, and their quotes are always copied.
    fn quotes_live(&self) -> bool {
        true
    }

    /// Whether [`Self::snapshot`] can return pictures. Kinds that cannot
    /// never get asked, and nobody is offered to picture them.
    fn snapshots(&self) -> bool {
        false
    }

    /// `anchor` inside `doc` as pictures, one per page or figure, in
    /// reading order. Empty when the place is gone or cannot be drawn.
    /// Rendering may be slow; the host calls this off the UI thread.
    fn snapshot(
        &self,
        doc: DocMeta<'_>,
        content: &Self::Content,
        attachment: Option<&[u8]>,
        anchor: &Self::Anchor,
    ) -> Vec<Snapshot> {
        let _ = (doc, content, attachment, anchor);
        Vec::new()
    }

    /// Something is about to link to `anchor` inside `doc`: keep that
    /// place findable when the document changes (a heading renamed).
    fn cited(&self, doc: DocId, anchor: &Self::Anchor) {
        let _ = (doc, anchor);
    }
}

#[cfg(test)]
mod quote_parts {
    use super::{QuoteOption, QuoteParts};

    fn options() -> (QuoteOption, QuoteOption) {
        let shown = QuoteOption::new("pronunciation", "Pronunciation").expect("valid id");
        let hidden = QuoteOption::new("senses", "Senses")
            .expect("valid id")
            .hidden_by_default();
        (shown, hidden)
    }

    #[test]
    fn defaults_hold_until_the_reader_changes_them() {
        let (shown, hidden) = options();
        let mut parts = QuoteParts::default();
        assert!(parts.shows(&shown));
        assert!(!parts.shows(&hidden));
        parts.set(&shown, false);
        parts.set(&hidden, true);
        assert!(!parts.shows(&shown));
        assert!(parts.shows(&hidden));
        assert_eq!(
            parts.to_lists(),
            (Some("pronunciation".into()), Some("senses".into()))
        );
        parts.set(&shown, true);
        parts.set(&hidden, false);
        assert_eq!(parts, QuoteParts::default());
        assert_eq!(parts.to_lists(), (None, None));
    }

    #[test]
    fn stored_lists_read_back_and_skip_blanks() {
        let (shown, hidden) = options();
        let parts = QuoteParts::from_lists(Some(" pronunciation,,"), Some("senses"));
        assert!(!parts.shows(&shown));
        assert!(parts.shows(&hidden));
        assert!(
            parts.shows_id("unknown", true),
            "ids nobody stored keep their default"
        );
    }

    #[test]
    fn ids_outside_the_alphabet_are_refused() {
        assert!(QuoteOption::new("Bad Id", "x").is_none());
        assert!(QuoteOption::new("", "x").is_none());
        assert!(QuoteOption::new("ok-2", "x").is_some());
    }
}
