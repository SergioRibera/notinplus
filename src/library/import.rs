//! Importing a bundle with the user's say on collisions: keep both,
//! replace or skip, with one rule for all and exceptions per document.

use std::collections::{HashMap, HashSet};

use super::{
    Folder, FolderId, Item, ItemId, Library, LibraryData, LinkId, ROOT_FOLDER, Result, WorkspaceId,
    bodies, now_millis, refs,
};
use crate::bundle::ParsedBundle;

/// What to do with a document the library already has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Resolution {
    /// Import it as a separate copy.
    #[default]
    KeepBoth,
    /// Overwrite the one in the library.
    Replace,
    /// Leave the library's one alone.
    Skip,
}

/// A bundle document whose id the library already has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub incoming: Item,
    pub existing: Item,
    /// Same name and same content: skipped without asking.
    pub identical: bool,
}

/// What the import sheet shows before anything happens.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Review {
    pub documents: usize,
    pub mesas: usize,
    pub marks: usize,
    pub bytes: u64,
    pub conflicts: Vec<Conflict>,
    /// Kinds of the bundle that no installed plugin provides.
    pub missing_kinds: Vec<String>,
    /// Who made the copy ("Tablet de Ana").
    pub source: String,
}

impl Review {
    /// Documents that will actually be written for `plan`.
    #[must_use]
    pub fn importing(&self, plan: &ImportPlan) -> usize {
        let skipped = self
            .conflicts
            .iter()
            .filter(|c| c.identical || plan.resolution(c.incoming.id) == Resolution::Skip)
            .count();
        self.documents - skipped
    }

    /// Identical documents skipped on their own.
    #[must_use]
    pub fn identical(&self) -> usize {
        self.conflicts.iter().filter(|c| c.identical).count()
    }
}

/// The user's decisions.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ImportPlan {
    pub rule: Resolution,
    pub overrides: HashMap<ItemId, Resolution>,
    /// Where the bundle's top-level folders and documents land.
    pub destination: FolderId,
}

impl ImportPlan {
    #[must_use]
    pub fn resolution(&self, item: ItemId) -> Resolution {
        self.overrides.get(&item).copied().unwrap_or(self.rule)
    }
}

/// What an import wrote, so it can be undone as one.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ImportReceipt {
    pub items: Vec<ItemId>,
    pub folders: Vec<FolderId>,
    pub workspaces: Vec<WorkspaceId>,
    pub destination: FolderId,
}

impl LibraryData {
    /// Compare `bundle` against the library.
    #[must_use]
    pub fn review_import(&self, bundle: &ParsedBundle, installed_kinds: &[String]) -> Review {
        let manifest = &bundle.manifest;
        let conflicts = manifest
            .items
            .iter()
            .filter_map(|incoming| {
                let existing = self.item(incoming.id)?.clone();
                let same_body = bundle.docs.get(&incoming.id).map(Vec::as_slice)
                    == bodies::read_doc_bytes(incoming.id).as_deref();
                let same_content = bundle.snapshots.get(&incoming.id).cloned()
                    == bodies::read_plugin_snapshot(incoming.id, &incoming.kind.plugin_id)
                        .ok()
                        .flatten();
                let identical = existing.name == incoming.name && same_body && same_content;
                Some(Conflict {
                    incoming: incoming.clone(),
                    existing,
                    identical,
                })
            })
            .collect();
        let mut missing: Vec<String> = manifest
            .items
            .iter()
            .map(|i| i.kind.plugin_id.clone())
            .filter(|k| !installed_kinds.contains(k))
            .collect();
        missing.sort();
        missing.dedup();
        let bytes = bundle
            .docs
            .values()
            .chain(bundle.pdfs.values())
            .chain(bundle.snapshots.values())
            .map(|b| u64::try_from(b.len()).unwrap_or(0))
            .sum();
        let marks = bundle
            .docs
            .values()
            .filter_map(|b| crate::doc::Doc::from_snapshot(b).ok())
            .map(|d| d.bookmarks().len())
            .sum();
        Review {
            documents: manifest.items.len(),
            mesas: manifest.workspaces.len(),
            marks,
            bytes,
            conflicts,
            missing_kinds: missing,
            source: manifest.exported_by.clone(),
        }
    }
}

