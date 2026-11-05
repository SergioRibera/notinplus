//! Remembers what was deleted and when, so a deletion made here wins
//! over an older copy still held by another device.

use std::path::PathBuf;

use bincode::config;

use super::bodies;

/// Which kind of record a tombstone refers to.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Buried {
    Item,
    Folder,
    Tag,
    Workspace,
}

/// One deletion.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Tombstone {
    pub kind: Buried,
    pub id: [u8; 16],
    /// Unix milliseconds of the deletion.
    pub at: u64,
}

#[istmo::message]
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Graveyard {
    pub stones: Vec<Tombstone>,
}

impl Graveyard {
    fn path() -> PathBuf {
        bodies::root_dir().join("graveyard.bin")
    }

    /// Read from disk; a missing or unreadable file is an empty yard.
    /// The last yard read or saved is kept and reused while the file
    /// is unchanged.
    #[must_use]
    #[sismo::function("library::graveyard::load")]
    pub fn load() -> Self {
        let path = Self::path();
        let stamp = file_stamp(&path);
        let mut cache = cache();
        if let Some((held, yard)) = cache.as_ref()
            && *held == stamp
        {
            return yard.clone();
        }
        let yard: Self = std::fs::read(&path)
            .ok()
            .and_then(|bytes| {
                bincode::decode_from_slice(&bytes, config::standard())
                    .ok()
                    .map(|(yard, _): (Self, usize)| yard)
            })
            .unwrap_or_default();
        *cache = Some((stamp, yard.clone()));
        yard
    }

    #[sismo::function("library::graveyard::save")]
    pub fn save(&self) -> std::io::Result<()> {
        let bytes = bincode::encode_to_vec(self, config::standard())
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("bin.tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(tmp, &path)?;
        *cache() = Some((file_stamp(&path), self.clone()));
        Ok(())
    }

    /// Record a deletion. A later deletion of the same record keeps
    /// the newer time.
    pub fn bury(&mut self, kind: Buried, id: [u8; 16], at: u64) {
        match self
            .stones
            .iter_mut()
            .find(|s| s.kind == kind && s.id == id)
        {
            Some(stone) => stone.at = stone.at.max(at),
            None => self.stones.push(Tombstone { kind, id, at }),
        }
    }

    /// Drop the tombstone when a record comes back to life.
    pub fn revive(&mut self, kind: Buried, id: [u8; 16]) {
        self.stones.retain(|s| !(s.kind == kind && s.id == id));
    }

    #[must_use]
    pub fn get(&self, kind: Buried, id: [u8; 16]) -> Option<&Tombstone> {
        self.stones.iter().find(|s| s.kind == kind && s.id == id)
    }

    /// Forget stones older than `max_age` milliseconds. Returns how
    /// many went.
    #[sismo::function("library::graveyard::compact")]
    pub fn compact(&mut self, now: u64, max_age: u64) -> usize {
        let before = self.stones.len();
        self.stones.retain(|s| now.saturating_sub(s.at) <= max_age);
        before - self.stones.len()
    }
}

/// When and how big the file was, to tell a changed one.
type Stamp = Option<(std::time::SystemTime, u64)>;

fn file_stamp(path: &std::path::Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn cache() -> std::sync::MutexGuard<'static, Option<(Stamp, Graveyard)>> {
    static CACHE: std::sync::Mutex<Option<(Stamp, Graveyard)>> = std::sync::Mutex::new(None);
    CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Load, bury and save in one go; failures are logged, never fatal.
#[sismo::function("library::graveyard::bury_saved")]
pub fn bury(kind: Buried, id: [u8; 16], at: u64) {
    let mut yard = Graveyard::load();
    yard.bury(kind, id, at);
    if let Err(err) = yard.save() {
        log::error!("graveyard save: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bury_keeps_the_newest_time_and_revive_forgets() {
        let mut yard = Graveyard::default();
        yard.bury(Buried::Item, [1; 16], 10);
        yard.bury(Buried::Item, [1; 16], 5);
        assert_eq!(yard.get(Buried::Item, [1; 16]).map(|s| s.at), Some(10));
        yard.bury(Buried::Item, [1; 16], 20);
        assert_eq!(yard.get(Buried::Item, [1; 16]).map(|s| s.at), Some(20));
        assert_eq!(yard.stones.len(), 1);
        yard.revive(Buried::Item, [1; 16]);
        assert!(yard.get(Buried::Item, [1; 16]).is_none());
    }

    #[test]
    fn compact_drops_old_stones() {
        let mut yard = Graveyard::default();
        yard.bury(Buried::Item, [1; 16], 10);
        yard.bury(Buried::Tag, [2; 16], 990);
        assert_eq!(yard.compact(1000, 100), 1);
        assert!(yard.get(Buried::Item, [1; 16]).is_none());
        assert!(yard.get(Buried::Tag, [2; 16]).is_some());
    }

    #[test]
    fn kinds_do_not_collide() {
        let mut yard = Graveyard::default();
        yard.bury(Buried::Item, [1; 16], 1);
        yard.bury(Buried::Folder, [1; 16], 2);
        assert_eq!(yard.stones.len(), 2);
    }
}
