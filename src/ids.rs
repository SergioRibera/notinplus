//! UUID newtypes shared across the CRDT-ready data model.
//!
//! Canonical defs live in [`notein_plugin::ids`] — the plugin surface
//! owns them so third-party plugins reference the same wire shape. This
//! module stays as a shim so the rest of the app keeps importing from
//! `crate::ids`.

pub use notein_plugin::{BookmarkId, StrokeId};
