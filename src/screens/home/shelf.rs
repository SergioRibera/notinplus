//! What the library area of Home lists: the folders and documents of
//! one folder, filtered by kind and sorted, ready for cards.

use freya::prelude::Bytes;

use super::card::{DocCardData, FolderCardData};
use crate::library::{FolderId, Item, ItemId, LibraryData};
use crate::plugins::content::Glance;
use crate::settings::SortOrder;

/// Which documents the type filter keeps.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    All,
    Kind(String),
    Folders,
}

impl Filter {
    fn keeps(&self, item: &Item) -> bool {
        match self {
            Self::All => true,
            Self::Kind(kind) => item.kind.plugin_id == *kind,
            Self::Folders => false,
        }
    }

    const fn keeps_folders(&self) -> bool {
        matches!(self, Self::All | Self::Folders)
    }
}

/// One filter chip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterChip {
    pub filter: Filter,
    pub label: String,
}

/// The filter chips: everything, each installed kind, then folders.
#[must_use]
pub fn filter_chips() -> Vec<FilterChip> {
    let mut chips = vec![FilterChip {
        filter: Filter::All,
        label: "Todo".to_owned(),
    }];
    chips.extend(
        crate::plugins::catalog()
            .kinds()
            .iter()
            .map(|k| k.info())
            .map(|info| FilterChip {
                filter: Filter::Kind(info.id.into_owned()),
                label: info.plural.into_owned(),
            }),
    );
    chips.push(FilterChip {
        filter: Filter::Folders,
        label: "Carpetas".to_owned(),
    });
    chips
}

/// A folder on the shelf.
#[derive(Clone, Debug, PartialEq)]
pub struct FolderEntry {
    pub id: FolderId,
    pub card: FolderCardData,
}

/// A document on the shelf.
#[derive(Clone, Debug, PartialEq)]
pub struct DocEntry {
    pub id: ItemId,
    pub card: DocCardData,
}

/// The listed folder.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Shelf {
    pub folders: Vec<FolderEntry>,
    pub docs: Vec<DocEntry>,
}

impl Shelf {
    /// Contents of `folder` under `filter`, in `sort` order.
    #[must_use]
    pub fn build(data: &LibraryData, folder: FolderId, filter: &Filter, sort: SortOrder) -> Self {
        let now = crate::format::now();
        let shared: Vec<ItemId> = crate::sync::snapshot()
            .shares
            .into_iter()
            .flat_map(|s| s.docs)
            .collect();
        let mut items: Vec<&Item> = data
            .items_of(folder)
            .into_iter()
            .filter(|i| filter.keeps(i))
            .collect();
        let mut folders = if filter.keeps_folders() {
            data.folders_of(folder)
        } else {
            Vec::new()
        };
        match sort {
            SortOrder::Modified => {
                items.sort_by_key(|i| std::cmp::Reverse(i.updated_at));
                folders.sort_by_key(|f| std::cmp::Reverse(f.updated_at));
            }
            SortOrder::Created => {
                items.sort_by_key(|i| std::cmp::Reverse(i.created_at));
                folders.sort_by_key(|f| std::cmp::Reverse(f.created_at));
            }
            SortOrder::Name | SortOrder::Kind => {
                items.sort_by_cached_key(|i| i.name.to_lowercase());
                folders.sort_by_cached_key(|f| f.name.to_lowercase());
                if sort == SortOrder::Kind {
                    items.sort_by(|a, b| a.kind.plugin_id.cmp(&b.kind.plugin_id));
                }
            }
        }
        Self {
            folders: folders
                .into_iter()
                .map(|f| FolderEntry {
                    id: f.id,
                    card: FolderCardData {
                        name: f.name.clone(),
                        color: f.color,
                        count: data.items_of(f.id).len() + data.folders_of(f.id).len(),
                    },
                })
                .collect(),
            docs: items
                .into_iter()
                .map(|item| DocEntry {
                    id: item.id,
                    card: card(data, item, shared.contains(&item.id), now),
                })
                .collect(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.folders.is_empty() && self.docs.is_empty()
    }
}

/// Card content of `item`.
#[must_use]
pub fn card(data: &LibraryData, item: &Item, shared: bool, now: u64) -> DocCardData {
    let glance = Glance::of(item);
    let when = crate::format::ago(item.updated_at, now);
    let label = crate::plugins::kind_info(&item.kind.plugin_id)
        .map_or_else(|| "Documento".to_owned(), |k| k.label.into_owned());
    let meta = match glance.summary {
        Some(summary) => format!("{summary} · {when}"),
        None => format!("{label} · {when}"),
    };
    DocCardData {
        name: item.name.clone(),
        kind: item.kind.plugin_id.clone(),
        meta,
        color: item.color,
        background: item.background,
        thumbnail: item.thumbnail.clone().map(Bytes::from),
        notebooks: data.live_notebooks_of(item.id).len(),
        pinned: data.is_pinned(item.id),
        shared,
        preview: glance.preview,
    }
}
