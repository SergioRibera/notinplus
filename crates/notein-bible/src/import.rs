//! Import pipelines — biblionlabs catalogue downloader (8.2) + OSIS XML
//! parser (8.3). Both land rows into the shared [`crate::db::Db`].
//!
//! The HTTP side is gated behind the `importer` crate feature so
//! headless usages (bin plugins, tests) do not pull in `reqwest`.

#[cfg(feature = "importer")]
pub mod biblionlabs;

#[cfg(feature = "importer")]
pub mod helloao;

pub mod osis;
