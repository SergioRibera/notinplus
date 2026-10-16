//! Shim — `ColorWheel` + swatch helpers moved to `notein-ui-kit`.
//! Reexported here so existing notinplus imports (`crate::components::
//! ColorWheel`, etc.) keep resolving.

pub use notein_ui_kit::color_wheel::{
    ColorWheel, DEFAULT_SWATCHES, auto_color, color_swatch_strip,
};
