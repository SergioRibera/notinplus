//! Unified search: documents by name and tag, and places inside them
//! from every kind's own search.

use notein_i18n::tr;
use notein_plugin::bookmark::SourceRef;
use notein_plugin::kind::{KindColor, SearchQuery};

use crate::library::{ItemId, LibraryData, ROOT_FOLDER};
use crate::persist::Persisted;

/// Most hits kept per kind for one query.
pub const HITS_PER_KIND: usize = 40;
/// Most documents kept by name.
const DOCS: usize = 40;
/// Recent queries remembered.
const RECENT: usize = 8;

/// A document matched by name or tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocMatch {
    pub item: ItemId,
    pub name: String,
    pub kind: String,
    /// "Lienzo · Estudio".
    pub meta: String,
}

/// A place inside a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceMatch {
    /// Where it lives: the document name, or the kind's label for
    /// shared content (a Bible translation).
    pub doc_name: String,
    pub title: String,
    pub snippet: String,
    pub target: SourceRef,
}

/// Hits of one kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaceGroup {
    pub kind: String,
    /// "En la Biblia".
    pub label: String,
    pub color: KindColor,
    pub icon: freya::prelude::Bytes,
    pub hits: Vec<PlaceMatch>,
}

/// A document whose text is not searchable yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Indexing {
    pub name: String,
    pub done: u32,
    pub total: u32,
}

/// Everything one query found.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Results {
    pub query: String,
    pub documents: Vec<DocMatch>,
    pub groups: Vec<PlaceGroup>,
    pub indexing: Vec<Indexing>,
}

impl Results {
    /// Number of results of every kind.
    #[must_use]
    pub fn total(&self) -> usize {
        self.documents.len() + self.groups.iter().map(|g| g.hits.len()).sum::<usize>()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

fn folder_name(data: &LibraryData, folder: crate::library::FolderId) -> String {
    if folder == ROOT_FOLDER {
        tr!("common-library")
    } else {
        data.folder(folder)
            .map_or_else(|| tr!("common-library"), |f| f.name.clone())
    }
}

/// Run `query` against the library and every active kind. Blocking:
/// call it off the UI thread.
#[must_use]
#[sismo::function("search::run")]
pub fn run(query: &str, data: &LibraryData) -> Results {
    let text = query.trim();
    let mut results = Results {
        query: text.to_owned(),
        ..Results::default()
    };
    if text.is_empty() {
        return results;
    }
    let needle = text.to_lowercase();
    results.documents = {
        sismo::scope!("search::library");
        let tag_hits: Vec<_> = data
            .tags()
            .iter()
            .filter(|t| t.name.to_lowercase().contains(&needle))
            .map(|t| t.id)
            .collect();
        data.live_items()
            .filter(|item| {
                item.name.to_lowercase().contains(&needle)
                    || item.tags.iter().any(|t| tag_hits.contains(t))
            })
            .take(DOCS)
            .map(|item| {
                let label = crate::plugins::kind_info(&item.kind.plugin_id)
                    .map_or_else(|| tr!("find-document"), |k| k.label.into_owned());
                DocMatch {
                    item: item.id,
                    name: item.name.clone(),
                    kind: item.kind.plugin_id.clone(),
                    meta: format!("{label} · {}", folder_name(data, item.folder)),
                }
            })
            .collect()
    };

    let catalog = crate::plugins::catalog();
    let found_by_kind = {
        sismo::scope!("search::kinds");
        catalog.search(&SearchQuery::new(text, HITS_PER_KIND))
    };
    for found in found_by_kind {
        let Some(kind) = catalog.kind(&found.kind) else {
            continue;
        };
        let info = kind.info();
        let hits = found
            .hits
            .into_iter()
            .map(|hit| {
                let doc = hit.doc.map(|d| ItemId(d.0));
                let doc_name = doc
                    .and_then(|d| data.item(d))
                    .map_or_else(|| info.label.to_string(), |i| i.name.clone());
                PlaceMatch {
                    doc_name,
                    title: hit.title,
                    snippet: hit.snippet,
                    target: SourceRef::Plugin {
                        plugin_id: found.kind.clone(),
                        item: hit.doc.map_or([0; 16], |d| d.0),
                        anchor: hit.anchor,
                        selection: None,
                    },
                }
            })
            .collect();
        results.groups.push(PlaceGroup {
            kind: found.kind,
            label: info.search_group.into_owned(),
            color: info.color,
            icon: info.icon,
            hits,
        });
    }
    sismo::scope!("search::index_status");
    results.indexing = catalog
        .kinds()
        .iter()
        .flat_map(|kind| kind.index_status())
        .filter_map(|(doc, report)| {
            let name = data.item(ItemId(doc.0))?.name.clone();
            (!report.is_complete()).then_some(Indexing {
                name,
                done: report.done,
                total: report.total,
            })
        })
        .collect();
    results
}

/// Run `query` on a background thread.
#[sismo::function]
pub async fn run_async(query: String, data: std::sync::Arc<LibraryData>) -> Results {
    let (tx, rx) = flume::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("notein-search".to_owned())
        .spawn(move || {
            let _ = tx.send(run(&query, &data));
        });
    if let Err(err) = spawned {
        log::error!("search thread: {err}");
        return Results::default();
    }
    rx.recv_async().await.unwrap_or_default()
}

/// Recent queries, newest first. Device local.
#[istmo::message]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    pub recent: Vec<String>,
}

impl Persisted for History {
    const FILE: &'static str = "search-history.v2.bin";
}

impl History {
    /// Remember `query` at the top.
    pub fn push(&mut self, query: &str) {
        let query = query.trim();
        if query.is_empty() {
            return;
        }
        self.recent.retain(|q| !q.eq_ignore_ascii_case(query));
        self.recent.insert(0, query.to_owned());
        self.recent.truncate(RECENT);
    }
}

#[cfg(test)]
mod tests {
    use super::History;

    #[test]
    fn history_dedupes_and_caps() {
        let mut history = History::default();
        for q in [
            "uno", "dos", "UNO", " ", "tres", "4", "5", "6", "7", "8", "9",
        ] {
            history.push(q);
        }
        assert_eq!(history.recent.len(), 8);
        assert_eq!(history.recent[0], "9");
        assert_eq!(
            history
                .recent
                .iter()
                .filter(|q| q.eq_ignore_ascii_case("uno"))
                .count(),
            1
        );
    }
}
