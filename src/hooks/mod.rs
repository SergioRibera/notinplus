//! Reusable freya hooks shared across route components.

pub mod safe_area;

pub use safe_area::{SafeAreaOverride, provide_safe_area_override, use_safe_area_insets};
