//! Top-level screens.

pub mod bible;
pub mod collab;
pub mod home;
pub mod map;
pub mod mesa;
// Hot reload wants hot-patchable fn components, not the closures the
// harness renders.
#[cfg(all(test, not(feature = "hotreload")))]
mod preview;
pub mod search;
pub mod settings;
pub mod trash;
