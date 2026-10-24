//! Trash: deleting moves things here; they come back with Deshacer or
//! from the trash screen, and leave for good after the retention the
//! user picked.

use super::{
    Folder, FolderId, Item, ItemId, Library, LibraryData, LibraryError, LinkKind, NotFoundKind,
    ROOT_FOLDER, Result, Trashed, now_millis,
};

/// What happens to the notebooks of a document moved to the trash.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NotebookFate {
    /// They stay in the library, no longer linked to the document.
    #[default]
    Keep,
    /// They go to the trash with the document and restore with it.
    Trash,
}

/// Result of a move to the trash; `batch` undoes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrashReceipt {
    pub batch: u64,
    /// Items moved, counting the ones inside trashed folders.
    pub items: usize,
}

/// What a trash row holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrashedRecord {
    Item(Item),
    /// A folder and how many items went to the trash with it.
    Folder {
        folder: Folder,
        items: usize,
    },
}

/// Something the trash screen lists: only the top of each batch, so a
/// trashed folder is one row however much it held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrashEntry {
    pub trashed: Trashed,
    pub record: TrashedRecord,
}

impl TrashEntry {
    #[must_use]
    pub fn name(&self) -> &str {
        match &self.record {
            TrashedRecord::Item(item) => &item.name,
            TrashedRecord::Folder { folder, .. } => &folder.name,
        }
    }
}

/// Days a trashed record stays before it is purged; `None` keeps it.
#[must_use]
pub const fn retention_millis(days: Option<u32>) -> Option<u64> {
    match days {
        Some(days) => Some(days as u64 * 24 * 60 * 60 * 1000),
        None => None,
    }
}

