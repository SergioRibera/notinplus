//! Reusable UI primitives layered on top of freya's base elements.
//!
//! Every component here is a builder in the freya idiom: no `.build()`
//! terminator, chainable setters consume `self`, and the type itself
//! implements [`Component`] (or `Into<Element>`) so it drops into
//! `.child(...)` positions unchanged.

pub mod color_wheel;
pub mod fab_menu;
pub mod modal;

pub use color_wheel::{ColorWheel, DEFAULT_SWATCHES};
pub use fab_menu::{FabMenu, FabMenuEntry};
pub use modal::{Modal, ModalController, ModalPlacement, ModalPortal};
