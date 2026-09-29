//! Reusable UI primitives layered on top of freya's base elements.
//!
//! Every component here is a builder in the freya idiom: no `.build()`
//! terminator, chainable setters consume `self`, and the type itself
//! implements [`Component`] (or `Into<Element>`) so it drops into
//! `.child(...)` positions unchanged.

pub mod canvas_create;
pub mod color_wheel;
pub mod fab_menu;
pub mod folder_create;
pub mod modal;

pub use canvas_create::{CanvasCreateSheet, CreateCanvasRequest};
pub use color_wheel::{auto_color, ColorWheel, DEFAULT_SWATCHES};
pub use fab_menu::{FabMenu, FabMenuEntry};
pub use folder_create::{CreateFolderRequest, FolderCreateSheet};
pub use modal::{Modal, ModalController, ModalPlacement, ModalPortal};
