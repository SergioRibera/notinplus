//! Faces: how a document kind draws its documents.
//!
//! A [`Face`] renders one document in a pane with Freya. It receives a
//! [`FaceContext`] with the document id, the host services and a typed
//! [`ContentStore`] to load and save the document's content.

use std::marker::PhantomData;
use std::sync::Arc;

use freya_core::prelude::Element;

use crate::doc::DocId;
use crate::kind::DocumentKind;
use crate::payload::Payload;
use crate::services::Host;

/// Identifies the pane a document is shown in, so a face keeps state
/// (scroll, zoom) per pane. Opaque to plugins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PaneKey(pub [u8; 32]);

/// Untyped storage the shell provides for one document.
pub trait RawStore: Send + Sync {
    /// Stored content bytes; `None` for a document never saved.
    fn load(&self) -> Option<Vec<u8>>;
    /// Replace the stored content.
    fn save(&self, bytes: Vec<u8>);
    /// Fires when content arrives from another device or person.
    fn changes(&self) -> flume::Receiver<()>;
    /// The document is shared read-only with this person.
    fn read_only(&self) -> bool;
}

/// Typed access to one document's content.
pub struct ContentStore<C> {
    raw: Arc<dyn RawStore>,
    content: PhantomData<fn() -> C>,
}

impl<C> Clone for ContentStore<C> {
    fn clone(&self) -> Self {
        Self {
            raw: Arc::clone(&self.raw),
            content: PhantomData,
        }
    }
}

impl<C> std::fmt::Debug for ContentStore<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContentStore").finish_non_exhaustive()
    }
}

impl<C: Payload + Default> ContentStore<C> {
    #[must_use]
    pub fn new(raw: Arc<dyn RawStore>) -> Self {
        Self {
            raw,
            content: PhantomData,
        }
    }

    /// Current content; a new or unreadable document reads as default.
    #[must_use]
    pub fn load(&self) -> C {
        self.raw
            .load()
            .and_then(|bytes| C::from_bytes(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, content: &C) {
        self.raw.save(content.to_bytes());
    }

    #[must_use]
    pub fn changes(&self) -> flume::Receiver<()> {
        self.raw.changes()
    }

    #[must_use]
    pub fn read_only(&self) -> bool {
        self.raw.read_only()
    }
}

/// Everything a face gets to draw one document.
pub struct FaceContext<K: DocumentKind> {
    pub doc: DocId,
    pub pane: Option<PaneKey>,
    pub host: Host,
    pub store: ContentStore<K::Content>,
    kind: Arc<K>,
}

impl<K: DocumentKind> Clone for FaceContext<K> {
    fn clone(&self) -> Self {
        Self {
            doc: self.doc,
            pane: self.pane,
            host: Arc::clone(&self.host),
            store: self.store.clone(),
            kind: Arc::clone(&self.kind),
        }
    }
}

impl<K: DocumentKind> std::fmt::Debug for FaceContext<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FaceContext")
            .field("doc", &self.doc)
            .finish_non_exhaustive()
    }
}

impl<K: DocumentKind> FaceContext<K> {
    /// The kind, for its index and helpers.
    #[must_use]
    pub fn kind(&self) -> &K {
        &self.kind
    }

    /// Decode a queued jump into this kind's anchor.
    #[must_use]
    pub fn anchor(bytes: &[u8]) -> Option<K::Anchor> {
        K::Anchor::from_bytes(bytes).ok()
    }
}

/// Renders documents of [`Face::Kind`].
pub trait Face: Send + Sync + 'static {
    type Kind: DocumentKind;

    fn render(&self, ctx: FaceContext<Self::Kind>) -> Element;
}

/// What the shell passes to an erased face.
#[derive(Clone)]
pub struct FaceRequest {
    pub doc: DocId,
    pub pane: Option<PaneKey>,
    pub host: Host,
    pub store: Arc<dyn RawStore>,
}

impl std::fmt::Debug for FaceRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FaceRequest")
            .field("doc", &self.doc)
            .finish_non_exhaustive()
    }
}

/// Object-safe face, as the shell holds it.
pub trait ErasedFace: Send + Sync {
    fn kind_id(&self) -> String;
    fn render(&self, request: FaceRequest) -> Element;
}

pub(crate) struct FaceAdapter<F: Face> {
    pub(crate) face: F,
    pub(crate) kind: Arc<F::Kind>,
}

impl<F: Face> ErasedFace for FaceAdapter<F> {
    fn kind_id(&self) -> String {
        self.kind.info().id.into_owned()
    }

    fn render(&self, request: FaceRequest) -> Element {
        self.face.render(FaceContext {
            doc: request.doc,
            pane: request.pane,
            host: request.host,
            store: ContentStore::new(request.store),
            kind: Arc::clone(&self.kind),
        })
    }
}
