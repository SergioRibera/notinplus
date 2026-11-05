//! The PDF kind: imported PDFs to read and write on.

use std::collections::HashMap;
use std::sync::{PoisonError, RwLock, RwLockReadGuard};

use bincode::{Decode, Encode};
use freya::icons::lucide;
use freya_pdf::{PdfDocument, TocEntry};
use notein_i18n::tr;
use notein_plugin::config::{ConfigEnumOption, ConfigField, ConfigValue};
use notein_plugin::create::{
    Created, CreationEntry, CreationError, CreationRequest, ImportPreview, ImportedFile,
};
use notein_plugin::doc::{DocId, DocMeta};
use notein_plugin::kind::{
    Attachment, ChipView, DocumentKind, ExportError, ExportFile, ExportValues, Hit, IndexReport,
    KindColor, KindInfo, Mark, OutlineIcon, OutlineNode, Quote, SearchQuery, SelectionSupport,
    Snapshot,
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
    /// Where the entry starts on its page, in points from the top; `None`
    /// when the file pins the whole page.
    pub top: Option<u32>,
    pub children: Vec<Self>,
}

impl From<TocEntry> for Contents {
    fn from(entry: TocEntry) -> Self {
        Self {
            title: entry.title,
            page: entry.page.and_then(|p| u32::try_from(p).ok()),
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // points
            top: entry.y_from_top.map(|y| y.round().max(0.0) as u32),
            children: entry.children.into_iter().map(Self::from).collect(),
        }
    }
}

/// Most pages a quote or a snapshot of a range covers; the chip still
/// links to the whole range.
pub const MAX_QUOTED_PAGES: u32 = 10;
/// Pixels per point of a snapshot (144 dpi).
const SNAPSHOT_SCALE: f32 = 2.0;
/// A cropped page keeps at least this many pixel rows.
const MIN_CROP_ROWS: u32 = 8;

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

/// A place in a PDF: a page, a range of pages (a section of the table of
/// contents), optionally a fragment of a page's text.
#[derive(Clone, Debug, PartialEq, Eq, Default, Encode, Decode)]
pub struct PdfAnchor {
    /// Zero-based first page.
    pub page: u32,
    /// Zero-based last page of a range, inclusive; `None` for one page.
    pub last: Option<u32>,
    /// Where a section starts on `page`, in points from the top; `None`
    /// from the top edge.
    pub top: Option<u32>,
    /// Where a section ends on its last page, in points from the top;
    /// `None` at the bottom edge.
    pub bottom: Option<u32>,
    /// Title of the section the range covers, from the table of contents.
    pub section: Option<String>,
    /// Selected text, when the place is a fragment.
    pub text: Option<String>,
    /// Character range of the fragment inside the page text.
    pub range: Option<(u32, u32)>,
}

impl PdfAnchor {
    /// One whole page.
    #[must_use]
    pub fn page(page: u32) -> Self {
        Self {
            page,
            ..Self::default()
        }
    }

    /// The whole pages `first..=last`, in either order.
    #[must_use]
    pub fn pages(first: u32, last: u32) -> Self {
        Self {
            page: first.min(last),
            last: (first != last).then_some(first.max(last)),
            ..Self::default()
        }
    }

    /// Zero-based last page covered.
    #[must_use]
    pub fn last_page(&self) -> u32 {
        self.last.unwrap_or(self.page)
    }

    /// Zero-based pages covered, at most [`MAX_QUOTED_PAGES`] of them.
    fn quoted_pages(&self) -> std::ops::RangeInclusive<u32> {
        let last = self
            .last_page()
            .min(self.page.saturating_add(MAX_QUOTED_PAGES - 1));
        self.page..=last
    }

    /// "Name p. 3", "Name p. 3–5" or "Section · Name p. 3–5".
    #[sismo::function("pdf_kind::anchor_label")]
    fn label(&self, name: &str) -> String {
        let pages = self.pages_label(name);
        match &self.section {
            Some(section) => {
                tr!("plugins-pdf-chip-section", "section" => section.as_str(), "pages" => pages)
            }
            None => pages,
        }
    }

    /// "Name p. 3" or "Name p. 3–5".
    fn pages_label(&self, name: &str) -> String {
        self.last.map_or_else(
            || tr!("plugins-pdf-chip-page", "name" => name, "page" => self.page + 1),
            |last| {
                tr!(
                    "plugins-pdf-chip-pages",
                    "name" => name,
                    "first" => self.page + 1,
                    "last" => last + 1
                )
            },
        )
    }
}