impl Library {
    /// Write `bundle` into the library following `plan`.
    ///
    /// # Errors
    /// Filesystem or persistence errors.
    pub async fn import_planned(
        &mut self,
        bundle: &ParsedBundle,
        review: &Review,
        plan: &ImportPlan,
    ) -> Result<ImportReceipt> {
        let now = now_millis();
        let manifest = &bundle.manifest;
        let identical: HashSet<ItemId> = review
            .conflicts
            .iter()
            .filter(|c| c.identical)
            .map(|c| c.incoming.id)
            .collect();
        let colliding: HashSet<ItemId> = review.conflicts.iter().map(|c| c.incoming.id).collect();
        let bundle_folders: HashSet<FolderId> = manifest.folders.iter().map(|f| f.id).collect();

        let mut receipt = ImportReceipt {
            destination: plan.destination,
            ..ImportReceipt::default()
        };

        for tag in &manifest.tags {
            if self.data.index.tag(tag.id).is_none() {
                self.data.index.tags.push(tag.clone());
            }
        }

        let mut folder_ids: HashMap<FolderId, FolderId> = HashMap::new();
        for folder in &manifest.folders {
            let id = if self.data.index.folder(folder.id).is_some() {
                self.data.index.alloc_folder_id()
            } else {
                folder.id
            };
            folder_ids.insert(folder.id, id);
        }
        for folder in &manifest.folders {
            let parent = if bundle_folders.contains(&folder.parent) {
                folder_ids
                    .get(&folder.parent)
                    .copied()
                    .unwrap_or(plan.destination)
            } else {
                plan.destination
            };
            let id = folder_ids.get(&folder.id).copied().unwrap_or(folder.id);
            self.data.index.folders.push(Folder {
                id,
                parent,
                trashed: None,
                updated_at: now,
                ..folder.clone()
            });
            receipt.folders.push(id);
        }

        let mut item_ids: HashMap<ItemId, ItemId> = HashMap::new();
        for item in &manifest.items {
            if identical.contains(&item.id) {
                continue;
            }
            let resolution = if colliding.contains(&item.id) {
                plan.resolution(item.id)
            } else {
                Resolution::KeepBoth
            };
            let folder = if bundle_folders.contains(&item.folder) {
                folder_ids
                    .get(&item.folder)
                    .copied()
                    .unwrap_or(plan.destination)
            } else {
                plan.destination
            };
            let id = match resolution {
                Resolution::Skip => continue,
                Resolution::Replace => {
                    self.data.index.items.retain(|i| i.id != item.id);
                    bodies::purge(item.id);
                    item.id
                }
                Resolution::KeepBoth if colliding.contains(&item.id) => {
                    self.data.index.alloc_item_id()
                }
                Resolution::KeepBoth => item.id,
            };
            item_ids.insert(item.id, id);
            self.data.index.items.push(Item {
                id,
                folder,
                trashed: None,
                updated_at: now,
                ..item.clone()
            });
            if let Some(bytes) = bundle.docs.get(&item.id) {
                bodies::write_doc_bytes(id, bytes)?;
                if let Ok(doc) = bodies::read_doc(id) {
                    refs::record_doc(id, &doc);
                }
            }
            if let Some(bytes) = bundle.pdfs.get(&item.id) {
                bodies::write_pdf(id, bytes)?;
            }
            if let Some(bytes) = bundle.snapshots.get(&item.id) {
                bodies::write_plugin_snapshot(id, &item.kind.plugin_id, bytes)?;
            }
            receipt.items.push(id);
        }

        for incoming in &manifest.links {
            let (Some(from), Some(to)) = (item_ids.get(&incoming.from), item_ids.get(&incoming.to))
            else {
                continue;
            };
            let mut link = incoming.clone();
            link.id = LinkId::new_v4();
            link.from = *from;
            link.to = *to;
            link.updated_at = now;
            self.data.links.links.push(link);
        }

        for workspace in &manifest.workspaces {
            let mut copy = workspace.clone();
            copy.id = self.data.workspaces.alloc_workspace_id();
            copy.layout.retarget_items(&item_ids);
            copy.updated_at = now;
            receipt.workspaces.push(copy.id);
            self.data.workspaces.workspaces.push(copy);
        }

        self.persist().await?;
        self.persist_workspaces().await?;
        self.persist_links().await?;
        Ok(receipt)
    }

    /// Undo an import: everything it wrote leaves for good.
    ///
    /// # Errors
    /// Persistence errors.
    pub async fn undo_import(&mut self, receipt: &ImportReceipt) -> Result<()> {
        for item in &receipt.items {
            if self.data.index.item(*item).is_some() {
                self.purge_item(*item).await?;
            }
        }
        for folder in receipt.folders.iter().rev() {
            if self.data.index.folder(*folder).is_some() {
                self.purge_folder(*folder).await?;
            }
        }
        for workspace in &receipt.workspaces {
            if self.data.workspaces.workspace(*workspace).is_some() {
                self.delete_workspace(*workspace).await?;
            }
        }
        if receipt.destination != ROOT_FOLDER
            && self.data.index.folder(receipt.destination).is_some()
            && self.items_of(receipt.destination).is_empty()
            && self.folders_of(receipt.destination).is_empty()
        {
            self.purge_folder(receipt.destination).await?;
        }
        Ok(())
    }
}
