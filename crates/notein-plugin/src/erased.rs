//! Object-safe view of a [`DocumentKind`] for the shell's catalog.
//!
//! The adapter decodes anchors and content on the way in and encodes
//! them on the way out, so the shell only ever handles bytes and the
//! plugin only ever handles its own types.

use std::sync::Arc;

use crate::create::{Created, CreationEntry, CreationError, CreationRequest};
use crate::doc::{DocId, DocMeta};
use crate::kind::{
    ChipView, DocumentKind, Hit, IndexReport, KindInfo, Mark, OutlineNode, SearchQuery,
    SelectionSupport,
};
use crate::payload::Payload;

/// A document kind with its types erased to bytes.
pub trait ErasedKind: Send + Sync {
    fn info(&self) -> KindInfo;
    fn search(&self, query: &SearchQuery) -> Vec<Hit<Vec<u8>>>;
    fn index(&self, doc: DocMeta<'_>, content: &[u8], attachment: Option<&[u8]>) -> IndexReport;
    fn forget(&self, doc: DocId);
    fn outline(&self, content: &[u8]) -> Vec<OutlineNode<Vec<u8>>>;
    fn marks(&self, content: &[u8]) -> Vec<Mark<Vec<u8>>>;
    fn summary(&self, content: &[u8]) -> Option<String>;
    fn chip(&self, doc: DocMeta<'_>, anchor: &[u8]) -> ChipView;
    fn selection(&self) -> SelectionSupport;
    fn creation_entries(&self) -> Vec<CreationEntry>;
    fn create(
        &self,
        entry: &str,
        request: CreationRequest<'_>,
    ) -> Result<Created<Vec<u8>>, CreationError>;
    /// Bytes of an empty document.
    fn empty_content(&self) -> Vec<u8>;
}

pub(crate) struct KindAdapter<K: DocumentKind>(pub(crate) Arc<K>);

impl<K: DocumentKind> KindAdapter<K> {
    fn content(bytes: &[u8]) -> K::Content {
        if bytes.is_empty() {
            return K::Content::default();
        }
        K::Content::from_bytes(bytes).unwrap_or_else(|err| {
            log::warn!("content of kind does not decode, using empty: {err}");
            K::Content::default()
        })
    }
}

impl<K: DocumentKind> ErasedKind for KindAdapter<K> {
    fn info(&self) -> KindInfo {
        self.0.info()
    }

    fn search(&self, query: &SearchQuery) -> Vec<Hit<Vec<u8>>> {
        self.0
            .search(query)
            .take(query.limit)
            .map(|hit| Hit {
                doc: hit.doc,
                anchor: hit.anchor.to_bytes(),
                title: hit.title,
                snippet: hit.snippet,
                score: hit.score,
            })
            .collect()
    }

    fn index(&self, doc: DocMeta<'_>, content: &[u8], attachment: Option<&[u8]>) -> IndexReport {
        self.0.index(doc, &Self::content(content), attachment)
    }

    fn forget(&self, doc: DocId) {
        self.0.forget(doc);
    }

    fn outline(&self, content: &[u8]) -> Vec<OutlineNode<Vec<u8>>> {
        self.0
            .outline(&Self::content(content))
            .into_iter()
            .map(|node| node.map(&mut |anchor: K::Anchor| anchor.to_bytes()))
            .collect()
    }

    fn marks(&self, content: &[u8]) -> Vec<Mark<Vec<u8>>> {
        self.0
            .marks(&Self::content(content))
            .into_iter()
            .map(|mark| Mark {
                anchor: mark.anchor.to_bytes(),
                title: mark.title,
                snippet: mark.snippet,
                color: mark.color,
                updated_at: mark.updated_at,
            })
            .collect()
    }

    fn summary(&self, content: &[u8]) -> Option<String> {
        self.0.summary(&Self::content(content))
    }

    fn chip(&self, doc: DocMeta<'_>, anchor: &[u8]) -> ChipView {
        match K::Anchor::from_bytes(anchor) {
            Ok(anchor) => self.0.chip(doc, &anchor),
            Err(_) => ChipView::broken(doc.name),
        }
    }

    fn selection(&self) -> SelectionSupport {
        self.0.selection()
    }

    fn creation_entries(&self) -> Vec<CreationEntry> {
        self.0.creation_entries()
    }

    fn create(
        &self,
        entry: &str,
        request: CreationRequest<'_>,
    ) -> Result<Created<Vec<u8>>, CreationError> {
        self.0.create(entry, request).map(|created| Created {
            name: created.name,
            content: created.content.to_bytes(),
            attachment: created.attachment,
        })
    }

    fn empty_content(&self) -> Vec<u8> {
        K::Content::default().to_bytes()
    }
}
