//! What the library exposes to other devices: every record as a
//! newest-wins blob, plus the tombstones that make deletions win.

use notein_sync::{BlobHead, BlobKey, BlobKind};

use super::error::Result;
use super::graveyard::{Buried, Graveyard, Tombstone};
use super::links::Link;
use super::model::{Folder, Item, Tag, Workspace};
use super::{Library, bodies, now_millis, refs};

/// One record as it travels between devices.
#[istmo::message]
#[derive(Clone, PartialEq, Debug)]
pub enum SyncRecord {
    Item(Item),
    Folder(Folder),
    Tag(Tag),
    Link(Link),
    Workspace(Workspace),
    Gone(Tombstone),
}

impl SyncRecord {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        bincode::encode_to_vec(self, bincode::config::standard()).unwrap_or_default()
    }

    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        bincode::decode_from_slice(bytes, bincode::config::standard())
            .ok()
            .map(|(record, _): (Self, usize)| record)
    }
}

const fn kind_of(buried: Buried) -> BlobKind {
    match buried {
        Buried::Item => BlobKind::Item,
        Buried::Folder => BlobKind::Folder,
        Buried::Tag => BlobKind::Tag,
        Buried::Workspace => BlobKind::Workspace,
    }
}

const fn buried_of(kind: BlobKind) -> Option<Buried> {
    match kind {
        BlobKind::Item => Some(Buried::Item),
        BlobKind::Folder => Some(Buried::Folder),
        BlobKind::Tag => Some(Buried::Tag),
        BlobKind::Workspace => Some(Buried::Workspace),
        BlobKind::Link => None,
    }
}

fn head(kind: BlobKind, id: [u8; 16], updated_at: u64) -> BlobHead {
    BlobHead {
        kind,
        id,
        updated_at,
        gone: false,
    }
}

fn gone(kind: BlobKind, id: [u8; 16], at: u64) -> BlobHead {
    BlobHead {
        kind,
        id,
        updated_at: at,
        gone: true,
    }
}

impl Library {
    /// Freshness of every record, live or buried. When a tombstone and
    /// a live record share an id the newer one speaks.
    #[must_use]
    pub fn sync_heads(&self, yard: &Graveyard) -> Vec<BlobHead> {
        let mut heads: Vec<BlobHead> = Vec::new();
        heads.extend(
            self.index
                .items
                .iter()
                .map(|i| head(BlobKind::Item, i.id.0, i.updated_at)),
        );
        heads.extend(
            self.index
                .folders
                .iter()
                .map(|f| head(BlobKind::Folder, f.id.0, f.updated_at)),
        );
        heads.extend(
            self.index
                .tags
                .iter()
                .map(|t| head(BlobKind::Tag, t.id.0, t.updated_at)),
        );
        heads.extend(
            self.links
                .links
                .iter()
                .map(|l| head(BlobKind::Link, l.id.0, l.updated_at)),
        );
        heads.extend(
            self.workspaces
                .workspaces
                .iter()
                .map(|w| head(BlobKind::Workspace, w.id.0, w.updated_at)),
        );
        for stone in &yard.stones {
            let kind = kind_of(stone.kind);
            match heads
                .iter_mut()
                .find(|h| h.kind == kind && h.id == stone.id)
            {
                Some(live) if live.updated_at >= stone.at => {}
                Some(live) => *live = gone(kind, stone.id, stone.at),
                None => heads.push(gone(kind, stone.id, stone.at)),
            }
        }
        heads
    }

    /// The record behind a head, as it should travel.
    #[must_use]
    pub fn sync_record(&self, key: BlobKey, yard: &Graveyard) -> Option<SyncRecord> {
        let stone = buried_of(key.kind).and_then(|b| yard.get(b, key.id).copied());
        let live = match key.kind {
            BlobKind::Item => self
                .index
                .items
                .iter()
                .find(|i| i.id.0 == key.id)
                .map(|i| (i.updated_at, SyncRecord::Item(i.clone()))),
            BlobKind::Folder => self
                .index
                .folders
                .iter()
                .find(|f| f.id.0 == key.id)
                .map(|f| (f.updated_at, SyncRecord::Folder(f.clone()))),
            BlobKind::Tag => self
                .index
                .tags
                .iter()
                .find(|t| t.id.0 == key.id)
                .map(|t| (t.updated_at, SyncRecord::Tag(t.clone()))),
            BlobKind::Link => self
                .links
                .links
                .iter()
                .find(|l| l.id.0 == key.id)
                .map(|l| (l.updated_at, SyncRecord::Link(l.clone()))),
            BlobKind::Workspace => self
                .workspaces
                .workspaces
                .iter()
                .find(|w| w.id.0 == key.id)
                .map(|w| (w.updated_at, SyncRecord::Workspace(w.clone()))),
        };
        match (live, stone) {
            (Some((at, record)), Some(stone)) if at >= stone.at => Some(record),
            (Some(_), Some(stone)) | (None, Some(stone)) => Some(SyncRecord::Gone(stone)),
            (Some((_, record)), None) => Some(record),
            (None, None) => None,
        }
    }

