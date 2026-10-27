//! Object-safe view of a [`DocumentKind`] for the shell's catalog.
//!
//! The adapter decodes anchors and content on the way in and encodes
//! them on the way out, so the shell only ever handles bytes and the
//! plugin only ever handles its own types.

use std::sync::Arc;

use crate::bookmark::SourceRef;
use crate::create::{
    Created, CreationEntry, CreationError, CreationRequest, ImportPreview, ImportedFile,
};
use crate::doc::{DocId, DocMeta};
use crate::kind::{
    ActionEffect, ChipView, DocumentKind, ExportFile, Hit, IndexReport, KindAction, KindChoice,
    KindInfo, Mark, OutlineNode, Quote, SearchQuery, SelectionSupport,
};
use crate::payload::Payload;

/// A document kind with its types erased to bytes.
pub trait ErasedKind: Send + Sync {
    fn info(&self) -> KindInfo;
    fn search(&self, query: &SearchQuery) -> Vec<Hit<Vec<u8>>>;
    fn index(&self, doc: DocMeta<'_>, content: &[u8], attachment: Option<&[u8]>) -> IndexReport;
    fn forget(&self, doc: DocId);
    fn index_status(&self) -> Vec<(DocId, IndexReport)>;
    fn index_size(&self, content: &[u8]) -> u32;
    fn outline(&self, content: &[u8]) -> Vec<OutlineNode<Vec<u8>>>;
    fn marks(&self, content: &[u8]) -> Vec<Mark<Vec<u8>>>;
    fn summary(&self, content: &[u8]) -> Option<String>;
    fn chip(&self, doc: DocMeta<'_>, anchor: &[u8]) -> ChipView;
    fn selection(&self) -> SelectionSupport;
    fn actions(&self) -> Vec<KindAction>;
    fn run_action(&self, id: &str, doc: DocMeta<'_>, content: &[u8]) -> ActionEffect;
    fn creation_entries(&self) -> Vec<CreationEntry>;
    fn create(
        &self,
        entry: &str,
        request: CreationRequest<'_>,
    ) -> Result<Created<Vec<u8>>, CreationError>;
    /// Bytes of an empty document.
    fn empty_content(&self) -> Vec<u8>;
    fn launch(&self, entry: &str);
    /// # Errors
    /// The file is not one this kind opens.
    fn inspect(&self, entry: &str, file: ImportedFile<'_>) -> Result<ImportPreview, CreationError>;
    /// New content bytes after the kept file was replaced.
    ///
    /// # Errors
    /// The kind keeps no file or this one cannot be read.
    fn replace_attachment(
        &self,
        file: ImportedFile<'_>,
        old: &[u8],
    ) -> Result<Vec<u8>, CreationError>;
    fn thumbnail(&self, content: &[u8], attachment: &[u8]) -> Option<Vec<u8>>;
    fn details(&self, content: &[u8]) -> Option<String>;
    fn choice(&self, content: &[u8]) -> Option<KindChoice>;
    /// New content bytes when option `id` changed something.
    fn choose(&self, content: &[u8], id: &str) -> Option<Vec<u8>>;
    fn excerpt(&self, content: &[u8]) -> Option<String>;
    fn references(&self, content: &[u8]) -> Option<Vec<SourceRef>>;
    /// Content bytes without highlights, annotations and bookmarks;
    /// `None` when `content` does not decode.
    fn without_marks(&self, content: &[u8]) -> Option<Vec<u8>>;
    fn export(
        &self,
        doc: DocMeta<'_>,
        content: &[u8],
        attachment: Option<&[u8]>,
    ) -> Option<ExportFile>;
    fn cited(&self, doc: DocId, anchor: &[u8]);
    fn quotes(&self) -> bool;
    fn quote(&self, doc: DocMeta<'_>, anchor: &[u8]) -> Option<Quote>;
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

    fn index_status(&self) -> Vec<(DocId, IndexReport)> {
        self.0.index_status()
    }

    fn index_size(&self, content: &[u8]) -> u32 {
        self.0.index_size(&Self::content(content))
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
        K::Anchor::from_bytes(anchor).map_or_else(
            |_| ChipView::broken(doc.name),
            |anchor| self.0.chip(doc, &anchor),
        )
    }

    fn selection(&self) -> SelectionSupport {
        self.0.selection()
    }

    fn actions(&self) -> Vec<KindAction> {
        self.0.actions()
    }

    fn run_action(&self, id: &str, doc: DocMeta<'_>, content: &[u8]) -> ActionEffect {
        self.0.run_action(id, doc, &Self::content(content))
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

    fn launch(&self, entry: &str) {
        self.0.launch(entry);
    }

    fn inspect(&self, entry: &str, file: ImportedFile<'_>) -> Result<ImportPreview, CreationError> {
        self.0.inspect(entry, file)
    }

    fn replace_attachment(
        &self,
        file: ImportedFile<'_>,
        old: &[u8],
    ) -> Result<Vec<u8>, CreationError> {
        self.0
            .replace_attachment(file, &Self::content(old))
            .map(|content| content.to_bytes())
    }

    fn thumbnail(&self, content: &[u8], attachment: &[u8]) -> Option<Vec<u8>> {
        self.0.thumbnail(&Self::content(content), attachment)
    }

    fn details(&self, content: &[u8]) -> Option<String> {
        self.0.details(&Self::content(content))
    }

    fn choice(&self, content: &[u8]) -> Option<KindChoice> {
        self.0.choice(&Self::content(content))
    }

    fn choose(&self, content: &[u8], id: &str) -> Option<Vec<u8>> {
        let mut content = Self::content(content);
        self.0.choose(&mut content, id).then(|| content.to_bytes())
    }

    fn excerpt(&self, content: &[u8]) -> Option<String> {
        self.0.excerpt(&Self::content(content))
    }

    fn references(&self, content: &[u8]) -> Option<Vec<SourceRef>> {
        self.0.references(&Self::content(content))
    }

    fn without_marks(&self, content: &[u8]) -> Option<Vec<u8>> {
        let mut content = K::Content::from_bytes(content).ok()?;
        self.0.without_marks(&mut content);
        Some(content.to_bytes())
    }

    fn export(
        &self,
        doc: DocMeta<'_>,
        content: &[u8],
        attachment: Option<&[u8]>,
    ) -> Option<ExportFile> {
        self.0.export(doc, &Self::content(content), attachment)
    }

    fn cited(&self, doc: DocId, anchor: &[u8]) {
        if let Ok(anchor) = K::Anchor::from_bytes(anchor) {
            self.0.cited(doc, &anchor);
        }
    }

    fn quotes(&self) -> bool {
        self.0.quotes()
    }

    fn quote(&self, doc: DocMeta<'_>, anchor: &[u8]) -> Option<Quote> {
        let anchor = K::Anchor::from_bytes(anchor).ok()?;
        self.0.quote(doc, &anchor)
    }
}
