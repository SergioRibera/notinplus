//! Every piece of text the editor and the toolbar show on their own:
//! tooltips, menu entries, block captions and the `/` menu.
//!
//! The text comes from this crate's Fluent catalog (`locales/`) in the
//! active [`notein_i18n::Lang`]. Hosts that need other wording build a
//! [`Labels`] from a function that matches on every [`Label`], so a
//! label added here fails their build until they word it:
//!
//! ```rust,ignore
//! theme.labels = Labels::from_fn(|label| match label {
//!     Label::Undo => "Undo",
//!     // ...
//! });
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use notein_i18n::Lang;

use crate::document::{BlockType, CalloutKind, HeadingLevel};

macro_rules! labels {
    ($($name:ident = $id:literal,)*) => {
        /// One piece of editor text. Each variant documents its Fluent
        /// message id.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Label {
            $(#[doc = concat!("`", $id, "`")] $name,)*
        }

        impl Label {
            /// Every label, in declaration order.
            pub const ALL: [Self; COUNT] = [$(Self::$name,)*];

            /// Fluent message id of the label.
            #[must_use]
            pub const fn id(self) -> &'static str {
                match self {
                    $(Self::$name => $id,)*
                }
            }
        }

        const COUNT: usize = [$(stringify!($name),)*].len();
    };
}

labels! {
    Undo = "md-undo",
    Redo = "md-redo",
    DefaultFont = "md-default-font",
    FontSize = "md-font-size",
    DefaultSize = "md-default-size",
    Bold = "md-bold",
    Italic = "md-italic",
    Underline = "md-underline",
    Strikethrough = "md-strikethrough",
    InlineCode = "md-inline-code",
    Highlight = "md-highlight",
    Superscript = "md-superscript",
    Subscript = "md-subscript",
    TextColor = "md-text-color",
    BackgroundColor = "md-background-color",
    DefaultColor = "md-default-color",
    ClearFormatting = "md-clear-formatting",
    Link = "md-link",
    ApplyLink = "md-apply-link",
    RemoveLink = "md-remove-link",
    AlignLeft = "md-align-left",
    AlignCenter = "md-align-center",
    AlignRight = "md-align-right",
    Justify = "md-justify",
    Outdent = "md-outdent",
    Indent = "md-indent",
    Insert = "md-insert",
    ReadingMode = "md-reading-mode",
    EditingMode = "md-editing-mode",
    Text = "md-text",
    Heading = "md-heading",
    Heading1 = "md-heading-1",
    Heading2 = "md-heading-2",
    Heading3 = "md-heading-3",
    Quote = "md-quote",
    Callout = "md-callout",
    BulletedList = "md-bulleted-list",
    NumberedList = "md-numbered-list",
    TodoList = "md-todo-list",
    Code = "md-code",
    Math = "md-math",
    Term = "md-term",
    Definition = "md-definition",
    Table = "md-table",
    Divider = "md-divider",
    CodeBlock = "md-code-block",
    MathBlock = "md-math-block",
    TableOfContents = "md-table-of-contents",
    Note = "md-note",
    Tip = "md-tip",
    Important = "md-important",
    Warning = "md-warning",
    Caution = "md-caution",
    Blocks = "md-blocks",
    NoMatches = "md-no-matches",
    PlainParagraph = "md-plain-paragraph",
    BigHeading = "md-big-heading",
    MediumHeading = "md-medium-heading",
    SmallHeading = "md-small-heading",
    SimpleList = "md-simple-list",
    OrderedList = "md-ordered-list",
    Checklist = "md-checklist",
    Quotation = "md-quotation",
    TexFormula = "md-tex-formula",
    TermDetail = "md-term-detail",
    DefinitionDetail = "md-definition-detail",
    HorizontalRule = "md-horizontal-rule",
    SmallTable = "md-small-table",
    FromHeadings = "md-from-headings",
    FrontMatter = "md-front-matter",
    EmptyContents = "md-empty-contents",
    Block = "md-block",
    PlainText = "md-plain-text",
    SearchLanguage = "md-search-language",
    PlainQuote = "md-plain-quote",
    CopyCode = "md-copy-code",
    Copied = "md-copied",
    EditSource = "md-edit-source",
    DoneEditing = "md-done-editing",
    Rename = "md-rename",
    RenameHint = "md-rename-hint",
    Comment = "md-comment",
    CommentHint = "md-comment-hint",
    Collapsible = "md-collapsible",
    DetailsSummary = "md-details-summary",
    Image = "md-image",
    Properties = "md-properties",
    AddProperty = "md-add-property",
    EditAsText = "md-edit-as-text",
    ShowAsList = "md-show-as-list",
    PropertyName = "md-property-name",
    PropertyValue = "md-property-value",
    RemoveProperty = "md-remove-property",
    Add = "md-add",
    Save = "md-save",
    Formula = "md-formula",
    Fraction = "md-fraction",
    SquareRoot = "md-square-root",
    Power = "md-power",
    MathIndex = "md-math-index",
    Sum = "md-sum",
    Integral = "md-integral",
    Matrix = "md-matrix",
    Diagram = "md-diagram",
    DiagramDetail = "md-diagram-detail",
    DiagramTemplate = "md-diagram-template",
    ReplaceImage = "md-replace-image",
    AltText = "md-alt-text",
    AltTextHint = "md-alt-text-hint",
    RemoveImage = "md-remove-image",
    AddImage = "md-add-image",
    TableAddRow = "md-table-add-row",
    TableAddColumn = "md-table-add-column",
    TableRemoveRow = "md-table-remove-row",
    TableRemoveColumn = "md-table-remove-column",
    Contents = "md-contents",
}

