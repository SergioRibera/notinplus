//! Shared Freya widgets reused across notinplus + its plugins.
//!
//! The kit lives outside `notinplus` itself so third-party plugin
//! authors can build drop-in UI that visually matches the shell —
//! theme palette, form field, color picker, modal (future). All items
//! are plain Freya builders / `Component` impls; no notinplus-only
//! context is required.
//!
//! Keep the surface additive: `pub use` reexports from the sub-modules
//! so consumer `use` lines stay stable even if internals move.

pub mod color_wheel;
pub mod form_input;
pub mod modal;
pub mod theme;

pub use color_wheel::{ColorWheel, DEFAULT_SWATCHES, auto_color, color_swatch_strip};
pub use form_input::FormInput;
pub use modal::{Modal, ModalChrome, ModalController, ModalPlacement, ModalPortal};