impl Library {
    fn new_batch() -> u64 {
        let bytes = uuid::Uuid::new_v4().into_bytes();
        u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ])
    }

    /// Move `ids` to the trash in one batch. Notebooks follow `fate`.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] for an unknown id; persistence errors.
    pub async fn trash_items(
        &mut self,
        ids: &[ItemId],
        fate: NotebookFate,
    ) -> Result<TrashReceipt> {
        let batch = Self::new_batch();
        let at = now_millis();
        let mut moved = Vec::new();
        for id in ids {
            let notebooks = self.live_notebooks_of(*id);
            match fate {
                NotebookFate::Trash => moved.extend(notebooks),
                NotebookFate::Keep => {
                    let links: Vec<_> = self
                        .links_of(*id)
                        .filter(|l| l.kind == LinkKind::Companion && l.from == *id)
                        .map(|l| l.id)
                        .collect();
                    for link in links {
                        self.delete_link(link).await?;
                    }
                }
            }
            moved.push(*id);
        }
        moved.sort_unstable();
        moved.dedup();
        for id in &moved {
            let item = self
                .data
                .index
                .item_mut(*id)
                .ok_or(LibraryError::NotFound(NotFoundKind::Item(*id)))?;
            item.trashed = Some(Trashed {
                at,
                from: item.folder,
                batch,
            });
            item.updated_at = at;
        }
        self.data.index.pins.retain(|p| !moved.contains(p));
        self.persist().await?;
        Ok(TrashReceipt {
            batch,
            items: moved.len(),
        })
    }

    /// Move a folder, its subfolders and everything inside them to the
    /// trash in one batch.
    ///
    /// # Errors
    /// [`LibraryError::NotFound`] / [`LibraryError::RootImmutable`];
    /// persistence errors.
    pub async fn trash_folder(&mut self, id: FolderId) -> Result<TrashReceipt> {
        Self::reject_root(id)?;
        if self.data.index.folder(id).is_none() {
            return Err(LibraryError::NotFound(NotFoundKind::Folder(id)));
        }
        let batch = Self::new_batch();
        let at = now_millis();
        let subtree = self.collect_subtree(id);
        for folder in self
            .data
            .index
            .folders
            .iter_mut()
            .filter(|f| subtree.contains(&f.id))
        {
            if folder.trashed.is_none() {
                folder.trashed = Some(Trashed {
                    at,
                    from: folder.parent,
                    batch,
                });
                folder.updated_at = at;
            }
        }
        let mut items = 0;
        let mut trashed_ids = Vec::new();
        for item in self
            .data
            .index
            .items
            .iter_mut()
            .filter(|i| subtree.contains(&i.folder))
        {
            if item.trashed.is_none() {
                item.trashed = Some(Trashed {
                    at,
                    from: item.folder,
                    batch,
                });
                item.updated_at = at;
                trashed_ids.push(item.id);
                items += 1;
            }
        }
        self.data.index.pins.retain(|p| !trashed_ids.contains(p));
        self.persist().await?;
        Ok(TrashReceipt { batch, items })
    }

    /// Bring back everything moved by `batch`. Records whose folder no
    /// longer exists land at the library root.
    ///
    /// # Errors
    /// Persistence errors.
    pub async fn restore_batch(&mut self, batch: u64) -> Result<usize> {
        let at = now_millis();
        let mut restored = 0;
        let live_or_restoring = |index: &super::LibraryIndex, folder: FolderId| {
            folder == ROOT_FOLDER
                || index
                    .folder(folder)
                    .is_some_and(|f| f.trashed.is_none_or(|t| t.batch == batch))
        };
        let folder_targets: Vec<(FolderId, FolderId)> = self
            .data
            .index
            .folders
            .iter()
            .filter_map(|f| {
                f.trashed
                    .filter(|t| t.batch == batch)
                    .map(|t| (f.id, t.from))
            })
            .map(|(id, from)| {
                let parent = if live_or_restoring(&self.data.index, from) {
                    from
                } else {
                    ROOT_FOLDER
                };
                (id, parent)
            })
            .collect();
        for (id, parent) in folder_targets {
            if let Some(folder) = self.data.index.folder_mut(id) {
                folder.trashed = None;
                folder.parent = parent;
                folder.updated_at = at;
            }
        }
        let item_targets: Vec<(ItemId, FolderId)> = self
            .data
            .index
            .items
            .iter()
            .filter_map(|i| {
                i.trashed
                    .filter(|t| t.batch == batch)
                    .map(|t| (i.id, t.from))
            })
            .map(|(id, from)| {
                let folder = if live_or_restoring(&self.data.index, from) {
                    from
                } else {
                    ROOT_FOLDER
                };
                (id, folder)
            })
            .collect();
        for (id, folder) in item_targets {
            if let Some(item) = self.data.index.item_mut(id) {
                item.trashed = None;
                item.folder = folder;
                item.updated_at = at;
                restored += 1;
            }
        }
        self.persist().await?;
        Ok(restored)
    }

    /// Delete for good everything in `batch`, on every device.
    ///
    /// # Errors
    /// Persistence errors.
    pub async fn purge_batch(&mut self, batch: u64) -> Result<()> {
        let folders: Vec<FolderId> = self
            .data
            .index
            .folders
            .iter()
            .filter(|f| f.trashed.is_some_and(|t| t.batch == batch))
            .filter(|f| {
                self.data
                    .index
                    .folder(f.parent)
                    .and_then(|p| p.trashed)
                    .is_none_or(|t| t.batch != batch)
            })
            .map(|f| f.id)
            .collect();
        for folder in folders {
            self.purge_folder(folder).await?;
        }
        let items: Vec<ItemId> = self
            .data
            .index
            .items
            .iter()
            .filter(|i| i.trashed.is_some_and(|t| t.batch == batch))
            .map(|i| i.id)
            .collect();
        for item in items {
            self.purge_item(item).await?;
        }
        Ok(())
    }

    /// Delete every trashed record for good.
    ///
    /// # Errors
    /// Persistence errors.
    pub async fn empty_trash(&mut self) -> Result<()> {
        let mut batches: Vec<u64> = self
            .data
            .index
            .items
            .iter()
            .filter_map(|i| i.trashed.map(|t| t.batch))
            .chain(
                self.data
                    .index
                    .folders
                    .iter()
                    .filter_map(|f| f.trashed.map(|t| t.batch)),
            )
            .collect();
        batches.sort_unstable();
        batches.dedup();
        for batch in batches {
            self.purge_batch(batch).await?;
        }
        Ok(())
    }

    /// Purge batches trashed more than `retention` milliseconds ago.
    ///
    /// # Errors
    /// Persistence errors.
    pub async fn purge_expired(&mut self, retention: Option<u64>) -> Result<usize> {
        let Some(retention) = retention else {
            return Ok(0);
        };
        let now = now_millis();
        let mut expired: Vec<u64> = self
            .trash_entries()
            .iter()
            .map(|e| e.trashed)
            .filter(|t| now.saturating_sub(t.at) > retention)
            .map(|t| t.batch)
            .collect();
        expired.sort_unstable();
        expired.dedup();
        let count = expired.len();
        for batch in expired {
            self.purge_batch(batch).await?;
        }
        Ok(count)
    }
}

