//! The Lienzo kind: infinite ink canvases, scratch canvases and
//! notebooks beside another document.

use std::sync::{PoisonError, RwLock, RwLockReadGuard};

use bincode::{Decode, Encode};
use freya::icons::lucide;
use notein_plugin::create::{Created, CreationEntry, CreationError, CreationFlow, CreationRequest};
use notein_plugin::doc::{DocId, DocMeta};
use notein_plugin::kind::{
    ChipView, DocumentKind, Hit, IndexReport, KindColor, KindInfo, Mark, OutlineIcon, OutlineNode,
    SearchQuery, SelectionSupport,
};

/// Kind id stored with every canvas.
pub const KIND: &str = "io.notein.canvas";
/// Create entry of a canvas.
pub const ENTRY_CANVAS: &str = "canvas";
/// Create entry of a scratch canvas (discarded unless saved).
pub const ENTRY_SCRATCH: &str = "scratch";
/// Create entry of a notebook beside another document.
pub const ENTRY_NOTEBOOK: &str = "notebook";

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
            icon: lucide::pen_line(),
            color: KindColor::Canvas,
            supports_ink: true,
            has_paper: true,
            attachment: None,
            singleton: false,
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
            .starter(0),
            CreationEntry::new(
                ENTRY_SCRATCH,
                "Borrador",
                lucide::pencil(),
                KindColor::Scratch,
            )
            .subtitle("Se descarta al cerrar si no lo guardás")
            .flow(CreationFlow::Direct),
            CreationEntry::new(
                ENTRY_NOTEBOOK,
                "Cuaderno",
                lucide::notebook_pen(),
                KindColor::Canvas,
            )
            .subtitle("Al lado de un documento")
            .flow(CreationFlow::Custom),
        ]
    }

    fn create(
        &self,
        entry: &str,
        request: CreationRequest<'_>,
    ) -> Result<Created<CanvasContent>, CreationError> {
        let fallback = match entry {
            ENTRY_CANVAS => "Lienzo",
            ENTRY_SCRATCH => "Borrador",
            ENTRY_NOTEBOOK => "Cuaderno",
            other => return Err(CreationError::UnknownEntry(other.to_owned())),
        };
        let name = if request.name.trim().is_empty() {
            fallback.to_owned()
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
