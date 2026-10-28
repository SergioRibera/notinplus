//! The Nota kind.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLockReadGuard};

use bincode::{Decode, Encode};
use freya::icons::lucide;
use freya_markdown_editor::document::{Document, HeadingRef};
use notein_sdk::bookmark::SourceRef;
use notein_sdk::create::{
    Created, CreationEntry, CreationError, CreationRequest, ImportPreview, ImportedFile,
};
use notein_sdk::doc::{DocId, DocMeta};
use notein_sdk::kind::{
    ActionEffect, ChipView, DocumentKind, ExportFile, Hit, IndexReport, KindAction, KindColor,
    KindInfo, OutlineIcon, OutlineNode, Quote, SearchQuery,
};
use notein_sdk::services::Host;
use notein_sdk::text::Folded;

use crate::anchor::NoteAnchor;
use crate::index::{IndexedNote, NotesIndex};
use crate::{IMPORT_EXTENSIONS, IMPORT_MIME_TYPES, import_name};

/// Kind id stored with every note.
pub const KIND: &str = "io.notein.note";
/// Create entry of a note.
pub const ENTRY_NOTE: &str = "note";
/// Create entry of a markdown import.
pub const ENTRY_IMPORT_MARKDOWN: &str = "import-markdown";
/// Document action: copy the note as markdown.
pub const ACTION_COPY_MARKDOWN: &str = "copy-markdown";
/// Document action: share the note as a `.md` file.
pub const ACTION_SHARE_FILE: &str = "share-md";

const SNIPPET_CONTEXT: usize = 40;

/// What the shell stores for a note: its markdown.
#[derive(Clone, Debug, PartialEq, Eq, Default, Encode, Decode)]
pub struct NoteContent {
    pub markdown: String,
}

impl NoteContent {
    /// Stored bytes of a note with `markdown`.
    #[must_use]
    pub fn encode(markdown: &str) -> Vec<u8> {
        use notein_sdk::payload::Payload;
        Self {
            markdown: markdown.to_owned(),
        }
        .to_bytes()
    }

    /// The markdown of stored note bytes; empty when they are not a note.
    #[must_use]
    pub fn markdown_of(bytes: &[u8]) -> String {
        use notein_sdk::payload::Payload;
        Self::from_bytes(bytes)
            .map(|c| c.markdown)
            .unwrap_or_default()
    }
}

/// The note kind. Shares its index with whoever renders notes.
#[derive(Debug, Default)]
pub struct NotesKind {
    index: NotesIndex,
    /// The shell, once the plugin is installed: fixing a cited heading
    /// in a note that is not on screen writes its body.
    host: OnceLock<Host>,
}

impl NotesKind {
    #[must_use]
    pub fn new(index: NotesIndex) -> Self {
        Self {
            index,
            host: OnceLock::new(),
        }
    }

    /// Hand the kind the shell; the first one stays.
    pub fn attach(&self, host: &Host) {
        let _ = self.host.set(Arc::clone(host));
    }

    #[must_use]
    pub const fn notes_index(&self) -> &NotesIndex {
        &self.index
    }
}

/// Matches over every indexed note, read from the locked index: title
/// matches first, then headings, then body text.
pub struct NoteHits<'a> {
    notes: RwLockReadGuard<'a, HashMap<[u8; 16], IndexedNote>>,
    order: Vec<[u8; 16]>,
    needle: String,
    /// Notes already returned by an earlier pass.
    seen: Vec<[u8; 16]>,
    pass: u8,
    note: usize,
    heading: usize,
}

impl std::fmt::Debug for NoteHits<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoteHits")
            .field("needle", &self.needle)
            .field("pass", &self.pass)
            .field("note", &self.note)
            .finish_non_exhaustive()
    }
}

impl NoteHits<'_> {
    const fn hit(
        doc: [u8; 16],
        anchor: NoteAnchor,
        title: String,
        snippet: String,
        score: f32,
    ) -> Hit<NoteAnchor> {
        Hit {
            doc: Some(DocId(doc)),
            anchor,
            title,
            snippet,
            score,
        }
    }
}

