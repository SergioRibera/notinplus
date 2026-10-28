//! Every piece of text the editor and the toolbar show on their own:
//! tooltips, menu entries, block captions and the `/` menu.
//!
//! English is the default. Hosts in another language build a [`Labels`]
//! from a function that matches on every [`Label`], so a label added here
//! fails their build until it is translated:
//!
//! ```rust,ignore
//! theme.labels = Labels::from_fn(|label| match label {
//!     Label::Undo => "Deshacer (Ctrl+Z)",
//!     // ...
//! });
//! ```

use std::rc::Rc;

use crate::document::{BlockType, CalloutKind, HeadingLevel};

macro_rules! labels {
    ($($name:ident = $english:literal,)*) => {
        /// One piece of editor text. Each variant documents its English
        /// text.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Label {
            $(#[doc = $english] $name,)*
        }

        impl Label {
            /// Every label, in declaration order.
            pub const ALL: [Self; COUNT] = [$(Self::$name,)*];

            /// The built in English text.
            #[must_use]
            pub const fn english(self) -> &'static str {
                match self {
                    $(Self::$name => $english,)*
                }
            }
        }

        const COUNT: usize = [$(stringify!($name),)*].len();
    };
}

labels! {
    Undo = "Undo (Ctrl+Z)",
    Redo = "Redo (Ctrl+Y)",
    DefaultFont = "Default font",
    FontSize = "Size",
    DefaultSize = "Default",
    Bold = "Bold (Ctrl+B)",
    Italic = "Italic (Ctrl+I)",
    Underline = "Underline (Ctrl+U)",
    Strikethrough = "Strikethrough (Ctrl+Shift+X)",
    InlineCode = "Inline code (Ctrl+E)",
    Highlight = "Highlight (Ctrl+Shift+H)",
    Superscript = "Superscript (Ctrl+.)",
    Subscript = "Subscript (Ctrl+,)",
    TextColor = "Text color",
    BackgroundColor = "Background color",
    DefaultColor = "Default",
    ClearFormatting = "Clear formatting (Ctrl+\\)",
    Link = "Link (Ctrl+K)",
    ApplyLink = "Apply",
    RemoveLink = "Remove",
    AlignLeft = "Align left",
    AlignCenter = "Center",
    AlignRight = "Align right",
    Justify = "Justify",
    Outdent = "Outdent (Shift+Tab)",
    Indent = "Indent (Tab)",
    Insert = "Insert",
    ReadingMode = "Switch to reading mode",
    EditingMode = "Switch to editing mode",
    Text = "Text",
    Heading = "Heading",
    Heading1 = "Heading 1",
    Heading2 = "Heading 2",
    Heading3 = "Heading 3",
    Quote = "Quote",
    Callout = "Callout",
    BulletedList = "Bulleted list",
    NumberedList = "Numbered list",
    TodoList = "To-do list",
    Code = "Code",
    Math = "Math",
    Term = "Term",
    Definition = "Definition",
    Table = "Table",
    Divider = "Divider",
    CodeBlock = "Code block",
    MathBlock = "Math block",
    TableOfContents = "Table of contents",
    Note = "Note",
    Tip = "Tip",
    Important = "Important",
    Warning = "Warning",
    Caution = "Caution",
    Blocks = "Blocks",
    NoMatches = "No matches",
    PlainParagraph = "Plain paragraph",
    BigHeading = "Big section heading",
    MediumHeading = "Medium section heading",
    SmallHeading = "Small section heading",
    SimpleList = "Simple list",
    OrderedList = "Ordered list",
    Checklist = "Checklist",
    Quotation = "Quotation",
    TexFormula = "TeX formula",
    TermDetail = "Definition list term",
    DefinitionDetail = "Definition list details",
    HorizontalRule = "Horizontal rule",
    SmallTable = "3 x 3 table",
    FromHeadings = "Generated from headings",
    FrontMatter = "Front matter",
    EmptyContents = "Add headings to build the table of contents",
    Block = "Block",
    PlainText = "Plain text",
    SearchLanguage = "Search or type a language",
    UseLanguage = "Use",
    PlainQuote = "Plain quote",
    CopyCode = "Copy",
    Copied = "Copied",
    EditSource = "Edit",
    DoneEditing = "Done",
    Rename = "Rename",
    RenameHint = "Leave empty for the default name",
    Comment = "Comment",
    CommentHint = "Hidden when reading",
    Collapsible = "Collapsible section",
    DetailsSummary = "Summary",
    Image = "Image",
    Properties = "Properties",
    AddProperty = "Add property",
    EditAsText = "Edit as text",
    ShowAsList = "Show as list",
    PropertyName = "Name",
    PropertyValue = "Value",
    RemoveProperty = "Remove",
    Add = "Add",
    Save = "Save",
    Formula = "Formula",
    Fraction = "Fraction",
    SquareRoot = "Square root",
    Power = "Power",
    MathIndex = "Index",
    Sum = "Sum",
    Integral = "Integral",
    Matrix = "Matrix",
    Diagram = "Diagram",
    DiagramDetail = "Flowchart you can edit as text",
    DiagramTemplate = "flowchart TD\n    start[Start] --> question{Decision?}\n    question -->|Yes| done[Do it]\n    question -->|No| stop[Stop]",
    ReplaceImage = "Replace image",
    AltText = "Description",
    AltTextHint = "Describes the image for people who cannot see it",
    RemoveImage = "Remove image",
    AddImage = "Add an image from your files",
}

/// The text shown for every [`Label`]; English by default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Labels(Rc<[&'static str; COUNT]>);

impl Default for Labels {
    fn default() -> Self {
        Self::from_fn(Label::english)
    }
}

impl Labels {
    /// Labels whose text comes from `text`.
    #[must_use]
    pub fn from_fn(text: impl Fn(Label) -> &'static str) -> Self {
        Self(Rc::new(Label::ALL.map(text)))
    }

    /// The text of `label`.
    #[must_use]
    pub fn get(&self, label: Label) -> &'static str {
        self.0[label as usize]
    }

    /// Caption of a block type, `Text` when there is none.
    #[must_use]
    pub fn block(&self, block_type: Option<BlockType>) -> &'static str {
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
    pub fn callout(&self, kind: CalloutKind) -> &'static str {
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
    fn default_is_english() {
        let labels = Labels::default();
        assert_eq!(labels.get(Label::Undo), "Undo (Ctrl+Z)");
        assert_eq!(labels.get(Label::Block), "Block");
    }

    #[test]
    fn from_fn_overrides_every_label() {
        let labels = Labels::from_fn(|_| "x");
        assert!(Label::ALL.iter().all(|label| labels.get(*label) == "x"));
    }
}
