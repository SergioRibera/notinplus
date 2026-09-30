//! Reusable UI primitives layered on top of freya's base elements.
//!
//! Every component here is a builder in the freya idiom: no `.build()`
//! terminator, chainable setters consume `self`, and the type itself
//! implements [`Component`] (or `Into<Element>`) so it drops into
//! `.child(...)` positions unchanged.

pub mod canvas_create;
pub mod chip;
pub mod color_wheel;
pub mod fab_menu;
pub mod folder_create;
pub mod form_input;
pub mod modal;
pub mod tag;
pub mod tag_picker;
pub mod theme;

pub use canvas_create::{CanvasCreateSheet, CreateCanvasRequest};
pub use chip::Chip;
pub use color_wheel::{ColorWheel, DEFAULT_SWATCHES, auto_color};
pub use fab_menu::{FabMenu, FabMenuEntry};
pub use folder_create::{CreateFolderRequest, FolderCreateSheet};
pub use form_input::FormInput;
pub use modal::{Modal, ModalController, ModalPlacement, ModalPortal};
pub use tag::Tag;
pub use tag_picker::TagPicker;
