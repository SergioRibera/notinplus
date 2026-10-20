//! Explicit relations between library items.
//!
//! A [`Link`] is an edge the user created: a notebook attached to a
//! document ([`LinkKind::Companion`]) or two documents marked as
//! related ([`LinkKind::Related`]). References that follow from
//! content (bookmark chips, note links) are not stored here — see
//! [`super::refs`].
//!
//! Removal is a tombstone so a later merge from another device can
//! tell "deleted" apart from "never seen".

use super::model::{ItemId, LinkId};

/// Schema version encoded into the links blob.
pub const LINKS_VERSION: u32 = 1;

/// What a link means.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LinkKind {
    /// `to` is a notebook of `from`. Directed; a document may have
    /// several notebooks and one canvas may serve several documents.
    Companion,
    /// The two items are related. Stored once, shown on both sides.
    Related,
}

/// One edge between two items.
#[istmo::message]
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Link {
    pub id: LinkId,
    pub from: ItemId,
    pub to: ItemId,
    pub kind: LinkKind,
    pub created_at: u64,
    pub updated_at: u64,
    pub deleted: bool,
}

impl Link {
    /// Whether the live edge touches `item` on either end.
    #[must_use]
    pub fn touches(&self, item: ItemId) -> bool {
        !self.deleted && (self.from == item || self.to == item)
    }

    /// The other end of the edge seen from `item`, if the edge
    /// touches it.
    #[must_use]
    pub fn other_end(&self, item: ItemId) -> Option<ItemId> {
        if self.deleted {
            return None;
        }
        if self.from == item {
            Some(self.to)
        } else if self.to == item {
            Some(self.from)
        } else {
            None
        }
    }

    fn matches(&self, from: ItemId, to: ItemId, kind: LinkKind) -> bool {
        if self.deleted || self.kind != kind {
            return false;
        }
        match kind {
            LinkKind::Companion => self.from == from && self.to == to,
            LinkKind::Related => {
                (self.from == from && self.to == to) || (self.from == to && self.to == from)
            }
        }
    }
}

/// Every link in the library, tombstones included.
#[istmo::message]
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LinkIndex {
    pub version: u32,
    pub links: Vec<Link>,
}

impl Default for LinkIndex {
    fn default() -> Self {
        Self {
            version: LINKS_VERSION,
            links: Vec::new(),
        }
    }
}

impl LinkIndex {
    /// Live links touching `item`, either direction.
    pub fn of(&self, item: ItemId) -> impl Iterator<Item = &Link> {
        self.links.iter().filter(move |l| l.touches(item))
    }

