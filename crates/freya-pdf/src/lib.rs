//! PDF rendering backend for Freya canvases.
//!
//! Ships a [`CanvasBackground`](freya_canvas_bg::CanvasBackground)
//! implementation ([`PdfBackground`]) that virtualises page rendering
//! for arbitrarily large documents: only visible pages request bitmaps,
//! a worker pool drives `PDFium` off the UI thread, and an LRU cache
//! keyed by (page, zoom bucket, tile) keeps memory bounded on mobile.
//!
//! The crate is framework-neutral above `freya-engine` and executor-
//! agnostic — OS threads + `flume` channels, no tokio.
//!
//! Modules are added incrementally per the crate roadmap (M3 onward)
//! in the workspace `PLAN.md`. M0 lands the crate skeleton only.

#![warn(missing_docs)]

pub mod backend;
pub mod cache;
pub mod cancel;
pub mod doc;
pub mod error;
pub mod render;
pub mod tiles;

pub use backend::PdfBackground;
pub use cache::{Cache, CachedTile};
pub use cancel::CancelToken;
pub use doc::PdfDocument;
pub use error::PdfError;
pub use render::RenderPool;
pub use tiles::{CacheKey, bucket_for, bucket_scale};