impl Iterator for NoteHits<'_> {
    type Item = Hit<NoteAnchor>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.needle.is_empty() {
            return None;
        }
        while self.pass < 3 {
            while let Some(id) = self.order.get(self.note).copied() {
                let Some(note) = self.notes.get(&id) else {
                    self.note += 1;
                    continue;
                };
                match self.pass {
                    0 => {
                        self.note += 1;
                        if Folded::new(&note.title).find(&self.needle).is_some() {
                            self.seen.push(id);
                            return Some(Self::hit(
                                id,
                                NoteAnchor::default(),
                                note.title.clone(),
                                "Nota".to_owned(),
                                3.0,
                            ));
                        }
                    }
                    1 => {
                        if let Some((slug, text)) = note.headings.get(self.heading) {
                            self.heading += 1;
                            if Folded::new(text).find(&self.needle).is_some() {
                                self.seen.push(id);
                                return Some(Self::hit(
                                    id,
                                    NoteAnchor::heading(slug.clone()),
                                    format!("{} › {text}", note.title),
                                    "Sección de nota".to_owned(),
                                    2.0,
                                ));
                            }
                            continue;
                        }
                        self.heading = 0;
                        self.note += 1;
                    }
                    _ => {
                        self.note += 1;
                        if self.seen.contains(&id) {
                            continue;
                        }
                        if let Some(excerpt) =
                            Folded::new(&note.body).excerpt(&self.needle, SNIPPET_CONTEXT)
                        {
                            return Some(Self::hit(
                                id,
                                NoteAnchor::default(),
                                note.title.clone(),
                                excerpt,
                                1.0,
                            ));
                        }
                    }
                }
            }
            self.pass += 1;
            self.note = 0;
        }
        None
    }
}

/// Turn the flat heading list into a tree: a heading owns the deeper
/// headings that follow it.
fn nest(headings: &[HeadingRef]) -> Vec<OutlineNode<NoteAnchor>> {
    fn attach(
        stack: &mut [(u8, OutlineNode<NoteAnchor>)],
        roots: &mut Vec<OutlineNode<NoteAnchor>>,
        node: OutlineNode<NoteAnchor>,
    ) {
        match stack.last_mut() {
            Some((_, parent)) => parent.children.push(node),
            None => roots.push(node),
        }
    }
    let mut roots = Vec::new();
    let mut stack: Vec<(u8, OutlineNode<NoteAnchor>)> = Vec::new();
    for heading in headings {
        let level = heading.level.get();
        while let Some((_, done)) = stack.pop_if(|(open, _)| *open >= level) {
            attach(&mut stack, &mut roots, done);
        }
        let label = if heading.text.trim().is_empty() {
            "(sin título)".to_owned()
        } else {
            heading.text.clone()
        };
        let node = OutlineNode::new(label, Some(NoteAnchor::heading(heading.slug.clone())))
            .icon(OutlineIcon::Heading);
        stack.push((level, node));
    }
    while let Some((_, done)) = stack.pop() {
        attach(&mut stack, &mut roots, done);
    }
    roots
}

impl DocumentKind for NotesKind {
    type Anchor = NoteAnchor;
    type Content = NoteContent;
    type Hits<'a> = NoteHits<'a>;

    fn info(&self) -> KindInfo {
        KindInfo {
            id: KIND.into(),
            label: "Nota".into(),
            plural: "Notas".into(),
            search_group: "En notas".into(),
            icon: lucide::sticky_note(),
            color: KindColor::Note,
            supports_ink: false,
            has_paper: false,
            attachment: None,
            shared_refs: false,
            tint: true,
            notebook: false,
            body: true,
            records: false,
        }
    }

    fn search<'a>(&'a self, query: &'a SearchQuery) -> NoteHits<'a> {
        let notes = self.index.read();
        let mut order: Vec<[u8; 16]> = notes.keys().copied().collect();
        order.sort_by_key(|id| {
            notes
                .get(id)
                .map(|n| n.title.to_lowercase())
                .unwrap_or_default()
        });
        NoteHits {
            notes,
            order,
            needle: query.folded(),
            seen: Vec::new(),
            pass: 0,
            note: 0,
            heading: 0,
        }
    }

    fn index(
        &self,
        doc: DocMeta<'_>,
        content: &NoteContent,
        _attachment: Option<&[u8]>,
    ) -> IndexReport {
        self.index.update(doc.id.0, doc.name, &content.markdown);
        IndexReport::complete(1)
    }

    fn forget(&self, doc: DocId) {
        self.index.remove(doc.0);
    }

    fn outline(&self, content: &NoteContent) -> Vec<OutlineNode<NoteAnchor>> {
        nest(&Document::parse(&content.markdown).outline())
    }

