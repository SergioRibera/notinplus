//! Reusable UI primitives layered on top of freya's base elements.
//!
//! Every component here is a builder in the freya idiom: no `.build()`
//! terminator, chainable setters consume `self`, and the type itself
//! implements [`Component`] (or `Into<Element>`) so it drops into
//! `.child(...)` positions unchanged.

pub mod color_wheel;
pub mod drag_drop;
pub mod fab_menu;
pub mod form_input;
pub mod modal;
pub mod note_editor;
pub mod theme;

pub use color_wheel::{ColorWheel, DEFAULT_SWATCHES, auto_color};
pub use drag_drop::{DragZone, DropZone, use_drag};
pub use fab_menu::{FabMenu, FabMenuEntry};
pub use form_input::FormInput;
pub use modal::{Modal, ModalController, ModalPlacement, ModalPortal};
pub use note_editor::{
    ChipTapCloseCallback, NoteEditor, set_show_markdown_syntax, show_markdown_syntax,
};
