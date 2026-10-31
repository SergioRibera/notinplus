//! Notein design system: themes, tokens and the components every
//! screen and plugin is built from.
//!
//! Plugins depend on this crate so their UI matches the shell. All
//! colours come from [`theme::palette`]; all spacing, radii and type
//! sizes from [`tokens`]; layouts adapt with [`width::WidthClass`].

notein_i18n::catalog!();

/// Wraps a future so the profiler records it as an async span named
/// `$name`, plus one plain span per poll.
#[cfg(feature = "sismo")]
macro_rules! traced {
    ($name:expr, $fut:expr $(,)?) => {{
        static CALLSITE: ::sismo::Callsite = ::sismo::Callsite::new(
            ::sismo::__private::CallsiteKind::Span,
            $name,
            module_path!(),
            file!(),
            line!(),
        );
        ::sismo::Instrumented::new(&CALLSITE, $fut)
    }};
}

#[cfg(not(feature = "sismo"))]
macro_rules! traced {
    ($name:expr, $fut:expr $(,)?) => {
        $fut
    };
}

pub mod color_wheel;
pub mod components;
pub mod modal;
pub mod theme;
pub mod tokens;
pub mod viewport;
pub mod width;

pub use color_wheel::{ColorWheel, DEFAULT_SWATCHES, auto_color, color_swatch_strip};
pub use components::*;
pub use modal::{
    Modal, ModalChrome, ModalController, ModalDrag, ModalMotion, ModalPlacement, ModalPortal,
};
pub use theme::{palette, reduce_motion};
pub use viewport::{
    Gutters, PaneArea, ViewportArea, clamp_popup_left, clamp_popup_top, clamp_popup_top_from_point,
    place_popup, safe_frame,
};
pub use width::{WidthClass, width_class};
