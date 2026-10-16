//! Shim — `FormInput` now lives in the shared `notein-ui-kit` crate so
//! plugins can embed the same widget. This module re-exports the type
//! to keep every notinplus call site working without an import rename.

pub use notein_ui_kit::FormInput;
