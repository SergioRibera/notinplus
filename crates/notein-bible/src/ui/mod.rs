//! Freya-driven reading UI.
//!
//! Everything here is gated behind the `ui` crate feature. Pure-data
//! helpers (chapter planning) live in `chapter.rs` and stay available
//! headless; interactive Freya builders live in `reader.rs`.

pub mod chapter;
pub mod reader;
pub mod theme;
pub mod typography;