/// What the index keeps of one document.
#[derive(Debug)]
struct IndexedDoc {
    id: DocId,
    name: String,
    /// Text of every page.
    pages: Vec<String>,
    contents: Vec<Contents>,
}

#[derive(Debug, Default)]
struct TextIndex {
    docs: Vec<IndexedDoc>,
    progress: HashMap<DocId, IndexReport>,
}

impl TextIndex {
    fn doc(&self, id: DocId) -> Option<&IndexedDoc> {
        self.docs.iter().find(|doc| doc.id == id)
    }
}

/// A page or range of pages asked for by number: "23", "p. 23", "23-25".
#[sismo::function("pdf_kind::parse_page_query")]
fn parse_page_query(text: &str) -> Option<(u32, u32)> {
    const PREFIXES: [&str; 9] = [
        "página", "pagina", "page", "pág.", "pág", "pag.", "pag", "p.", "p",
    ];
    let lower = text.trim().to_lowercase();
    let rest = PREFIXES
        .iter()
        .find_map(|prefix| lower.strip_prefix(prefix))
        .unwrap_or(&lower)
        .trim();
    let (from, to) = match rest.split_once(['-', '–']) {
        Some((a, b)) => (a.trim(), b.trim()),
        None => (rest, rest),
    };
    if from.is_empty() || !from.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let from: u32 = from.parse().ok().filter(|page| *page > 0)?;
    let to: u32 = if to.is_empty() {
        from
    } else {
        to.parse().ok().filter(|page| *page > 0)?
    };
    Some((from, to))
}

/// Where a section of the table of contents starts or the next one
/// begins: a page and an offset from its top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Boundary {
    page: u32,
    top: Option<u32>,
}

/// Every entry of `contents` with a page, as the range of pages it
/// covers: from its own start to where the next entry that is not inside
/// it begins, or to the end of the document (`page_count` pages).
#[sismo::function("pdf_kind::sections")]
fn sections(contents: &[Contents], page_count: u32) -> Vec<PdfAnchor> {
    fn walk(
        entries: &[Contents],
        after: Option<Boundary>,
        page_count: u32,
        out: &mut Vec<PdfAnchor>,
    ) {
        for (index, entry) in entries.iter().enumerate() {
            let next = entries[index + 1..]
                .iter()
                .find_map(|sibling| {
                    sibling.page.map(|page| Boundary {
                        page,
                        top: sibling.top,
                    })
                })
                .or(after);
            if let Some(page) = entry.page {
                out.push(section_anchor(entry, page, next, page_count));
            }
            walk(&entry.children, next, page_count, out);
        }
    }
    let mut out = Vec::new();
    walk(contents, None, page_count, &mut out);
    out
}

/// The range `entry` (starting on `page`) covers up to `next`.
fn section_anchor(
    entry: &Contents,
    page: u32,
    next: Option<Boundary>,
    page_count: u32,
) -> PdfAnchor {
    let end_of_doc = page_count.saturating_sub(1).max(page);
    let (last, bottom) = match next {
        Some(next) if next.page > page => match next.top {
            // The next section starts down its page: this one ends there.
            Some(top) if top > 0 => (next.page, Some(top)),
            _ => (next.page - 1, None),
        },
        Some(next) if next.page == page => (page, next.top.filter(|top| *top > 0)),
        // A table of contents out of order: just the page.
        Some(_) => (page, None),
        None => (end_of_doc, None),
    };
    PdfAnchor {
        page,
        last: (last != page).then_some(last),
        top: entry.top.filter(|top| *top > 0),
        bottom,
        section: Some(entry.title.clone()),
        text: None,
        range: None,
    }
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
    /// Pages and sections asked for by number or title, offered first.
    front: std::vec::IntoIter<Hit<PdfAnchor>>,
    doc: usize,
    page: usize,
}

impl std::fmt::Debug for PageHits<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageHits")
            .field("needle", &self.needle)
            .finish_non_exhaustive()
    }
}

/// Characters of context on each side of a snippet.
const SNIPPET_CONTEXT: usize = 48;

fn snippet(text: &str, needle: &str) -> Option<String> {
    Folded::new(text).excerpt(needle, SNIPPET_CONTEXT)
}

