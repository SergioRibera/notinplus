//! Author-facing SDK for notinplus plugins.
//!
//! Reexports the full [`notein_plugin`] surface plus two
//! author-focused helper modules:
//!
//! - [`codec`] — bincode encode / decode for payloads that cross the
//!   trait boundary as `Vec<u8>` (e.g. `list_bookmarks` returning a
//!   bincoded `Vec<Bookmark>`, `update_bookmark` taking a bincoded
//!   `Bookmark`). Plugin authors should go through these helpers
//!   rather than wiring `bincode::encode_to_vec` directly — the shell
//!   uses the exact same codec on the other side, so staying on one
//!   path means wire drift is impossible.
//! - [`build`] — fluent builders for [`ConfigSchema`] and
//!   [`CreationEntry`] so the common cases (declare a bool /
//!   int / float field, declare a `Blank` / `Import` entry) collapse
//!   to one or two lines of plugin code instead of a struct literal
//!   with ten `Vec::new()` calls.
//!
//! The brush engine will be reexported here once the stroke canvas
//! plugin extraction (Phase 2.6) moves it out of the host crate. For
//! now, canvas plugins that need the host's stroke primitives link
//! those modules directly from `notinplus`.

pub use notein_plugin::*;

pub mod build;
pub mod codec;
