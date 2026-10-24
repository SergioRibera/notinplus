//! Plugins as the shell runs them: the catalog of installed plugins,
//! the services the shell gives them, and the builtin kinds.

pub mod builtin;
pub mod catalog;
pub mod content;
pub mod dynamic;
pub mod faces;
pub mod host;
pub mod navigate;

pub use catalog::{catalog, kind, kind_info};
