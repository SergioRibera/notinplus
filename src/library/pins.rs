//! Pinned documents (Fijados): a short ordered list Home shows above
//! the library.

use super::{ItemId, Library, LibraryError, NotFoundKind, Result};

impl Library {
    /// Pinned items in display order, skipping any that left.
    #[must_use]
    pub fn pinned(&self) -> Vec<ItemId> {
        self.index
            .pins
            .iter()
            .copied()
            .filter(|id| self.item(*id).is_some_and(|i| i.trashed.is_none()))
            .collect()
    }

    #[must_use]
    pub fn is_pinned(&self, id: ItemId) -> bool {
        self.index.pins.contains(&id)
    }

    /// Pin `ids` at the end of the list (already pinned ones keep
    /// their place).
    ///
    /// # Errors
    /// [`LibraryError::NotFound`]; persistence errors.
    pub async fn pin_items(&mut self, ids: &[ItemId]) -> Result<()> {
        for id in ids {
            if self.item(*id).is_none() {
                return Err(LibraryError::NotFound(NotFoundKind::Item(*id)));
            }
            if !self.index.pins.contains(id) {
                self.index.pins.push(*id);
            }
        }
        self.persist().await
    }

    /// # Errors
    /// Persistence errors.
    pub async fn unpin_items(&mut self, ids: &[ItemId]) -> Result<()> {
        self.index.pins.retain(|p| !ids.contains(p));
        self.persist().await
    }

    /// Move the pin at `from` to position `to` (drag to reorder).
    ///
    /// # Errors
    /// Persistence errors.
    pub async fn move_pin(&mut self, from: usize, to: usize) -> Result<()> {
        if from >= self.index.pins.len() {
            return Ok(());
        }
        let id = self.index.pins.remove(from);
        let to = to.min(self.index.pins.len());
        self.index.pins.insert(to, id);
        self.persist().await
    }
}
