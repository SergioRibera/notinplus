//! The PDF kind: imported PDFs to read and write on.

use std::collections::HashMap;
use std::sync::{PoisonError, RwLock, RwLockReadGuard};

use bincode::{Decode, Encode};
use freya::icons::lucide;
use freya_pdf::{PdfDocument, TocEntry};
use notein_plugin::create::{
    Created, CreationEntry, CreationError, CreationRequest, ImportPreview, ImportedFile,
};
use notein_plugin::doc::{DocId, DocMeta};
use notein_plugin::kind::{
    Attachment, ChipView, DocumentKind, ExportFile, Hit, IndexReport, KindColor, KindInfo, Mark,
    OutlineIcon, OutlineNode, SearchQuery, SelectionSupport,
};
use notein_plugin::text::Folded;

/// Kind id stored with every PDF.
pub const KIND: &str = "io.notein.pdf";
/// Create entry of a PDF import.
pub const ENTRY_IMPORT: &str = "import-pdf";

/// One row of the PDF's own table of contents.
#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct Contents {
    pub title: String,
    /// Zero-based page.
    pub page: Option<u32>,
    pub children: Vec<Self>,
}

impl From<TocEntry> for Contents {
    fn from(entry: TocEntry) -> Self {
        Self {
            title: entry.title,
            page: entry.page.and_then(|p| u32::try_from(p).ok()),
            children: entry.children.into_iter().map(Self::from).collect(),
        }
    }
}

/// A bookmark or highlight on a page.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct PdfMark {
    pub id: [u8; 16],
    pub anchor: PdfAnchor,
    pub body: String,
    pub color: Option<[u8; 3]>,
    pub updated_at: u64,
}

/// What the shell stores for a PDF besides the file itself.
#[derive(Clone, Debug, PartialEq, Default, Encode, Decode)]
pub struct PdfContent {
    pub file_name: String,
    pub size_bytes: u64,
    pub page_count: u32,
    pub contents: Vec<Contents>,
    pub marks: Vec<PdfMark>,
}

/// A place in a PDF: a page, optionally a fragment of its text.
#[derive(Clone, Debug, PartialEq, Eq, Default, Encode, Decode)]
pub struct PdfAnchor {
    /// Zero-based page.
    pub page: u32,
    /// Selected text, when the place is a fragment.
    pub text: Option<String>,
    /// Character range of the fragment inside the page text.
    pub range: Option<(u32, u32)>,
}

#[derive(Debug, Default)]
struct TextIndex {
    docs: Vec<(DocId, Vec<String>)>,
    progress: HashMap<DocId, IndexReport>,
}

/// The PDF kind. Keeps the text of every page for search.
#[derive(Debug, Default)]
pub struct PdfKind {
    index: RwLock<TextIndex>,
}

/// Pages whose text contains the query, read from the locked index.
pub struct PageHits<'a> {
    index: RwLockReadGuard<'a, TextIndex>,
    needle: String,
    doc: usize,
    page: usize,
}

impl std::fmt::Debug for PageHits<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageHits").field("needle", &self.needle).finish_non_exhaustive()
    }
}

/// Characters of context on each side of a snippet.
const SNIPPET_CONTEXT: usize = 48;

fn snippet(text: &str, needle: &str) -> Option<String> {
    Folded::new(text).excerpt(needle, SNIPPET_CONTEXT)
}

impl Iterator for PageHits<'_> {
    type Item = Hit<PdfAnchor>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.needle.is_empty() {
            return None;
        }
        while let Some((doc, pages)) = self.index.docs.get(self.doc) {
            while let Some(text) = pages.get(self.page) {
                let page = self.page;
                self.page += 1;
                if let Some(snippet) = snippet(text, &self.needle) {
                    return Some(Hit {
                        doc: Some(*doc),
                        anchor: PdfAnchor {
                            page: u32::try_from(page).unwrap_or(u32::MAX),
                            text: None,
                            range: None,
                        },
                        title: format!("p. {}", page + 1),
                        snippet,
                        score: 1.0,
                    });
                }
            }
            self.doc += 1;
            self.page = 0;
        }
        None
    }
}

impl PdfKind {
    /// Extract every page's text from `pdf`, reporting progress through
    /// `on_page(done, total)` as it goes.
    pub fn extract_text(
        &self,
        doc: DocId,
        pdf: &PdfDocument,
        mut on_page: impl FnMut(u32, u32),
    ) -> Vec<String> {
        let total = u32::try_from(pdf.page_count()).unwrap_or(u32::MAX);
        let mut pages = Vec::with_capacity(pdf.page_count());
        for page in 0..pdf.page_count() {
            pages.push(pdf.extract_page_text(page).unwrap_or_default());
            let done = u32::try_from(page + 1).unwrap_or(total);
            self.index
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .progress
                .insert(doc, IndexReport { done, total });
            on_page(done, total);
        }
        pages
    }

