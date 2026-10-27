//! The Lienzo kind: infinite ink canvases, scratch canvases and
//! notebooks beside another document.

use std::sync::{PoisonError, RwLock, RwLockReadGuard};

use bincode::{Decode, Encode};
use freya::icons::lucide;
use notein_plugin::create::{Created, CreationEntry, CreationError, CreationFlow, CreationRequest};
use notein_plugin::doc::{DocId, DocMeta};
use notein_plugin::kind::{
    ChipView, DocumentKind, ExportFile, Hit, IndexReport, KindColor, KindInfo, Mark, OutlineIcon,
    OutlineNode, SearchQuery, SelectionSupport,
};

/// Kind id stored with every canvas.
pub const KIND: &str = "io.notein.canvas";
/// Create entry of a canvas.
pub const ENTRY_CANVAS: &str = "canvas";
/// Create entry of a scratch canvas (discarded unless saved).
pub const ENTRY_SCRATCH: &str = "scratch";
/// Create entry of a notebook beside another document.
pub const ENTRY_NOTEBOOK: &str = "notebook";
/// Paper of covers and exports of a canvas the user did not tint.
const UNTINTED_PAPER: [u8; 4] = [247, 245, 240, 255];

/// One layer, as the panel lists it.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct LayerSummary {
    pub name: String,
    pub visible: bool,
}

/// A bookmark pinned on the canvas.
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct CanvasMark {
    pub bookmark: [u8; 16],
    pub body: String,
    pub color: Option<[u8; 3]>,
    pub updated_at: u64,
}

impl CanvasMark {
    /// First line of the body, or a generic title.
    #[must_use]
    pub fn title(&self) -> String {
        self.body
            .lines()
            .map(|line| line.trim_start_matches('#').trim())
            .find(|line| !line.is_empty())
            .map_or_else(|| "Marcador".to_owned(), str::to_owned)
    }
}

/// What the shell stores for a canvas: a summary of its ink document,
/// refreshed on every save.
#[derive(Clone, Debug, PartialEq, Eq, Default, Encode, Decode)]
pub struct CanvasContent {
    pub layers: Vec<LayerSummary>,
    pub marks: Vec<CanvasMark>,
}

/// A place on a canvas.
#[derive(Clone, Debug, PartialEq, Eq, Default, Encode, Decode)]
pub struct CanvasAnchor {
    pub bookmark: Option<[u8; 16]>,
    pub layer: Option<u32>,
}

#[derive(Debug, Default)]
struct MarkIndex {
    docs: Vec<(DocId, String, Vec<CanvasMark>)>,
}

/// The canvas kind. Indexes bookmark bodies so search finds them.
#[derive(Debug, Default)]
pub struct CanvasKind {
    index: RwLock<MarkIndex>,
}

/// Bookmarks matching a query, read straight from the locked index.
pub struct MarkHits<'a> {
    index: RwLockReadGuard<'a, MarkIndex>,
    needle: String,
    doc: usize,
    mark: usize,
}

impl std::fmt::Debug for MarkHits<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MarkHits")
            .field("needle", &self.needle)
            .finish_non_exhaustive()
    }
}

impl Iterator for MarkHits<'_> {
    type Item = Hit<CanvasAnchor>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some((doc, name, marks)) = self.index.docs.get(self.doc) {
            while let Some(mark) = marks.get(self.mark) {
                self.mark += 1;
                if mark.body.to_lowercase().contains(&self.needle) {
                    return Some(Hit {
                        doc: Some(*doc),
                        anchor: CanvasAnchor {
                            bookmark: Some(mark.bookmark),
                            layer: None,
                        },
                        title: mark.title(),
                        snippet: format!("Lienzo · {name}"),
                        score: 1.0,
                    });
                }
            }
            self.doc += 1;
            self.mark = 0;
        }
        None
    }
}

impl CanvasKind {
    /// The canvas holding bookmark `bookmark`, if indexed.
    #[must_use]
    pub fn doc_of_bookmark(&self, bookmark: [u8; 16]) -> Option<DocId> {
        self.index
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .docs
            .iter()
            .find(|(_, _, marks)| marks.iter().any(|m| m.bookmark == bookmark))
            .map(|(doc, _, _)| *doc)
    }
}

impl DocumentKind for CanvasKind {
    type Anchor = CanvasAnchor;
    type Content = CanvasContent;
    type Hits<'a> = MarkHits<'a>;