impl Iterator for PageHits<'_> {
    type Item = Hit<PdfAnchor>;

    #[sismo::function("pdf_kind::search_next")]
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(hit) = self.front.next() {
            return Some(hit);
        }
        if self.needle.is_empty() {
            return None;
        }
        while let Some(doc) = self.index.docs.get(self.doc) {
            while let Some(text) = doc.pages.get(self.page) {
                let page = self.page;
                self.page += 1;
                if let Some(snippet) = snippet(text, &self.needle) {
                    return Some(Hit {
                        doc: Some(doc.id),
                        anchor: PdfAnchor::page(u32::try_from(page).unwrap_or(u32::MAX)),
                        title: tr!("plugins-pdf-page-short", "page" => page + 1),
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
    #[sismo::function("pdf_kind::extract_text")]
    pub fn extract_text(
        &self,
        doc: DocId,
        pdf: &PdfDocument,
        mut on_page: impl FnMut(u32, u32),
    ) -> Vec<String> {
        let total = u32::try_from(pdf.page_count()).unwrap_or(u32::MAX);
        let mut pages = Vec::with_capacity(pdf.page_count());
        for page in 0..pdf.page_count() {
            sismo::scope!("pdf_kind::extract_page");
            pages.push(pdf.extract_page_text(page).unwrap_or_default());
            let done = u32::try_from(page + 1).unwrap_or(total);
            self.index
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .progress
                .insert(doc, IndexReport { done, total });
            on_page(done, total);
        }
        sismo::counter!("pdf_kind.pages_extracted", pages.len());
        pages
    }

    /// Store the page texts of `doc` (from extraction or a cache) with
    /// what search needs to name its pages and sections.
    #[sismo::function("pdf_kind::store_text")]
    pub fn store_text(&self, doc: DocId, name: &str, pages: Vec<String>, contents: &[Contents]) {
        let mut index = self.index.write().unwrap_or_else(PoisonError::into_inner);
        index.progress.remove(&doc);
        index.docs.retain(|indexed| indexed.id != doc);
        index.docs.push(IndexedDoc {
            id: doc,
            name: name.to_owned(),
            pages,
            contents: contents.to_vec(),
        });
    }

    /// Whether `doc`'s text is already indexed.
    #[must_use]
    pub fn has_text(&self, doc: DocId) -> bool {
        self.index
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .doc(doc)
            .is_some()
    }

    /// Pages and sections of every indexed document that `query` names:
    /// "p. 23" or "23-25" for pages, words of a section's title.
    #[sismo::function("pdf_kind::named_hits")]
    fn named_hits(index: &TextIndex, query: &SearchQuery) -> Vec<Hit<PdfAnchor>> {
        let needle = query.folded();
        if needle.is_empty() {
            return Vec::new();
        }
        let pages = parse_page_query(&query.text);
        let mut out = Vec::new();
        for doc in &index.docs {
            let count = u32::try_from(doc.pages.len()).unwrap_or(u32::MAX);
            if let Some((first, last)) = pages
                && first <= count
            {
                let anchor = PdfAnchor::pages(first - 1, last.min(count) - 1);
                let first_line = doc
                    .pages
                    .get(anchor.page as usize)
                    .and_then(|text| text.lines().map(str::trim).find(|line| !line.is_empty()))
                    .unwrap_or_default();
                out.push(Hit {
                    doc: Some(doc.id),
                    title: anchor.label(&doc.name),
                    snippet: first_line.chars().take(SNIPPET_CONTEXT * 2).collect(),
                    anchor,
                    score: 2.0,
                });
            }
            for section in sections(&doc.contents, count) {
                let title = section.section.as_deref().unwrap_or_default();
                if notein_plugin::fold(title).contains(&needle) {
                    out.push(Hit {
                        doc: Some(doc.id),
                        title: title.to_owned(),
                        snippet: section.pages_label(&doc.name),
                        anchor: section,
                        score: 1.5,
                    });
                }
            }
        }
        sismo::counter!("pdf_kind.named_hits", out.len());
        out
    }

    /// Text of the pages `anchor` covers, as markdown paragraphs; `None`
    /// before the document is indexed.
    #[sismo::function("pdf_kind::pages_text")]
    fn pages_text(index: &TextIndex, doc: DocId, anchor: &PdfAnchor) -> Option<String> {
        let indexed = index.doc(doc)?;
        let pages: Vec<&str> = anchor
            .quoted_pages()
            .filter_map(|page| indexed.pages.get(page as usize))
            .map(|text| text.trim())
            .filter(|text| !text.is_empty())
            .collect();
        if pages.is_empty() {
            return None;
        }
        sismo::counter!("pdf_kind.pages_quoted", pages.len());
        Some(
            pages
                .iter()
                .map(|text| text.replace("\r\n", "\n"))
                .collect::<Vec<_>>()
                .join("\n\n"),
        )
    }

    /// Content of a freshly imported file.
    ///
    /// # Errors
    /// [`CreationError::Unreadable`] when the bytes are not a PDF the
    /// renderer can open.
    #[sismo::function("pdf_kind::read_file")]
    pub fn read_file(name: &str, bytes: &[u8]) -> Result<(PdfDocument, PdfContent), CreationError> {
        let pdf = PdfDocument::open_bytes(bytes.to_vec())
            .map_err(|_| CreationError::Unreadable(tr!("plugins-pdf-unreadable")))?;
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

/// Keep of `page` the rows between `top` and `bottom` points from the
/// page's top edge (`None` for its edges). Too thin a slice leaves the
/// page whole.
#[sismo::function("pdf_kind::crop_rows")]
fn crop_rows(page: &mut freya_pdf::write::PageImage, top: Option<u32>, bottom: Option<u32>) {
    if top.is_none() && bottom.is_none() || page.height_pt == 0 {
        return;
    }
    #[allow(clippy::cast_precision_loss)] // pixel and point sizes
    let per_point = page.height_px as f32 / page.height_pt as f32;
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let row = |points: u32| ((points as f32 * per_point).round() as u32).min(page.height_px);
    let first = top.map_or(0, row);
    let end = bottom.map_or(page.height_px, row);
    if end < first.saturating_add(MIN_CROP_ROWS) {
        return;
    }
    let stride = page.width_px as usize * 4;
    page.rgba = page.rgba[first as usize * stride..end as usize * stride].to_vec();
    page.height_px = end - first;
    page.height_pt = bottom
        .unwrap_or(page.height_pt)
        .saturating_sub(top.unwrap_or(0));
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
        tr!("plugins-pdf-default-name")
    } else {
        stem.to_owned()
    }
}

#[sismo::function("pdf_kind::outline")]
fn outline_of(contents: &[Contents]) -> Vec<OutlineNode<PdfAnchor>> {
    contents
        .iter()
        .map(|entry| {
            let anchor = entry.page.map(PdfAnchor::page);
            let node = OutlineNode::new(entry.title.clone(), anchor)
                .icon(if entry.page.is_some() {
                    OutlineIcon::Page
                } else {
                    OutlineIcon::Section
                })
                .children(outline_of(&entry.children));
            match entry.page {
                Some(page) => node.meta(tr!("plugins-pdf-page-short", "page" => page + 1)),
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
            search_group: tr!("plugins-pdf-search-group").into(),
            icon: lucide::file_text(),
            color: KindColor::Pdf,
            supports_ink: true,
            has_paper: false,
            attachment: Some(Attachment {
                extensions: vec!["pdf".into()],
            }),
            shared_refs: false,
            tint: true,
            notebook: false,
            body: false,
            records: false,
        }
    }

    #[sismo::function("pdf_kind::search")]
    fn search<'a>(&'a self, query: &'a SearchQuery) -> PageHits<'a> {
        let index = self.index.read().unwrap_or_else(PoisonError::into_inner);
        let front = Self::named_hits(&index, query).into_iter();
        PageHits {
            index,
            needle: query.folded(),
            front,
            doc: 0,
            page: 0,
        }
    }

    #[sismo::function("pdf_kind::index")]
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
        self.store_text(doc.id, doc.name, pages, &content.contents);
        IndexReport::complete(total)
    }

    fn index_size(&self, content: &PdfContent) -> u32 {
        content.page_count
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

    #[sismo::function("pdf_kind::forget")]
    fn forget(&self, doc: DocId) {
        let mut index = self.index.write().unwrap_or_else(PoisonError::into_inner);
        index.docs.retain(|indexed| indexed.id != doc);
        index.progress.remove(&doc);
    }

    fn outline(&self, content: &PdfContent) -> Vec<OutlineNode<PdfAnchor>> {
        outline_of(&content.contents)
    }

    #[sismo::function("pdf_kind::marks")]
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
                    .map_or_else(
                        || tr!("plugins-pdf-page-short", "page" => mark.anchor.page + 1),
                        str::to_owned,
                    ),
                snippet: mark.anchor.text.clone().unwrap_or_default(),
                color: mark.color,
                updated_at: mark.updated_at,
            })
            .collect()
    }

    fn summary(&self, content: &PdfContent) -> Option<String> {
        match content.page_count {
            0 => None,
            n => Some(tr!("plugins-pdf-pages", "count" => n)),
        }
    }

    fn chip(&self, doc: DocMeta<'_>, anchor: &PdfAnchor) -> ChipView {
        ChipView::new(if anchor.text.is_some() {
            tr!("plugins-pdf-chip-excerpt", "page" => anchor.page + 1)
        } else {
            anchor.label(doc.name)
        })
    }

    fn selection(&self) -> SelectionSupport {
        SelectionSupport::TEXT
    }

    fn quotes(&self) -> bool {
        true
    }

    /// A file never changes: its quotes are copies.
    fn quotes_live(&self) -> bool {
        false
    }

    #[sismo::function("pdf_kind::quote")]
    fn quote(&self, doc: DocMeta<'_>, anchor: &PdfAnchor) -> Option<Quote> {
        let source = anchor.label(doc.name);
        if let Some(text) = anchor
            .text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            return Some(Quote {
                body: text.to_owned(),
                source,
            });
        }
        let index = self.index.read().unwrap_or_else(PoisonError::into_inner);
        Self::pages_text(&index, doc.id, anchor).map(|body| Quote { body, source })
    }

    fn snapshots(&self) -> bool {
        true
    }

    #[sismo::function("pdf_kind::snapshot")]
    fn snapshot(
        &self,
        doc: DocMeta<'_>,
        _content: &PdfContent,
        attachment: Option<&[u8]>,
        anchor: &PdfAnchor,
    ) -> Vec<Snapshot> {
        use crate::readable::{ImageFormat, encode_page, pdf_pages};
        let Some(bytes) = attachment else {
            return Vec::new();
        };
        let item = crate::library::ItemId(doc.id.0);
        let pages: Vec<usize> = anchor.quoted_pages().map(|page| page as usize).collect();
        let Some(mut rendered) = pdf_pages(item, bytes, &pages, false, SNAPSHOT_SCALE) else {
            return Vec::new();
        };
        let last = rendered.len().saturating_sub(1);
        if let Some(first) = rendered.first_mut() {
            crop_rows(
                first,
                anchor.top,
                (last == 0).then_some(anchor.bottom).flatten(),
            );
        }
        if last > 0
            && let Some(page) = rendered.get_mut(last)
        {
            crop_rows(page, None, anchor.bottom);
        }
        sismo::counter!("pdf_kind.snapshot_pages", rendered.len());
        let source = anchor.label(doc.name);
        rendered
            .iter()
            .filter_map(|page| {
                Some(Snapshot {
                    png: encode_page(page, ImageFormat::Png)?,
                    width: page.width_px,
                    height: page.height_px,
                    source: source.clone(),
                })
            })
            .collect()
    }

    fn creation_entries(&self) -> Vec<CreationEntry> {
        vec![
            CreationEntry::new(ENTRY_IMPORT, "PDF", lucide::file_text(), KindColor::Pdf)
                .subtitle(tr!("plugins-pdf-entry-subtitle"))
                .import(
                    tr!("plugins-pdf-pick-title"),
                    &["pdf"],
                    &["application/pdf"],
                )
                .starter(2),
        ]
    }

    #[sismo::function("pdf_kind::create")]
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

    #[sismo::function("pdf_kind::inspect")]
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
                tr!("plugins-pdf-pages", "count" => pdf.page_count),
                crate::format::size(pdf.size_bytes),
                file.name
            )),
            preview: None,
        })
    }

    #[sismo::function("pdf_kind::replace_attachment")]
    fn replace_attachment(
        &self,
        file: ImportedFile<'_>,
        old: &PdfContent,
    ) -> Result<PdfContent, CreationError> {
        let (_, mut fresh) = Self::read_file(file.name, file.bytes)?;
        fresh.marks.clone_from(&old.marks);
        Ok(fresh)
    }

    #[sismo::function("pdf_kind::thumbnail")]
    fn thumbnail(&self, _content: &PdfContent, attachment: &[u8]) -> Option<Vec<u8>> {
        let pdf = PdfDocument::open_bytes(attachment.to_vec()).ok()?;
        crate::thumbnail::pdf(&pdf)
    }

    fn details(&self, content: &PdfContent) -> Option<String> {
        Some(format!(
            "{} · {} · {}",
            content.file_name,
            tr!("plugins-pdf-pages", "count" => content.page_count),
            crate::format::size(content.size_bytes)
        ))
    }

    fn without_marks(&self, content: &mut PdfContent) {
        content.marks.clear();
    }

    fn exports(&self) -> bool {
        true
    }

    #[sismo::function("pdf_kind::export_fields")]
    fn export_fields(
        &self,
        _doc: DocMeta<'_>,
        _content: &PdfContent,
        attachment: Option<&[u8]>,
    ) -> Vec<ConfigField> {
        let count = attachment
            .and_then(|bytes| PdfDocument::open_bytes(bytes.to_vec()).ok())
            .map_or(0, |pdf| pdf.page_count());
        let images = || {
            [
                ConfigValue::String(export::PNG.into()),
                ConfigValue::String(export::JPG.into()),
            ]
        };
        vec![
            ConfigField::choice(
                export::FORMAT,
                &tr!("export-format"),
                [
                    ConfigEnumOption::new(export::PDF, tr!("export-format-pdf")),
                    ConfigEnumOption::new(export::PNG, tr!("export-format-png")),
                    ConfigEnumOption::new(export::JPG, tr!("export-format-jpg")),
                ],
                export::PDF,
            ),
            ConfigField::choice(
                export::CONTENT,
                &tr!("export-content"),
                [
                    ConfigEnumOption::new(export::ANNOTATED, tr!("export-content-annotated")),
                    ConfigEnumOption::new(export::ORIGINAL, tr!("export-content-original")),
                ],
                export::ANNOTATED,
            ),
            ConfigField::string(export::PAGES, &tr!("export-pages"), 200, "")
                .help(&tr!("export-pages-help", "count" => count))
                .only_when(export::FORMAT, images()),
            ConfigField::choice(
                export::DPI,
                &tr!("export-dpi"),
                [
                    ConfigEnumOption::new("150", tr!("export-dpi-150")),
                    ConfigEnumOption::new("300", tr!("export-dpi-300")),
                ],
                "150",
            )
            .only_when(export::FORMAT, images()),
            ConfigField::choice(
                export::FILES,
                &tr!("export-files"),
                [
                    ConfigEnumOption::new(export::SEPARATE, tr!("export-files-separate")),
                    ConfigEnumOption::new(export::SINGLE, tr!("export-files-single")),
                ],
                export::SEPARATE,
            )
            .only_when(export::FORMAT, images()),
        ]
    }

    #[sismo::function("pdf_kind::export_with")]
    fn export_with(
        &self,
        doc: DocMeta<'_>,
        _content: &PdfContent,
        attachment: Option<&[u8]>,
        values: &ExportValues,
    ) -> Result<Vec<ExportFile>, ExportError> {
        use crate::readable::{ImageFormat, encode_page, flattened_pdf, pdf_pages, stack_pages};
        let item = crate::library::ItemId(doc.id.0);
        let bytes = attachment.ok_or_else(|| ExportError::Failed(tr!("export-nothing")))?;
        let failed = || ExportError::Failed(tr!("export-failed"));
        let annotated = values.str_or(export::CONTENT, export::ANNOTATED) == export::ANNOTATED;
        let format = match values.str_or(export::FORMAT, export::PDF) {
            export::PNG => ImageFormat::Png,
            export::JPG => ImageFormat::Jpg,
            _ => {
                let out = if annotated {
                    flattened_pdf(item, bytes).ok_or_else(failed)?
                } else {
                    bytes.to_vec()
                };
                return Ok(vec![ExportFile::new("pdf", "application/pdf", out)]);
            }
        };
        let count = PdfDocument::open_bytes(bytes.to_vec())
            .map_err(|_| failed())?
            .page_count();
        let pages = export::parse_pages(values.str_or(export::PAGES, ""), count)?;
        let dpi: f32 = values.str_or(export::DPI, "150").parse().unwrap_or(150.0);
        let rendered = pdf_pages(item, bytes, &pages, annotated, dpi / 72.0).ok_or_else(failed)?;
        let single = values.str_or(export::FILES, export::SEPARATE) == export::SINGLE;
        let files = if single || rendered.len() == 1 {
            let page = stack_pages(&rendered).ok_or_else(failed)?;
            vec![ExportFile::new(
                format.extension(),
                format.mime(),
                encode_page(&page, format).ok_or_else(failed)?,
            )]
        } else {
            rendered
                .iter()
                .zip(&pages)
                .map(|(page, index)| {
                    encode_page(page, format)
                        .map(|bytes| {
                            ExportFile::new(format.extension(), format.mime(), bytes)
                                .tagged((index + 1).to_string())
                        })
                        .ok_or_else(failed)
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(files)
    }
}

/// Option keys and values of the export sheet, and the page picker.
mod export {
    use notein_i18n::tr;
    use notein_plugin::kind::ExportError;

    pub(super) const FORMAT: &str = "format";
    pub(super) const PDF: &str = "pdf";
    pub(super) const PNG: &str = "png";
    pub(super) const JPG: &str = "jpg";
    pub(super) const CONTENT: &str = "content";
    pub(super) const ANNOTATED: &str = "annotated";
    pub(super) const ORIGINAL: &str = "original";
    pub(super) const PAGES: &str = "pages";
    pub(super) const DPI: &str = "dpi";
    pub(super) const FILES: &str = "files";
    pub(super) const SINGLE: &str = "single";
    pub(super) const SEPARATE: &str = "separate";

    /// Zero based pages named by `text` ("1-3, 7"; blank means every
    /// page), in order, each once.
    ///
    /// # Errors
    /// [`ExportError::BadOption`] for a page past `count` or text that is
    /// not numbers and ranges.
    pub(super) fn parse_pages(text: &str, count: usize) -> Result<Vec<usize>, ExportError> {
        let bad = |reason: String| ExportError::BadOption {
            key: PAGES.into(),
            reason,
        };
        if text.trim().is_empty() {
            return Ok((0..count).collect());
        }
        let mut pages = Vec::new();
        for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (from, to) = match part.split_once('-') {
                Some((a, b)) => (a.trim(), b.trim()),
                None => (part, part),
            };
            let (from, to): (usize, usize) = match (from.parse(), to.parse()) {
                (Ok(a), Ok(b)) => (a, b),
                _ => return Err(bad(tr!("export-pages-unreadable"))),
            };
            for page in from.min(to)..=from.max(to) {
                if page == 0 || page > count {
                    return Err(bad(
                        tr!("export-pages-out-of-range", "page" => page, "count" => count),
                    ));
                }
                if !pages.contains(&(page - 1)) {
                    pages.push(page - 1);
                }
            }
        }
        Ok(pages)
    }

    #[cfg(test)]
    mod tests {
        use super::parse_pages;

        #[test]
        fn ranges_and_singles_in_order_without_repeats() {
            assert_eq!(parse_pages("", 3).unwrap(), vec![0, 1, 2]);
            assert_eq!(parse_pages("3-1, 2, 5", 5).unwrap(), vec![0, 1, 2, 4]);
            assert!(parse_pages("7", 5).is_err());
            assert!(parse_pages("a-b", 5).is_err());
            assert!(parse_pages("0", 5).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_drop_the_extension() {
        notein_i18n::Lang::Es.make_current();
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
        let s = snippet(text, &notein_plugin::fold("metáfora")).expect("found");
        assert!(s.contains("metáfora"));
        assert!(snippet(text, "tribunal").is_none());
    }

    fn entry(
        title: &str,
        page: Option<u32>,
        top: Option<u32>,
        children: Vec<Contents>,
    ) -> Contents {
        Contents {
            title: title.into(),
            page,
            top,
            children,
        }
    }

    fn indexed() -> PdfKind {
        notein_i18n::Lang::Es.make_current();
        let kind = PdfKind::default();
        kind.store_text(
            DocId([3; 16]),
            "Romanos",
            vec![
                "Prólogo\nuno".into(),
                "el tribunal cierra".into(),
                "tres".into(),
                "cuatro".into(),
            ],
            &[
                entry("Prólogo", Some(0), None, Vec::new()),
                entry(
                    "La adopción",
                    Some(1),
                    Some(120),
                    vec![entry("Herederos", Some(2), Some(300), Vec::new())],
                ),
                entry("Apéndice", Some(3), Some(40), Vec::new()),
            ],
        );
        kind
    }

    #[test]
    fn search_reads_stored_pages() {
        let kind = indexed();
        let query = SearchQuery::new("Tribunal", 5);
        let hits: Vec<_> = kind.search(&query).collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].anchor, PdfAnchor::page(1));
        assert_eq!(hits[0].title, "p. 2");
    }

    #[test]
    fn page_numbers_and_ranges_are_offered_first() {
        let kind = indexed();
        let hits: Vec<_> = kind.search(&SearchQuery::new("p. 2-3", 5)).collect();
        assert_eq!(hits[0].anchor, PdfAnchor::pages(1, 2));
        assert_eq!(hits[0].title, "Romanos p. 2–3");
        assert_eq!(hits[0].snippet, "el tribunal cierra");
        let hits: Vec<_> = kind.search(&SearchQuery::new("9", 5)).collect();
        assert!(hits.is_empty(), "{hits:?}");
        let hits: Vec<_> = kind.search(&SearchQuery::new("3-9", 5)).collect();
        assert_eq!(hits[0].anchor, PdfAnchor::pages(2, 3));
    }

    #[test]
    fn sections_match_by_title_and_span_to_the_next_entry() {
        let kind = indexed();
        let hits: Vec<_> = kind.search(&SearchQuery::new("adopcion", 5)).collect();
        assert_eq!(hits.len(), 1);
        let anchor = &hits[0].anchor;
        assert_eq!(hits[0].title, "La adopción");
        assert_eq!(hits[0].snippet, "Romanos p. 2–4");
        assert_eq!((anchor.page, anchor.last), (1, Some(3)));
        assert_eq!((anchor.top, anchor.bottom), (Some(120), Some(40)));
        let hits: Vec<_> = kind.search(&SearchQuery::new("herederos", 5)).collect();
        let anchor = &hits[0].anchor;
        assert_eq!((anchor.page, anchor.last), (2, Some(3)));
        assert_eq!((anchor.top, anchor.bottom), (Some(300), Some(40)));
        let hits: Vec<_> = kind.search(&SearchQuery::new("apéndice", 5)).collect();
        let anchor = &hits[0].anchor;
        assert_eq!((anchor.page, anchor.last, anchor.bottom), (3, None, None));
    }

    #[test]
    fn page_queries_accept_the_usual_spellings() {
        assert_eq!(parse_page_query("23"), Some((23, 23)));
        assert_eq!(parse_page_query("p. 23"), Some((23, 23)));
        assert_eq!(parse_page_query("Pág. 23 - 25"), Some((23, 25)));
        assert_eq!(parse_page_query("23–25"), Some((23, 25)));
        assert_eq!(parse_page_query("página 4"), Some((4, 4)));
        assert_eq!(parse_page_query("0"), None);
        assert_eq!(parse_page_query("p"), None);
        assert_eq!(parse_page_query("23a"), None);
        assert_eq!(parse_page_query("tribunal"), None);
    }

    #[test]
    fn quotes_copy_the_pages_or_the_fragment() {
        let kind = indexed();
        let name = "Romanos";
        let meta = DocMeta {
            id: DocId([3; 16]),
            name,
            updated_at: 0,
            tint: None,
            paper: notein_plugin::create::Paper::default(),
        };
        let quote = kind.quote(meta, &PdfAnchor::pages(0, 1)).expect("indexed");
        assert_eq!(quote.body, "Prólogo\nuno\n\nel tribunal cierra");
        assert_eq!(quote.source, "Romanos p. 1–2");
        let fragment = PdfAnchor {
            text: Some("acto de amor".into()),
            ..PdfAnchor::page(1)
        };
        let quote = kind.quote(meta, &fragment).expect("fragment");
        assert_eq!(quote.body, "acto de amor");
        assert!(kind.quote(meta, &PdfAnchor::page(9)).is_none());
        assert!(!kind.quotes_live());
    }

    #[test]
    fn quoted_ranges_are_capped() {
        let anchor = PdfAnchor::pages(2, 40);
        assert_eq!(anchor.quoted_pages(), 2..=2 + MAX_QUOTED_PAGES - 1);
        assert_eq!(anchor.last_page(), 40);
    }

    #[test]
    fn cropping_keeps_the_rows_between_the_points() {
        let mut page = freya_pdf::write::PageImage {
            rgba: (0..40u8).flat_map(|row| [row; 4]).collect(),
            width_px: 1,
            height_px: 40,
            width_pt: 1,
            height_pt: 20,
        };
        crop_rows(&mut page, Some(5), Some(15));
        assert_eq!((page.height_px, page.height_pt), (20, 10));
        assert_eq!(page.rgba[0], 10);
        assert_eq!(page.rgba[page.rgba.len() - 1], 29);
        let mut thin = page.clone();
        crop_rows(&mut thin, Some(0), Some(1));
        assert_eq!(thin, page);
    }
}
