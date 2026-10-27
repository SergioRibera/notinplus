//! Importing a bundle with the user's say on collisions: keep both,
//! replace or skip, with one rule for all and exceptions per document.

use std::collections::{HashMap, HashSet};

use super::{
    Folder, FolderId, Item, ItemId, Library, LibraryData, LinkId, ROOT_FOLDER, Result, WorkspaceId,
    bodies, now_millis, refs,
};
use crate::bundle::{BundleManifest, ParsedBundle};

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

/// Where each bundled folder lands in the library.
struct FolderMap {
    /// Bundle id → library id (new when the id is already taken).
    ids: HashMap<FolderId, FolderId>,
    destination: FolderId,
}

impl FolderMap {
    /// The library folder for a bundle `folder`; outside the bundle, the
    /// destination.
    fn resolve(&self, folder: FolderId) -> FolderId {
        self.ids.get(&folder).copied().unwrap_or(self.destination)
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
        let mut receipt = ImportReceipt {
            destination: plan.destination,
            ..ImportReceipt::default()
        };
        for tag in &manifest.tags {
            if self.data.index.tag(tag.id).is_none() {
                self.data.index.tags.push(tag.clone());
            }
        }
        let folders = self.import_folders(manifest, plan.destination, now, &mut receipt);
        let item_ids = self.import_items(bundle, review, plan, &folders, now, &mut receipt)?;
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
            copy.id = WorkspaceId::new_v4();
            copy.layout.retarget_items(&item_ids);
            copy.updated_at = now;
            receipt.workspaces.push(copy.id);
            self.data.workspaces.workspaces.push(copy);
        }
        self.persist()?;
        self.persist_workspaces()?;
        self.persist_links()?;
        Ok(receipt)
    }

    /// The bundle's folders, under `destination`; an id already in the
    /// library gets a new one.
    fn import_folders(
        &mut self,
        manifest: &BundleManifest,
        destination: FolderId,
        now: u64,
        receipt: &mut ImportReceipt,
    ) -> FolderMap {
        let ids = manifest
            .folders
            .iter()
            .map(|folder| {
                let id = if self.data.index.folder(folder.id).is_some() {
                    FolderId::new_v4()
                } else {
                    folder.id
                };
                (folder.id, id)
            })
            .collect();
        let map = FolderMap { ids, destination };
        for folder in &manifest.folders {
            let id = map.ids.get(&folder.id).copied().unwrap_or(folder.id);
            self.data.index.folders.push(Folder {
                id,
                parent: map.resolve(folder.parent),
                trashed: None,
                updated_at: now,
                ..folder.clone()
            });
            receipt.folders.push(id);
        }
        map
    }

    /// The bundle's documents and their bodies, resolving the ones
    /// already in the library as `plan` says. Returns bundle id →
    /// library id.
    fn import_items(
        &mut self,
        bundle: &ParsedBundle,
        review: &Review,
        plan: &ImportPlan,
        folders: &FolderMap,
        now: u64,
        receipt: &mut ImportReceipt,
    ) -> Result<HashMap<ItemId, ItemId>> {
        let identical: HashSet<ItemId> = review
            .conflicts
            .iter()
            .filter(|c| c.identical)
            .map(|c| c.incoming.id)
            .collect();
        let colliding: HashSet<ItemId> = review.conflicts.iter().map(|c| c.incoming.id).collect();
        let mut item_ids = HashMap::new();
        for item in &bundle.manifest.items {
            if identical.contains(&item.id) {
                continue;
            }
            let resolution = if colliding.contains(&item.id) {
                plan.resolution(item.id)
            } else {
                Resolution::KeepBoth
            };
            let id = match resolution {
                Resolution::Skip => continue,
                Resolution::Replace => {
                    self.data.index.items.retain(|i| i.id != item.id);
                    bodies::purge(item.id);
                    item.id
                }
                Resolution::KeepBoth if colliding.contains(&item.id) => ItemId::new_v4(),
                Resolution::KeepBoth => item.id,
            };
            item_ids.insert(item.id, id);
            self.data.index.items.push(Item {
                id,
                folder: folders.resolve(item.folder),
                trashed: None,
                updated_at: now,
                ..item.clone()
            });
            write_bodies(bundle, item, id)?;
            receipt.items.push(id);
        }
        Ok(item_ids)
    }
}

/// The ink, PDF and plugin content of bundled `item`, stored as `id`.
fn write_bodies(bundle: &ParsedBundle, item: &Item, id: ItemId) -> Result<()> {
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
    Ok(())
}

impl Library {
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