    fn info(&self) -> KindInfo {
        KindInfo {
            id: KIND.into(),
            label: "Lienzo".into(),
            plural: "Lienzos".into(),
            search_group: "Marcadores".into(),
            icon: lucide::pen_line(),
            color: KindColor::Canvas,
            supports_ink: true,
            has_paper: true,
            attachment: None,
            singleton: false,
            tint: true,
            notebook: true,
            body: true,
        }
    }

    fn search<'a>(&'a self, query: &'a SearchQuery) -> MarkHits<'a> {
        MarkHits {
            index: self.index.read().unwrap_or_else(PoisonError::into_inner),
            needle: query.folded(),
            doc: 0,
            mark: 0,
        }
    }

    fn index(
        &self,
        doc: DocMeta<'_>,
        content: &CanvasContent,
        _attachment: Option<&[u8]>,
    ) -> IndexReport {
        let mut index = self.index.write().unwrap_or_else(PoisonError::into_inner);
        index.docs.retain(|(id, _, _)| *id != doc.id);
        if !content.marks.is_empty() {
            index
                .docs
                .push((doc.id, doc.name.to_owned(), content.marks.clone()));
            drop(index);
        }
        IndexReport::complete(1)
    }

    fn forget(&self, doc: DocId) {
        self.index
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .docs
            .retain(|(id, _, _)| *id != doc);
    }

    fn outline(&self, content: &CanvasContent) -> Vec<OutlineNode<CanvasAnchor>> {
        let layers = content
            .layers
            .iter()
            .enumerate()
            .rev()
            .map(|(index, layer)| {
                let anchor = CanvasAnchor {
                    bookmark: None,
                    layer: u32::try_from(index).ok(),
                };
                OutlineNode::new(layer.name.clone(), Some(anchor))
                    .icon(OutlineIcon::Layer)
                    .meta(if layer.visible { "" } else { "oculta" })
            })
            .collect();
        vec![
            OutlineNode::new("Capas", None)
                .icon(OutlineIcon::Section)
                .children(layers),
        ]
    }

    fn marks(&self, content: &CanvasContent) -> Vec<Mark<CanvasAnchor>> {
        content
            .marks
            .iter()
            .map(|mark| Mark {
                anchor: CanvasAnchor {
                    bookmark: Some(mark.bookmark),
                    layer: None,
                },
                title: mark.title(),
                snippet: mark.body.lines().skip(1).collect::<Vec<_>>().join(" "),
                color: mark.color,
                updated_at: mark.updated_at,
            })
            .collect()
    }

    fn summary(&self, content: &CanvasContent) -> Option<String> {
        match content.marks.len() {
            0 => None,
            1 => Some("1 marcador".to_owned()),
            n => Some(format!("{n} marcadores")),
        }
    }

    fn chip(&self, doc: DocMeta<'_>, anchor: &CanvasAnchor) -> ChipView {
        match anchor.bookmark {
            Some(_) => ChipView::new(format!("{} · marcador", doc.name)),
            None => ChipView::new(doc.name),
        }
    }

    fn selection(&self) -> SelectionSupport {
        SelectionSupport::NONE
    }

    fn creation_entries(&self) -> Vec<CreationEntry> {
        vec![
            CreationEntry::new(
                ENTRY_CANVAS,
                "Lienzo",
                lucide::pen_line(),
                KindColor::Canvas,
            )
            .subtitle("Escribir a mano")
            .starter(0)
            .named(),
            CreationEntry::new(
                ENTRY_SCRATCH,
                "Borrador",
                lucide::pencil(),
                KindColor::Scratch,
            )
            .subtitle("Se descarta al cerrar si no lo guardás")
            .flow(CreationFlow::Launch),
            CreationEntry::new(
                ENTRY_NOTEBOOK,
                "Cuaderno",
                lucide::notebook_pen(),
                KindColor::Canvas,
            )
            .subtitle("Al lado de un documento")
            .flow(CreationFlow::Launch),
        ]
    }

    fn create(
        &self,
        entry: &str,
        request: CreationRequest<'_>,
    ) -> Result<Created<CanvasContent>, CreationError> {
        if entry != ENTRY_CANVAS {
            return Err(CreationError::UnknownEntry(entry.to_owned()));
        }
        let name = if request.name.trim().is_empty() {
            "Lienzo".to_owned()
        } else {
            request.name.trim().to_owned()
        };
        Ok(Created {
            name,
            content: CanvasContent {
                layers: vec![LayerSummary {
                    name: "Tinta".to_owned(),
                    visible: true,
                }],
                marks: Vec::new(),
            },
            attachment: None,
        })
    }