    /// Items that are notebooks of `item`.
    pub fn companions_of(&self, item: ItemId) -> impl Iterator<Item = ItemId> + '_ {
        self.links
            .iter()
            .filter(move |l| !l.deleted && l.kind == LinkKind::Companion && l.from == item)
            .map(|l| l.to)
    }

    /// Items that `item` is a notebook of.
    pub fn companion_parents_of(&self, item: ItemId) -> impl Iterator<Item = ItemId> + '_ {
        self.links
            .iter()
            .filter(move |l| !l.deleted && l.kind == LinkKind::Companion && l.to == item)
            .map(|l| l.from)
    }

    /// Live link by id.
    #[must_use]
    pub fn get(&self, id: LinkId) -> Option<&Link> {
        self.links.iter().find(|l| l.id == id && !l.deleted)
    }

    /// Add an edge, or return the id of the live edge that already
    /// says the same thing. Returns `None` when `from == to`.
    pub fn insert(&mut self, from: ItemId, to: ItemId, kind: LinkKind, now: u64) -> Option<LinkId> {
        if from == to {
            return None;
        }
        if let Some(existing) = self.links.iter().find(|l| l.matches(from, to, kind)) {
            return Some(existing.id);
        }
        let id = LinkId::new_v4();
        self.links.push(Link {
            id,
            from,
            to,
            kind,
            created_at: now,
            updated_at: now,
            deleted: false,
        });
        Some(id)
    }

    /// Tombstone the edge. Returns whether a live edge was found.
    pub fn remove(&mut self, id: LinkId, now: u64) -> bool {
        let Some(link) = self.links.iter_mut().find(|l| l.id == id && !l.deleted) else {
            return false;
        };
        link.deleted = true;
        link.updated_at = now;
        true
    }

    /// Tombstone every live edge touching `item`. Returns how many
    /// edges were affected.
    pub fn retire_item(&mut self, item: ItemId, now: u64) -> usize {
        let mut count = 0;
        for link in self.links.iter_mut().filter(|l| l.touches(item)) {
            link.deleted = true;
            link.updated_at = now;
            count += 1;
        }
        count
    }

    /// Forget tombstones older than `max_age` milliseconds. Returns how
    /// many went. A device away longer than that may bring a deleted
    /// link back, which is the accepted price of a bounded index.
    pub fn compact(&mut self, now: u64, max_age: u64) -> usize {
        let before = self.links.len();
        self.links
            .retain(|l| !l.deleted || now.saturating_sub(l.updated_at) <= max_age);
        before - self.links.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(seed: u8) -> ItemId {
        ItemId([seed; 16])
    }

    #[test]
    fn insert_dedupes_companion_by_direction() {
        let mut idx = LinkIndex::default();
        let a = idx
            .insert(item(1), item(2), LinkKind::Companion, 1)
            .unwrap();
        let b = idx
            .insert(item(1), item(2), LinkKind::Companion, 2)
            .unwrap();
        let c = idx
            .insert(item(2), item(1), LinkKind::Companion, 3)
            .unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(idx.links.len(), 2);
    }

    #[test]
    fn insert_dedupes_related_symmetrically() {
        let mut idx = LinkIndex::default();
        let a = idx.insert(item(1), item(2), LinkKind::Related, 1).unwrap();
        let b = idx.insert(item(2), item(1), LinkKind::Related, 2).unwrap();
        assert_eq!(a, b);
        assert_eq!(idx.links.len(), 1);
    }

    #[test]
    fn self_link_is_rejected() {
        let mut idx = LinkIndex::default();
        assert!(idx.insert(item(1), item(1), LinkKind::Related, 1).is_none());
        assert!(idx.links.is_empty());
    }

    #[test]
    fn removed_link_is_a_tombstone_and_can_be_recreated() {
        let mut idx = LinkIndex::default();
        let id = idx
            .insert(item(1), item(2), LinkKind::Companion, 1)
            .unwrap();
        assert!(idx.remove(id, 2));
        assert!(!idx.remove(id, 3));
        assert!(idx.get(id).is_none());
        assert_eq!(idx.of(item(1)).count(), 0);
        let again = idx
            .insert(item(1), item(2), LinkKind::Companion, 4)
            .unwrap();
        assert_ne!(again, id);
        assert_eq!(idx.links.len(), 2);
    }

    #[test]
    fn companions_and_parents_follow_direction() {
        let mut idx = LinkIndex::default();
        idx.insert(item(1), item(2), LinkKind::Companion, 1);
        idx.insert(item(3), item(2), LinkKind::Companion, 1);
        idx.insert(item(1), item(4), LinkKind::Related, 1);
        assert_eq!(
            idx.companions_of(item(1)).collect::<Vec<_>>(),
            vec![item(2)]
        );
        let mut parents: Vec<ItemId> = idx.companion_parents_of(item(2)).collect();
        parents.sort();
        assert_eq!(parents, vec![item(1), item(3)]);
        assert_eq!(idx.of(item(1)).count(), 2);
        assert_eq!(idx.of(item(4)).count(), 1);
    }

    #[test]
    fn retire_item_tombstones_every_edge_it_touches() {
        let mut idx = LinkIndex::default();
        idx.insert(item(1), item(2), LinkKind::Companion, 1);
        idx.insert(item(2), item(3), LinkKind::Related, 1);
        idx.insert(item(3), item(4), LinkKind::Related, 1);
        assert_eq!(idx.retire_item(item(2), 5), 2);
        assert_eq!(idx.of(item(1)).count(), 0);
        assert_eq!(idx.of(item(3)).count(), 1);
    }

    #[test]
    fn compact_forgets_only_old_tombstones() {
        let mut index = LinkIndex::default();
        let a = ItemId([1; 16]);
        let b = ItemId([2; 16]);
        let c = ItemId([3; 16]);
        let old = index.insert(a, b, LinkKind::Related, 10).unwrap();
        let fresh = index.insert(a, c, LinkKind::Related, 10).unwrap();
        let live = index.insert(b, c, LinkKind::Related, 10).unwrap();
        assert!(index.remove(old, 100));
        assert!(index.remove(fresh, 900));
        assert_eq!(index.compact(1000, 500), 1);
        assert!(index.links.iter().all(|l| l.id != old));
        assert!(index.links.iter().any(|l| l.id == fresh && l.deleted));
        assert!(index.get(live).is_some());
    }
}
