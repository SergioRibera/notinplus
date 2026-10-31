//! Freya-driven reading UI.
//!
//! Everything here is gated behind the `ui` crate feature. Pure-data
//! helpers (chapter planning) live in `chapter.rs` and stay available
//! headless; interactive Freya builders live in `reader.rs`.

pub mod bottom_nav;
pub(crate) mod catalog;
pub mod chapter;
pub(crate) mod face;
pub mod reader;
pub mod search;
pub mod typography;
