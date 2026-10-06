//! Bookmark model.
//!
//! Canonical defs live in [`notein_plugin::bookmark`] — the plugin
//! surface owns them so canvas-plugins (Phase 2.6+) and source-plugins
//! reference the same wire shape as the core canvas. This module stays
//! as a shim so the rest of the app keeps importing from
//! `crate::bookmark`.

pub use notein_plugin::{Bookmark, Rgba, SourceRef, StrokeAnchor, TimestampMs, WorldPoint};