impl Label {
    /// The catalog text of the label in the active language.
    #[must_use]
    pub fn text(self) -> String {
        crate::LOCALES.text(self.id())
    }
}

/// The text shown for every [`Label`]; the catalog's text in the active
/// language by default, formatted once per language and thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Labels(Rc<[String; COUNT]>);

impl Default for Labels {
    fn default() -> Self {
        thread_local! {
            static CATALOG: RefCell<Option<(Lang, Labels)>> = const { RefCell::new(None) };
        }
        let lang = Lang::current();
        CATALOG.with_borrow_mut(|cached| match cached {
            Some((cached_lang, labels)) if *cached_lang == lang => labels.clone(),
            _ => cached.insert((lang, Self::from_fn(Label::text))).1.clone(),
        })
    }
}

impl Labels {
    /// Labels whose text comes from `text`.
    #[must_use]
    pub fn from_fn<S: Into<String>>(text: impl Fn(Label) -> S) -> Self {
        Self(Rc::new(Label::ALL.map(|label| text(label).into())))
    }

    /// The text of `label`.
    #[must_use]
    pub fn get(&self, label: Label) -> &str {
        &self.0[label as usize]
    }

    /// Caption of a block type, `Text` when there is none.
    #[must_use]
    pub fn block(&self, block_type: Option<BlockType>) -> &str {
        self.get(match block_type {
            Some(BlockType::Heading(HeadingLevel::H1)) => Label::Heading1,
            Some(BlockType::Heading(HeadingLevel::H2)) => Label::Heading2,
            Some(BlockType::Heading(HeadingLevel::H3)) => Label::Heading3,
            Some(BlockType::Heading(_)) => Label::Heading,
            Some(BlockType::Quote) => Label::Quote,
            Some(BlockType::Callout(_)) => Label::Callout,
            Some(BlockType::BulletList) => Label::BulletedList,
            Some(BlockType::OrderedList) => Label::NumberedList,
            Some(BlockType::TaskList) => Label::TodoList,
            Some(BlockType::Code) => Label::Code,
            Some(BlockType::Math) => Label::Math,
            Some(BlockType::Term) => Label::Term,
            Some(BlockType::Definition) => Label::Definition,
            Some(BlockType::Paragraph) | None => Label::Text,
        })
    }

    /// Default title of a callout.
    #[must_use]
    pub fn callout(&self, kind: CalloutKind) -> &str {
        self.get(match kind {
            CalloutKind::Note => Label::Note,
            CalloutKind::Tip => Label::Tip,
            CalloutKind::Important => Label::Important,
            CalloutKind::Warning => Label::Warning,
            CalloutKind::Caution => Label::Caution,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_label_has_catalog_text() {
        let labels = Labels::default();
        let missing: Vec<_> = Label::ALL
            .into_iter()
            .filter(|label| labels.get(*label) == label.id())
            .collect();
        assert!(missing.is_empty(), "{missing:?}");
    }

    #[test]
    fn diagram_template_keeps_mermaid_braces_and_indent() {
        let template = Label::DiagramTemplate.text();
        assert!(
            template.starts_with("flowchart TD\n    ")
                && template.contains('{')
                && template.contains('}'),
            "{template:?}"
        );
    }

    #[test]
    fn from_fn_overrides_every_label() {
        let labels = Labels::from_fn(|_| "x");
        assert!(Label::ALL.iter().all(|label| labels.get(*label) == "x"));
    }
}