    fn chip(&self, doc: DocMeta<'_>, anchor: &NoteAnchor) -> ChipView {
        self.index
            .label(doc.id.0, anchor)
            .map_or_else(|| ChipView::broken(doc.name), ChipView::new)
    }

    fn quotes(&self) -> bool {
        true
    }

    fn quote(&self, doc: DocMeta<'_>, anchor: &NoteAnchor) -> Option<Quote> {
        let source = self.index.label(doc.id.0, anchor)?;
        let live = self
            .host
            .get()
            .map(|host| host.body(doc.id).text())
            .filter(|text| !text.is_empty());
        let markdown = live.or_else(|| self.index.body(doc.id.0))?;
        let body = crate::section(&markdown, anchor.heading.as_deref())?;
        Some(Quote { body, source })
    }

    fn actions(&self) -> Vec<KindAction> {
        vec![
            KindAction {
                id: ACTION_COPY_MARKDOWN.into(),
                label: "Copiar como markdown".into(),
                icon: lucide::clipboard_copy(),
            },
            KindAction {
                id: ACTION_SHARE_FILE.into(),
                label: "Enviar como archivo .md".into(),
                icon: lucide::share_2(),
            },
        ]
    }

    fn run_action(&self, id: &str, doc: DocMeta<'_>, content: &NoteContent) -> ActionEffect {
        match id {
            ACTION_COPY_MARKDOWN => ActionEffect::Copy {
                text: content.markdown.clone(),
                toast: "Nota copiada como markdown".to_owned(),
            },
            ACTION_SHARE_FILE => ActionEffect::Share {
                file_name: format!("{}.md", doc.name.trim()),
                mime: "text/markdown".to_owned(),
                bytes: content.markdown.clone().into_bytes(),
            },
            _ => ActionEffect::Done,
        }
    }

    fn creation_entries(&self) -> Vec<CreationEntry> {
        vec![
            CreationEntry::new(ENTRY_NOTE, "Nota", lucide::sticky_note(), KindColor::Note)
                .subtitle("Texto y referencias")
                .starter(1)
                .named()
                .sheet(
                    "Nueva nota",
                    Some("Vacío = toma el nombre de la primera línea"),
                    false,
                ),
            CreationEntry::new(
                ENTRY_IMPORT_MARKDOWN,
                "Markdown",
                lucide::file_input(),
                KindColor::Note,
            )
            .subtitle("Importar un archivo .md")
            .import(
                "Elegí un archivo markdown",
                &IMPORT_EXTENSIONS,
                &IMPORT_MIME_TYPES,
            ),
        ]
    }

    fn create(
        &self,
        entry: &str,
        request: CreationRequest<'_>,
    ) -> Result<Created<NoteContent>, CreationError> {
        match entry {
            ENTRY_NOTE => Ok(Created {
                name: request.name.trim().to_owned(),
                content: NoteContent::default(),
                attachment: None,
            }),
            ENTRY_IMPORT_MARKDOWN => {
                let file = request.file.ok_or(CreationError::MissingFile)?;
                let markdown = String::from_utf8(file.bytes.to_vec())
                    .map_err(|_| CreationError::Unreadable("El archivo no es texto.".to_owned()))?;
                let name = if request.name.trim().is_empty() {
                    import_name(Some(file.name), &markdown)
                } else {
                    request.name.trim().to_owned()
                };
                Ok(Created {
                    name,
                    content: NoteContent { markdown },
                    attachment: None,
                })
            }
            other => Err(CreationError::UnknownEntry(other.to_owned())),
        }
    }

    fn inspect(
        &self,
        _entry: &str,
        file: ImportedFile<'_>,
    ) -> Result<ImportPreview, CreationError> {
        let text = String::from_utf8_lossy(file.bytes).into_owned();
        let headings = Document::parse(&text).outline().len();
        let words = text.split_whitespace().count();
        Ok(ImportPreview {
            name: import_name(Some(file.name), &text),
            details: Some(format!(
                "{} · {} · {}",
                file.name,
                notein_ui_kit::count(headings, "título", "títulos"),
                notein_ui_kit::count(words, "palabra", "palabras")
            )),
            preview: Some(text.lines().take(8).collect::<Vec<_>>().join("\n")),
        })
    }

    fn excerpt(&self, content: &NoteContent) -> Option<String> {
        Some(content.markdown.clone())
    }

