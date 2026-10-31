//! Reusable UI primitives layered on top of freya's base elements.
//!
//! Every component here is a builder in the freya idiom: no `.build()`
//! terminator, chainable setters consume `self`, and the type itself
//! implements [`Component`] (or `Into<Element>`) so it drops into
//! `.child(...)` positions unchanged.

pub mod drag_drop;
pub mod modal;

pub use drag_drop::{DragZone, DropZone, use_drag};
pub use modal::{Modal, ModalController, ModalPlacement, ModalPortal};
