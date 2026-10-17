#![doc = include_str!("../README.md")]
//!
//! - [`document`]: the editable model (blocks, inline marks, atoms).
//! - [`markdown`]: `CommonMark` + GFM + extensions parsing and writing, in a
//!   rich and a portable dialect.
//! - [`MarkdownEditor`]: the component, editable or read only.
//! - [`EditorController`]: shared state and commands for toolbars and
//!   integrations.
//! - [`Decorator`]: hooks to map links, text patterns, code blocks and
//!   images to custom visuals.
//! - `MarkdownToolbar` (feature `toolbar`): a ready made formatting bar.

mod controller;
mod decorator;
pub mod document;
mod editor;
pub mod markdown;
mod theme;
#[cfg(feature = "toolbar")]
mod toolbar;

pub use controller::{
    ActiveFormat, EditorController, Selection, use_editor_controller, use_editor_controller_with,
};
pub use decorator::{
    CodeContext, Decorator, Decorators, EmbedContext, ImageContext, PrefixTokens, SchemeEmbed,
    SpanStyle, TextMatch,
};
pub use editor::{MarkdownEditor, Suggestion, SuggestionAction, Trigger};
pub use theme::{EditorColors, EditorTheme};
#[cfg(feature = "toolbar")]
pub use toolbar::MarkdownToolbar;

/// Everything needed to embed an editor.
pub mod prelude {
    pub use crate::document::{
        Align, Atom, BlockKind, BlockType, CalloutKind, Document, HeadingLevel, Inline, Link,
        ListKind, MarkFamily, MarkKind, Rgba,
    };
    pub use crate::markdown::{Dialect, Extensions, ParseOptions};
    #[cfg(feature = "toolbar")]
    pub use crate::toolbar::MarkdownToolbar;
    pub use crate::{
        ActiveFormat, CodeContext, Decorator, Decorators, EditorColors, EditorController,
        EditorTheme, EmbedContext, ImageContext, MarkdownEditor, PrefixTokens, SchemeEmbed,
        Selection, SpanStyle, Suggestion, SuggestionAction, TextMatch, Trigger,
        use_editor_controller, use_editor_controller_with,
    };
}