    fn references(&self, content: &NoteContent) -> Option<Vec<SourceRef>> {
        Some(notein_sdk::link::scan_links(&content.markdown).collect())
    }

    fn export(
        &self,
        _doc: DocMeta<'_>,
        content: &NoteContent,
        _attachment: Option<&[u8]>,
    ) -> Option<ExportFile> {
        Some(ExportFile {
            extension: "md".into(),
            bytes: content.markdown.clone().into_bytes(),
        })
    }

    /// Fix the anchor of the cited heading, so the link survives the
    /// heading being renamed: in the open editor when the note is on
    /// screen, in its body otherwise.
    fn cited(&self, doc: DocId, anchor: &NoteAnchor) {
        let Some(slug) = anchor.heading.as_deref() else {
            return;
        };
        if let Some(controller) = crate::face::open_editor(doc) {
            let heading = controller
                .document()
                .outline()
                .into_iter()
                .find(|heading| heading.slug == slug && !heading.pinned);
            if let Some(heading) = heading {
                controller.set_heading_anchor(heading.block, Some(heading.slug));
            }
            return;
        }
        let Some(host) = self.host.get() else {
            return;
        };
        let body = host.body(doc);
        if let Some(pinned) = crate::pin_heading(&body.text(), slug) {
            body.set(&pinned);
            host.store(doc, KIND).save(NoteContent::encode(&pinned));
        }
    }
}

/// Name a note takes from its first non-empty line when the user left
/// the name empty.
#[must_use]
pub fn name_from_body(markdown: &str) -> Option<String> {
    markdown
        .lines()
        .map(|line| line.trim_start_matches('#').trim())
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(80).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_heading_quotes_its_section_from_the_index() {
        let kind = indexed();
        let meta = DocMeta {
            id: DocId([7; 16]),
            name: "Apuntes del sábado",
            updated_at: 0,
            tint: None,
        };
        let quote = kind
            .quote(meta, &NoteAnchor::heading("idea-central"))
            .expect("quote");
        assert!(quote.body.starts_with("La adopción"), "{}", quote.body);
        assert_eq!(quote.source, "Apuntes del sábado › Idea central");
        assert!(kind.quote(meta, &NoteAnchor::heading("nada")).is_none());
    }

    fn indexed() -> NotesKind {
        let kind = NotesKind::default();
        let meta = DocMeta {
            id: DocId([7; 16]),
            name: "Apuntes del sábado",
            updated_at: 0,
            tint: None,
        };
        let content = NoteContent {
            markdown: "# Idea central\n\nLa adopción no es sólo legal.\n\n## Preguntas\n"
                .to_owned(),
        };
        kind.index(meta, &content, None);
        kind
    }

    #[test]
    fn titles_then_headings_then_body() {
        let kind = indexed();
        let titles: Vec<_> = kind
            .search(&SearchQuery::new("sábado", 10))
            .map(|h| h.score)
            .collect();
        assert_eq!(titles, [3.0]);
        let headings: Vec<_> = kind
            .search(&SearchQuery::new("PREGUNTAS", 10))
            .map(|h| h.title)
            .collect();
        assert_eq!(headings, ["Apuntes del sábado › Preguntas"]);
        let body: Vec<_> = kind
            .search(&SearchQuery::new("adopción", 10))
            .map(|h| h.snippet)
            .collect();
        assert_eq!(body.len(), 1);
        assert!(body[0].contains("adopción"));
    }

    #[test]
    fn outline_nests_headings_by_level() {
        let content = NoteContent {
            markdown: "# A\n\n## A.1\n\n### A.1.a\n\n## A.2\n\n# B".to_owned(),
        };
        let outline = NotesKind::default().outline(&content);
        let labels = |nodes: &[OutlineNode<NoteAnchor>]| -> Vec<String> {
            nodes.iter().map(|node| node.label.clone()).collect()
        };
        assert_eq!(labels(&outline), ["A", "B"]);
        assert_eq!(labels(&outline[0].children), ["A.1", "A.2"]);
        assert_eq!(labels(&outline[0].children[0].children), ["A.1.a"]);
    }

    #[test]
    fn first_line_names_an_unnamed_note() {
        assert_eq!(
            name_from_body("\n\n# Diario\nhoy").as_deref(),
            Some("Diario")
        );
        assert_eq!(name_from_body("   \n"), None);
    }
}