/// Whole days left before a record trashed at `at` is purged.
#[must_use]
pub const fn days_left(at: u64, now: u64, retention: Option<u64>) -> Option<u64> {
    const DAY: u64 = 24 * 60 * 60 * 1000;
    match retention {
        Some(retention) => {
            let deadline = at.saturating_add(retention);
            Some(deadline.saturating_sub(now).div_ceil(DAY))
        }
        None => None,
    }
}

impl LibraryData {
    /// Notebooks (companion canvases) of `item` that are still live.
    #[must_use]
    pub fn live_notebooks_of(&self, item: ItemId) -> Vec<ItemId> {
        self.companions_of(item)
            .filter(|id| self.item(*id).is_some_and(|i| i.trashed.is_none()))
            .collect()
    }

    /// Top-level trash entries, most recently trashed first.
    #[must_use]
    pub fn trash_entries(&self) -> Vec<TrashEntry> {
        let index = &self.index;
        let parent_in_same_batch = |folder: FolderId, batch: u64| {
            index
                .folder(folder)
                .and_then(|f| f.trashed)
                .is_some_and(|t| t.batch == batch)
        };
        let mut entries: Vec<TrashEntry> = index
            .folders
            .iter()
            .filter_map(|f| f.trashed.map(|t| (f, t)))
            .filter(|(f, t)| !parent_in_same_batch(f.parent, t.batch))
            .map(|(f, t)| TrashEntry {
                trashed: t,
                record: TrashedRecord::Folder {
                    folder: f.clone(),
                    items: index
                        .items
                        .iter()
                        .filter(|i| i.trashed.is_some_and(|it| it.batch == t.batch))
                        .count(),
                },
            })
            .collect();
        entries.extend(
            index
                .items
                .iter()
                .filter_map(|i| i.trashed.map(|t| (i, t)))
                .filter(|(i, t)| !parent_in_same_batch(i.folder, t.batch))
                .map(|(i, t)| TrashEntry {
                    trashed: t,
                    record: TrashedRecord::Item(i.clone()),
                }),
        );
        entries.sort_by_key(|e| std::cmp::Reverse(e.trashed.at));
        entries
    }

    /// Bytes on disk held by trashed items.
    #[must_use]
    pub fn trash_size(&self) -> u64 {
        self.index
            .items
            .iter()
            .filter(|i| i.trashed.is_some())
            .map(|i| super::bodies::disk_usage(i.id))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::{days_left, retention_millis};

    const DAY: u64 = 24 * 60 * 60 * 1000;

    #[test]
    fn days_left_rounds_up() {
        let retention = retention_millis(Some(30));
        assert_eq!(days_left(0, 0, retention), Some(30));
        assert_eq!(days_left(0, DAY / 2, retention), Some(30));
        assert_eq!(days_left(0, 29 * DAY + 1, retention), Some(1));
        assert_eq!(days_left(0, 31 * DAY, retention), Some(0));
        assert_eq!(days_left(0, 31 * DAY, None), None);
    }
}