    /// Freshness of what is held locally under `key`, if anything.
    fn local_freshness(&self, key: BlobKey, yard: &Graveyard) -> Option<u64> {
        let stone = buried_of(key.kind).and_then(|b| yard.get(b, key.id).map(|s| s.at));
        let live = match key.kind {
            BlobKind::Item => self
                .index
                .items
                .iter()
                .find(|i| i.id.0 == key.id)
                .map(|i| i.updated_at),
            BlobKind::Folder => self
                .index
                .folders
                .iter()
                .find(|f| f.id.0 == key.id)
                .map(|f| f.updated_at),
            BlobKind::Tag => self
                .index
                .tags
                .iter()
                .find(|t| t.id.0 == key.id)
                .map(|t| t.updated_at),
            BlobKind::Link => self
                .links
                .links
                .iter()
                .find(|l| l.id.0 == key.id)
                .map(|l| l.updated_at),
            BlobKind::Workspace => self
                .workspaces
                .workspaces
                .iter()
                .find(|w| w.id.0 == key.id)
                .map(|w| w.updated_at),
        };
        match (live, stone) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        }
    }

    /// Keep `record` if it is newer than what is held. Returns whether
    /// anything changed. Persists what it touched.
    ///
    /// # Errors
    /// Store failure while persisting.
    pub async fn apply_sync_record(
        &mut self,
        incoming: BlobHead,
        record: SyncRecord,
        yard: &mut Graveyard,
    ) -> Result<bool> {
        let key = incoming.key();
        if self
            .local_freshness(key, yard)
            .is_some_and(|held| held >= incoming.updated_at)
        {
            return Ok(false);
        }
        match record {
            SyncRecord::Item(mut item) => {
                yard.revive(Buried::Item, item.id.0);
                if item.folder != super::ROOT_FOLDER && self.index.folder(item.folder).is_none() {
                    item.folder = super::ROOT_FOLDER;
                }
                upsert(&mut self.index.items, item, |i| i.id);
                self.persist().await?;
            }
            SyncRecord::Folder(folder) => {
                yard.revive(Buried::Folder, folder.id.0);
                upsert(&mut self.index.folders, folder, |f| f.id);
                self.persist().await?;
            }
            SyncRecord::Tag(tag) => {
                yard.revive(Buried::Tag, tag.id.0);
                upsert(&mut self.index.tags, tag, |t| t.id);
                self.persist().await?;
            }
            SyncRecord::Link(link) => {
                upsert(&mut self.links.links, link, |l| l.id);
                self.persist_links().await?;
            }
            SyncRecord::Workspace(workspace) => {
                yard.revive(Buried::Workspace, workspace.id.0);
                upsert(&mut self.workspaces.workspaces, workspace, |w| w.id);
                if self.workspaces.active.is_none() {
                    self.workspaces.active = self.workspaces.workspaces.first().map(|w| w.id);
                }
                self.persist_workspaces().await?;
            }
            SyncRecord::Gone(stone) => {
                yard.bury(stone.kind, stone.id, stone.at);
                match stone.kind {
                    Buried::Item => {
                        let id = super::ItemId(stone.id);
                        self.index.items.retain(|i| i.id != id);
                        bodies::purge(id);
                        refs::forget(id);
                        if self.links.retire_item(id, now_millis()) > 0 {
                            self.persist_links().await?;
                        }
                        self.persist().await?;
                    }
                    Buried::Folder => {
                        let id = super::FolderId(stone.id);
                        self.index.folders.retain(|f| f.id != id);
                        for item in &mut self.index.items {
                            if item.folder == id {
                                item.folder = super::ROOT_FOLDER;
                            }
                        }
                        self.persist().await?;
                    }
                    Buried::Tag => {
                        let id = super::TagId(stone.id);
                        self.index.tags.retain(|t| t.id != id);
                        for item in &mut self.index.items {
                            item.tags.retain(|t| *t != id);
                        }
                        self.persist().await?;
                    }
                    Buried::Workspace => {
                        let id = super::WorkspaceId(stone.id);
                        self.workspaces.workspaces.retain(|w| w.id != id);
                        if self.workspaces.active == Some(id) {
                            self.workspaces.active =
                                self.workspaces.workspaces.first().map(|w| w.id);
                        }
                        self.persist_workspaces().await?;
                    }
                }
            }
        }
        Ok(true)
    }
}

fn upsert<T, K: PartialEq>(list: &mut Vec<T>, record: T, id: impl Fn(&T) -> K) -> bool {
    let key = id(&record);
    match list.iter_mut().find(|r| id(r) == key) {
        Some(slot) => {
            *slot = record;
            false
        }
        None => {
            list.push(record);
            true
        }
    }
}
