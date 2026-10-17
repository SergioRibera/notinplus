//! Plugin-shipped icon bytes tagged by format.
//!
//! Chip pills in bookmark bodies, canvas-background outline rows, and
//! future plugin-surfaced UI (sidebar entries, context-menu items)
//! all consume the same handle. Keeping the type in `notein-plugin`
//! lets the Freya widget layer (`notein-ui-kit`, `freya-canvas-bg`)
//! and the plugin API refer to a single canonical shape.

use std::sync::Arc;

/// Icon bytes tagged by format. The renderer picks SVG rasterisation
/// vs. image decoding based on the variant. `Arc<[u8]>` is cheap to
/// clone and already thread-safe so backends can keep a single
/// instance per icon and hand clones to every consumer.
///
/// Not derived `rkyv::Archive` because `Arc<[u8]>` has no built-in
/// archive impl and the type is only passed through in-process
/// `Arc<dyn Source>` plugins today. Dynamic (dlopen) plugins that
/// need to ship icons bind the bytes directly via `IconType::bytes()`
/// at the FFI edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconType {
    /// Vector icon — raw SVG document bytes.
    Svg(Arc<[u8]>),
    /// Raster icon — PNG-encoded bytes. Consumers decode on first use.
    Png(Arc<[u8]>),
}

impl IconType {
    /// Build an SVG icon from any `Into<Arc<[u8]>>` source.
    pub fn svg(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self::Svg(bytes.into())
    }

    /// Build a PNG icon from any `Into<Arc<[u8]>>` source.
    pub fn png(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self::Png(bytes.into())
    }

    /// Borrow the underlying byte slice regardless of format tag.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        match self {
            Self::Svg(b) | Self::Png(b) => b,
        }
    }
}