    /// Store the page texts of `doc` (from extraction or a cache).
    pub fn store_text(&self, doc: DocId, pages: Vec<String>) {
        let mut index = self.index.write().unwrap_or_else(PoisonError::into_inner);
        index.progress.remove(&doc);
        index.docs.retain(|(id, _)| *id != doc);
        index.docs.push((doc, pages));
    }

    /// Whether `doc`'s text is already indexed.
    #[must_use]
    pub fn has_text(&self, doc: DocId) -> bool {
        self.index
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .docs
            .iter()
            .any(|(id, _)| *id == doc)
    }

    /// Content of a freshly imported file.
    ///
    /// # Errors
    /// [`CreationError::Unreadable`] when the bytes are not a PDF the
    /// renderer can open.
    pub fn read_file(name: &str, bytes: &[u8]) -> Result<(PdfDocument, PdfContent), CreationError> {
        let pdf = PdfDocument::open_bytes(bytes.to_vec()).map_err(|_| {
            CreationError::Unreadable(
                "Puede estar dañado o protegido con contraseña. Probá abrirlo en otro programa y guardarlo de nuevo."
                    .to_owned(),
            )
        })?;
        let content = PdfContent {
            file_name: name.to_owned(),
            size_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            page_count: u32::try_from(pdf.page_count()).unwrap_or(u32::MAX),
            contents: pdf
                .table_of_contents()
                .into_iter()
                .map(Contents::from)
                .collect(),
            marks: Vec::new(),
        };
        Ok((pdf, content))
    }
}

/// Document name for an imported file: its name without `.pdf`.
#[must_use]
pub fn name_from_file(file_name: &str) -> String {
    let trimmed = file_name.trim();
    let stem = trimmed
        .len()
        .checked_sub(4)
        .filter(|cut| {
            trimmed.is_char_boundary(*cut) && trimmed[*cut..].eq_ignore_ascii_case(".pdf")
        })
        .map_or(trimmed, |cut| &trimmed[..cut]);
    if stem.is_empty() {
        "Documento PDF".to_owned()
    } else {
        stem.to_owned()
    }
}

fn outline_of(contents: &[Contents]) -> Vec<OutlineNode<PdfAnchor>> {
    contents
        .iter()
        .map(|entry| {
            let anchor = entry.page.map(|page| PdfAnchor {
                page,
                text: None,
                range: None,
            });
            let node = OutlineNode::new(entry.title.clone(), anchor)
                .icon(if entry.page.is_some() {
                    OutlineIcon::Page
                } else {
                    OutlineIcon::Section
                })
                .children(outline_of(&entry.children));
            match entry.page {
                Some(page) => node.meta(format!("p. {}", page + 1)),
                None => node,
            }
        })
        .collect()
}

impl DocumentKind for PdfKind {
    type Anchor = PdfAnchor;
    type Content = PdfContent;
    type Hits<'a> = PageHits<'a>;

    fn info(&self) -> KindInfo {
        KindInfo {
            id: KIND.into(),
            label: "PDF".into(),
            plural: "PDF".into(),
            search_group: "En PDFs".into(),
            icon: lucide::file_text(),
            color: KindColor::Pdf,
            supports_ink: true,
            has_paper: false,
            attachment: Some(Attachment {
                extensions: vec!["pdf".into()],
            }),
            singleton: false,
            tint: true,
            notebook: false,
            body: false,
        }
    }

    fn search<'a>(&'a self, query: &'a SearchQuery) -> PageHits<'a> {
        PageHits {
            index: self.index.read().unwrap_or_else(PoisonError::into_inner),
            needle: query.folded(),
            doc: 0,
            page: 0,
        }
    }

    fn index(
        &self,
        doc: DocMeta<'_>,
        content: &PdfContent,
        attachment: Option<&[u8]>,
    ) -> IndexReport {
        if self.has_text(doc.id) {
            return IndexReport::complete(content.page_count);
        }
        let Some(bytes) = attachment else {
            return IndexReport::default();
        };
        let Ok(pdf) = PdfDocument::open_bytes(bytes.to_vec()) else {
            return IndexReport::default();
        };
        let pages = self.extract_text(doc.id, &pdf, |_, _| {});
        let total = u32::try_from(pages.len()).unwrap_or(u32::MAX);
        self.store_text(doc.id, pages);
        IndexReport::complete(total)
    }

    fn index_status(&self) -> Vec<(DocId, IndexReport)> {
        self.index
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .progress
            .iter()
            .map(|(doc, report)| (*doc, report.clone()))
            .collect()
    }

    fn forget(&self, doc: DocId) {
        let mut index = self.index.write().unwrap_or_else(PoisonError::into_inner);
        index.docs.retain(|(id, _)| *id != doc);
        index.progress.remove(&doc);
    }

    fn outline(&self, content: &PdfContent) -> Vec<OutlineNode<PdfAnchor>> {
        outline_of(&content.contents)
    }

