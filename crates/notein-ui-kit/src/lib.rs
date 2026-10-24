//! Notein design system: themes, tokens and the components every
//! screen and plugin is built from.
//!
//! Plugins depend on this crate so their UI matches the shell. All
//! colours come from [`theme::palette`]; all spacing, radii and type
//! sizes from [`tokens`]; layouts adapt with [`width::WidthClass`].

pub mod color_wheel;
pub mod components;
pub mod modal;
pub mod theme;
pub mod tokens;
pub mod viewport;
pub mod width;

pub use color_wheel::{ColorWheel, DEFAULT_SWATCHES, auto_color, color_swatch_strip};
pub use components::*;
pub use modal::{Modal, ModalChrome, ModalController, ModalPlacement, ModalPortal};
pub use theme::{palette, reduce_motion};
pub use viewport::{
    PaneArea, ViewportArea, clamp_popup_left, clamp_popup_top, clamp_popup_top_from_point,
};
pub use width::{WidthClass, width_class};