    fn launch(&self, entry: &str) {
        match entry {
            ENTRY_SCRATCH => {
                crate::space::open(
                    crate::library::TabTarget::Scratch(crate::library::ScratchId::fresh()),
                    crate::space::OpenHint::Focused,
                );
            }
            ENTRY_NOTEBOOK => {
                crate::flows::pickers::document_picker(crate::flows::pickers::Purpose::NotebookFor);
            }
            _ => {}
        }
    }

    fn without_marks(&self, content: &mut CanvasContent) {
        content.marks.clear();
    }

    fn export(
        &self,
        doc: DocMeta<'_>,
        _content: &CanvasContent,
        _attachment: Option<&[u8]>,
    ) -> Option<ExportFile> {
        let paper = doc.tint.unwrap_or(UNTINTED_PAPER);
        crate::readable::canvas_pdf(crate::library::ItemId(doc.id.0), paper).map(|bytes| {
            ExportFile {
                extension: "pdf".into(),
                bytes,
            }
        })
    }
}

impl CanvasContent {
    /// Summary of an ink document.
    #[must_use]
    pub fn of(doc: &crate::doc::Doc) -> Self {
        Self {
            layers: doc
                .layers
                .iter()
                .map(|layer| LayerSummary {
                    name: layer.name.clone(),
                    visible: layer.visible,
                })
                .collect(),
            marks: doc
                .bookmarks()
                .iter()
                .map(|bookmark| CanvasMark {
                    bookmark: bookmark.id.0,
                    body: bookmark.body.clone(),
                    color: bookmark.color.map(|[r, g, b, _]| [r, g, b]),
                    updated_at: bookmark.updated_at,
                })
                .collect(),
        }
    }
}

/// Shortest pause between two cover renders of the same canvas.
const COVER_INTERVAL: std::time::Duration = std::time::Duration::from_secs(4);

static LAST_COVER: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<crate::library::ItemId, std::time::Instant>>,
> = std::sync::LazyLock::new(Default::default);

/// After a canvas was saved: refresh its summary (layers, bookmarks),
/// search index and, at most every few seconds, its cover. Runs on the
/// thread that saved it.
pub fn after_save(item: crate::library::ItemId, content: &CanvasContent, snapshot: &[u8]) {
    let due = {
        let mut last = LAST_COVER.lock().unwrap_or_else(PoisonError::into_inner);
        let now = std::time::Instant::now();
        let due = last
            .get(&item)
            .is_none_or(|at| now.duration_since(*at) >= COVER_INTERVAL);
        if due {
            last.insert(item, now);
        }
        due
    };
    crate::plugins::content::write(
        item,
        KIND,
        &notein_plugin::payload::Payload::to_bytes(content),
    );
    crate::plugins::content::index_blocking(item);
    if !due {
        return;
    }
    let Ok(doc) = crate::doc::Doc::from_snapshot(snapshot) else {
        return;
    };
    let Ok(handle) = crate::library::shared_blocking() else {
        return;
    };
    let paper = pollster::block_on(handle.lock())
        .item(item)
        .and_then(|i| i.color)
        .unwrap_or(UNTINTED_PAPER);
    let cover = crate::thumbnail::canvas(&doc, paper);
    let mut lib = pollster::block_on(handle.lock());
    if let Err(err) = pollster::block_on(lib.set_item_thumbnail(item, cover)) {
        log::error!("cover {item}: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(body: &str) -> CanvasMark {
        CanvasMark {
            bookmark: [1; 16],
            body: body.to_owned(),
            color: None,
            updated_at: 0,
        }
    }

    #[test]
    fn search_walks_bookmarks_of_every_canvas() {
        let kind = CanvasKind::default();
        let content = CanvasContent {
            layers: Vec::new(),
            marks: vec![mark("# Adopción\nde hijos"), mark("Otra cosa")],
        };
        let meta = DocMeta {
            id: DocId([2; 16]),
            name: "Mapa",
            updated_at: 0,
            tint: None,
        };
        kind.index(meta, &content, None);
        let query = SearchQuery::new("ADOP", 10);
        let hits: Vec<_> = kind.search(&query).collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Adopción");
        kind.forget(DocId([2; 16]));
        assert_eq!(kind.search(&query).count(), 0);
    }
}
