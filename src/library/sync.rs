//! What the library exposes to other devices: every record as a
//! newest-wins blob, plus the tombstones that make deletions win.

use notein_sync::{BlobHead, BlobKey, BlobKind};

use super::error::Result;
use super::graveyard::{Buried, Graveyard, Tombstone};
use super::links::Link;
use super::model::{Folder, Item, Tag, Workspace};
use super::{Library, LibraryData, bodies, now_millis, refs};

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
    /// A plugin's state for an item: last writer wins.
    Snapshot {
        item: super::ItemId,
        plugin_id: String,
        updated_at: u64,
        bytes: Vec<u8>,
    },
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
        BlobKind::Link | BlobKind::Snapshot => None,
    }
}

const fn head(kind: BlobKind, id: [u8; 16], updated_at: u64) -> BlobHead {
    BlobHead {
        kind,
        id,
        updated_at,
        gone: false,
    }
}

const fn gone(kind: BlobKind, id: [u8; 16], at: u64) -> BlobHead {
    BlobHead {
        kind,
        id,
        updated_at: at,
        gone: true,
    }
}

impl Library {
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
                if item.folder != super::ROOT_FOLDER
                    && self.data.index.folder(item.folder).is_none()
                {
                    item.folder = super::ROOT_FOLDER;
                }
                super::keep_cover(item.id, item.thumbnail.as_deref());
                upsert(&mut self.data.index.items, item, |i| i.id);
                self.persist()?;
            }
            SyncRecord::Folder(folder) => {
                yard.revive(Buried::Folder, folder.id.0);
                upsert(&mut self.data.index.folders, folder, |f| f.id);
                self.persist()?;
            }
            SyncRecord::Tag(tag) => {
                yard.revive(Buried::Tag, tag.id.0);
                upsert(&mut self.data.index.tags, tag, |t| t.id);
                self.persist()?;
            }
            SyncRecord::Link(link) => {
                upsert(&mut self.data.links.links, link, |l| l.id);
                self.persist_links()?;
            }
            SyncRecord::Workspace(workspace) => {
                yard.revive(Buried::Workspace, workspace.id.0);
                upsert(&mut self.data.workspaces.workspaces, workspace, |w| w.id);
                if self.data.workspaces.active.is_none() {
                    self.data.workspaces.active =
                        self.data.workspaces.workspaces.first().map(|w| w.id);
                }
                self.persist_workspaces()?;
            }
            SyncRecord::Snapshot {
                item,
                plugin_id,
                updated_at,
                bytes,
            } => {
                if self.data.index.item(item).is_none() {
                    return Ok(false);
                }
                bodies::write_plugin_snapshot_at(item, &plugin_id, &bytes, updated_at)?;
                crate::plugins::content::record_references(item, &plugin_id, &bytes);
                crate::plugins::content::reindex(item);
                crate::plugins::content::notify_remote(item);
            }
            SyncRecord::Gone(stone) => self.apply_gone(&stone, yard).await?,
        }
        Ok(true)
    }

    /// Something was deleted on another device: delete it here too.
    async fn apply_gone(&mut self, stone: &Tombstone, yard: &mut Graveyard) -> Result<()> {
        yard.bury(stone.kind, stone.id, stone.at);
        match stone.kind {
            Buried::Item => {
                let id = super::ItemId(stone.id);
                self.data.index.items.retain(|i| i.id != id);
                bodies::purge(id);
                refs::forget(id);
                if self.data.links.retire_item(id, now_millis()) > 0 {
                    self.persist_links()?;
                }
                self.persist()?;
            }
            Buried::Folder => {
                let id = super::FolderId(stone.id);
                self.data.index.folders.retain(|f| f.id != id);
                for item in &mut self.data.index.items {
                    if item.folder == id {
                        item.folder = super::ROOT_FOLDER;
                    }
                }
                self.persist()?;
            }
            Buried::Tag => {
                let id = super::TagId(stone.id);
                self.data.index.tags.retain(|t| t.id != id);
                for item in &mut self.data.index.items {
                    item.tags.retain(|t| *t != id);
                }
                self.persist()?;
            }
            Buried::Workspace => {
                let id = super::WorkspaceId(stone.id);
                self.data.workspaces.workspaces.retain(|w| w.id != id);
                if self.data.workspaces.active == Some(id) {
                    self.data.workspaces.active =
                        self.data.workspaces.workspaces.first().map(|w| w.id);
                }
                self.persist_workspaces()?;
            }
        }
        Ok(())
    }
}

fn upsert<T, K: PartialEq>(list: &mut Vec<T>, record: T, id: impl Fn(&T) -> K) -> bool {
    let key = id(&record);
    if let Some(slot) = list.iter_mut().find(|r| id(r) == key) {
        *slot = record;
        false
    } else {
        list.push(record);
        true
    }
}

impl LibraryData {
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
        heads.extend(self.index.items.iter().filter_map(|i| {
            let at = bodies::plugin_snapshot_stamp(i.id, &i.kind.plugin_id)?;
            Some(head(BlobKind::Snapshot, i.id.0, at))
        }));
        let mut at: std::collections::HashMap<(BlobKind, [u8; 16]), usize> = heads
            .iter()
            .enumerate()
            .map(|(i, h)| ((h.kind, h.id), i))
            .collect();
        for stone in &yard.stones {
            let kind = kind_of(stone.kind);
            match at.get(&(kind, stone.id)) {
                Some(&i) if heads[i].updated_at >= stone.at => {}
                Some(&i) => heads[i] = gone(kind, stone.id, stone.at),
                None => {
                    at.insert((kind, stone.id), heads.len());
                    heads.push(gone(kind, stone.id, stone.at));
                }
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
            BlobKind::Snapshot => self.snapshot_record(super::ItemId(key.id)),
        };
        match (live, stone) {
            (Some((at, record)), Some(stone)) if at >= stone.at => Some(record),
            (Some(_) | None, Some(stone)) => Some(SyncRecord::Gone(stone)),
            (Some((_, record)), None) => Some(record),
            (None, None) => None,
        }
    }

    fn snapshot_record(&self, item: super::ItemId) -> Option<(u64, SyncRecord)> {
        let entry = self.index.item(item)?;
        let plugin_id = entry.kind.plugin_id.clone();
        let updated_at = bodies::plugin_snapshot_stamp(item, &plugin_id)?;
        let bytes = bodies::read_plugin_snapshot(item, &plugin_id).ok()??;
        Some((
            updated_at,
            SyncRecord::Snapshot {
                item,
                plugin_id,
                updated_at,
                bytes,
            },
        ))
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
            BlobKind::Snapshot => self
                .index
                .item(super::ItemId(key.id))
                .and_then(|i| bodies::plugin_snapshot_stamp(i.id, &i.kind.plugin_id)),
        };
        match (live, stone) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        }
    }
}