    fn marks(&self, content: &PdfContent) -> Vec<Mark<PdfAnchor>> {
        content
            .marks
            .iter()
            .map(|mark| Mark {
                anchor: mark.anchor.clone(),
                title: mark
                    .body
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .map_or_else(|| format!("p. {}", mark.anchor.page + 1), str::to_owned),
                snippet: mark.anchor.text.clone().unwrap_or_default(),
                color: mark.color,
                updated_at: mark.updated_at,
            })
            .collect()
    }

    fn summary(&self, content: &PdfContent) -> Option<String> {
        match content.page_count {
            0 => None,
            1 => Some("1 página".to_owned()),
            n => Some(format!("{n} páginas")),
        }
    }

    fn chip(&self, doc: DocMeta<'_>, anchor: &PdfAnchor) -> ChipView {
        ChipView::new(match &anchor.text {
            Some(_) => format!("PDF p.{} · fragmento", anchor.page + 1),
            None => format!("{} p.{}", doc.name, anchor.page + 1),
        })
    }

    fn selection(&self) -> SelectionSupport {
        SelectionSupport::TEXT
    }

    fn creation_entries(&self) -> Vec<CreationEntry> {
        vec![
            CreationEntry::new(ENTRY_IMPORT, "PDF", lucide::file_text(), KindColor::Pdf)
                .subtitle("Importar y escribir encima")
                .import("Elegí un PDF", &["pdf"], &["application/pdf"])
                .starter(2),
        ]
    }

    fn create(
        &self,
        entry: &str,
        request: CreationRequest<'_>,
    ) -> Result<Created<PdfContent>, CreationError> {
        if entry != ENTRY_IMPORT {
            return Err(CreationError::UnknownEntry(entry.to_owned()));
        }
        let file = request.file.ok_or(CreationError::MissingFile)?;
        let (_, content) = Self::read_file(file.name, file.bytes)?;
        let name = if request.name.trim().is_empty() {
            name_from_file(file.name)
        } else {
            request.name.trim().to_owned()
        };
        Ok(Created {
            name,
            content,
            attachment: Some(file.bytes.to_vec()),
        })
    }

    fn inspect(
        &self,
        _entry: &str,
        file: ImportedFile<'_>,
    ) -> Result<ImportPreview, CreationError> {
        let (_, pdf) = Self::read_file(file.name, file.bytes)?;
        Ok(ImportPreview {
            name: name_from_file(file.name),
            details: Some(format!(
                "{} · {} · {}",
                crate::format::count(pdf.page_count as usize, "página", "páginas"),
                crate::format::size(pdf.size_bytes),
                file.name
            )),
            preview: None,
        })
    }

    fn replace_attachment(
        &self,
        file: ImportedFile<'_>,
        old: &PdfContent,
    ) -> Result<PdfContent, CreationError> {
        let (_, mut fresh) = Self::read_file(file.name, file.bytes)?;
        fresh.marks.clone_from(&old.marks);
        Ok(fresh)
    }

    fn thumbnail(&self, _content: &PdfContent, attachment: &[u8]) -> Option<Vec<u8>> {
        let pdf = PdfDocument::open_bytes(attachment.to_vec()).ok()?;
        crate::thumbnail::pdf(&pdf)
    }

    fn details(&self, content: &PdfContent) -> Option<String> {
        Some(format!(
            "{} · {} · {}",
            content.file_name,
            crate::format::count(content.page_count as usize, "página", "páginas"),
            crate::format::size(content.size_bytes)
        ))
    }

    fn without_marks(&self, content: &mut PdfContent) {
        content.marks.clear();
    }

    fn export(
        &self,
        doc: DocMeta<'_>,
        _content: &PdfContent,
        attachment: Option<&[u8]>,
    ) -> Option<ExportFile> {
        let item = crate::library::ItemId(doc.id.0);
        crate::readable::flattened_pdf(item, attachment?).map(|bytes| ExportFile {
            extension: "pdf".into(),
            bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_drop_the_extension() {
        assert_eq!(
            name_from_file("Comentario_Romanos.pdf"),
            "Comentario_Romanos"
        );
        assert_eq!(name_from_file("ATLAS.PDF"), "ATLAS");
        assert_eq!(name_from_file(".pdf"), "Documento PDF");
        assert_eq!(name_from_file("notas"), "notas");
    }

    #[test]
    fn snippets_keep_context_around_the_match() {
        let text = "La adopción no es una metáfora legal fría sino un acto de amor.";
        let s = snippet(text, "metáfora").expect("found");
        assert!(s.contains("metáfora"));
        assert!(snippet(text, "tribunal").is_none());
    }

    #[test]
    fn search_reads_stored_pages() {
        let kind = PdfKind::default();
        kind.store_text(
            DocId([3; 16]),
            vec!["uno".into(), "el tribunal cierra".into()],
        );
        let query = SearchQuery::new("Tribunal", 5);
        let hits: Vec<_> = kind.search(&query).collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].anchor.page, 1);
        assert_eq!(hits[0].title, "p. 2");
    }
}
