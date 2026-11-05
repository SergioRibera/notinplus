//! Create-menu entries and the typed result of creating a document.

use std::borrow::Cow;

use bytes::Bytes;
use notein_i18n::tr;

use crate::failure::UserMessage;
use crate::kind::KindColor;

/// What the shell shows between picking an entry and opening the new
/// document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreationFlow {
    /// No sheet: create and open at once (Borrador).
    Direct,
    /// The common create sheet: name, colour, tags, location; plus the
    /// paper picker when the kind has paper.
    Form,
    /// The system file picker first, then the import sheet.
    Import {
        /// Title of the system picker.
        title: Cow<'static, str>,
        /// Accepted extensions, lower case, no dot.
        extensions: Vec<Cow<'static, str>>,
        mime_types: Vec<Cow<'static, str>>,
    },
    /// Pick one option first (a Bible translation). The chosen id
    /// arrives as [`CreationRequest::choice`].
    Choose {
        title: Cow<'static, str>,
        options: Vec<CreationChoice>,
        /// A last row that runs [`crate::DocumentKind::launch`] with its
        /// id instead of creating (Agregar traducción).
        more: Option<CreationChoice>,
        /// After the choice, the create sheet asks for the name, folder
        /// and tags; otherwise the document is made at once.
        named: bool,
    },
    /// The plugin shows its own UI from its face (Biblia first use).
    Custom,
    /// No document is made here: the shell hands the entry to
    /// [`crate::DocumentKind::launch`] (a scratch canvas, a picker).
    Launch,
}

/// One option of a [`CreationFlow::Choose`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreationChoice {
    pub id: Cow<'static, str>,
    pub label: Cow<'static, str>,
    pub subtitle: Cow<'static, str>,
}

/// One entry of the create menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreationEntry {
    /// Stable id passed back to [`crate::DocumentKind::create`].
    pub id: Cow<'static, str>,
    /// Tile and row label ("Lienzo", "Importar PDF").
    pub label: Cow<'static, str>,
    /// Second line ("Escribir a mano").
    pub subtitle: Cow<'static, str>,
    pub icon: Bytes,
    pub color: KindColor,
    pub flow: CreationFlow,
    /// Competes for the four big tiles. Organising entries (folders)
    /// leave it off.
    pub ranked: bool,
    /// Position among the starting tiles of a new install; lower comes
    /// first, `None` never starts as a tile.
    pub starter_rank: Option<u8>,
    /// Offered where a name is all there is (Crear con este nombre, a
    /// search with no results): the entry needs nothing else.
    pub named: bool,
    /// Title of the create sheet; `None` reads "Nuevo <label>".
    pub sheet_title: Option<Cow<'static, str>>,
    /// Hint under the name field; `None` keeps the keyboard hint.
    pub name_hint: Option<Cow<'static, str>>,
    /// The create sheet asks for the folder.
    pub location: bool,
}

impl CreationEntry {
    #[must_use]
    pub fn new(
        id: impl Into<Cow<'static, str>>,
        label: impl Into<Cow<'static, str>>,
        icon: Bytes,
        color: KindColor,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            subtitle: Cow::Borrowed(""),
            icon,
            color,
            flow: CreationFlow::Form,
            ranked: true,
            starter_rank: None,
            named: false,
            sheet_title: None,
            name_hint: None,
            location: true,
        }
    }

    #[must_use]
    pub fn subtitle(mut self, subtitle: impl Into<Cow<'static, str>>) -> Self {
        self.subtitle = subtitle.into();
        self
    }

    #[must_use]
    pub fn flow(mut self, flow: CreationFlow) -> Self {
        self.flow = flow;
        self
    }

    /// Shorthand for an import flow.
    #[must_use]
    pub fn import(
        self,
        title: impl Into<Cow<'static, str>>,
        extensions: &[&'static str],
        mime_types: &[&'static str],
    ) -> Self {
        self.flow(CreationFlow::Import {
            title: title.into(),
            extensions: extensions.iter().map(|e| Cow::Borrowed(*e)).collect(),
            mime_types: mime_types.iter().map(|m| Cow::Borrowed(*m)).collect(),
        })
    }

    #[must_use]
    pub const fn unranked(mut self) -> Self {
        self.ranked = false;
        self
    }

    #[must_use]
    pub const fn starter(mut self, rank: u8) -> Self {
        self.starter_rank = Some(rank);
        self
    }

    /// See [`CreationEntry::named`].
    #[must_use]
    pub const fn named(mut self) -> Self {
        self.named = true;
        self
    }

    /// Title and name hint of the create sheet, and whether it asks for
    /// the folder.
    #[must_use]
    pub fn sheet(
        mut self,
        title: impl Into<Cow<'static, str>>,
        name_hint: Option<impl Into<Cow<'static, str>>>,
        location: bool,
    ) -> Self {
        self.sheet_title = Some(title.into());
        self.name_hint = name_hint.map(Into::into);
        self.location = location;
        self
    }
}

/// What the import sheet shows for a picked file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportPreview {
    /// Name the document gets unless the user types another.
    pub name: String,
    /// One line about the file ("Libro.pdf · 248 páginas · 3 MB").
    pub details: Option<String>,
    /// The first lines of its text.
    pub preview: Option<String>,
}

/// The paper pattern picked in the create sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, bincode::Encode, bincode::Decode)]
pub enum Paper {
    #[default]
    Blank,
    Lines,
    Grid,
    Dots,
}

/// What the create sheet collected.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CreationRequest<'a> {
    /// Name typed by the user; empty means "derive one".
    pub name: &'a str,
    pub paper: Paper,
    /// Paper colour (sRGB), when the kind has paper.
    pub paper_color: Option<[u8; 3]>,
    /// The imported file, for import flows.
    pub file: Option<ImportedFile<'a>>,
    /// The option picked in a [`CreationFlow::Choose`].
    pub choice: Option<&'a str>,
}

/// A file chosen in the system picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImportedFile<'a> {
    pub name: &'a str,
    pub bytes: &'a [u8],
}

/// A new document, ready for the shell to store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Created<C> {
    /// Name for the document; the shell uses it when the user left the
    /// name empty.
    pub name: String,
    pub content: C,
    /// File to keep next to the content (the PDF itself).
    pub attachment: Option<Vec<u8>>,
}

/// Why a creation failed. [`UserMessage::user_message`] is what the
/// user sees; `Display` is for logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreationError {
    UnknownEntry(String),
    /// The imported file cannot be read; the plugin explains why in
    /// plain words, in the interface language (`tr!`), and that text
    /// is shown as is ("It may be damaged or password protected").
    Unreadable(String),
    /// Missing input the flow needs.
    MissingFile,
}

impl std::fmt::Display for CreationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownEntry(id) => write!(f, "unknown creation entry {id}"),
            Self::Unreadable(why) => write!(f, "unreadable file: {why}"),
            Self::MissingFile => f.write_str("no file given"),
        }
    }
}

impl std::error::Error for CreationError {}

impl UserMessage for CreationError {
    fn user_message(&self) -> String {
        match self {
            Self::UnknownEntry(id) => tr!("plugin-create-unknown-entry", "id" => id.as_str()),
            Self::Unreadable(why) => why.clone(),
            Self::MissingFile => tr!("plugin-create-missing-file"),
        }
    }
}
