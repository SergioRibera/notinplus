//! Pinned documents (Fijados): a short ordered list Home shows above
//! the library.

use super::{ItemId, Library, LibraryData, LibraryError, NotFoundKind, Result};

impl Library {
    /// Pin `ids` at the end of the list (already pinned ones keep
    /// their place).
    ///
    /// # Errors
    /// [`LibraryError::NotFound`]; persistence errors.
    #[sismo::function]
    pub async fn pin_items(&mut self, ids: &[ItemId]) -> Result<()> {
        let index = &mut self.data.index;
        for id in ids {
            if !index.items.iter().any(|i| i.id == *id) {
                return Err(LibraryError::NotFound(NotFoundKind::Item(*id)));
            }
            if !index.pins.order.contains(id) {
                index.pins.order.push(*id);
                index.pins.updated_at = super::now_millis();
            }
        }
        self.persist()
    }

    /// # Errors
    /// Persistence errors.
    #[sismo::function]
    pub async fn unpin_items(&mut self, ids: &[ItemId]) -> Result<()> {
        self.data.index.pins.unpin(ids, super::now_millis());
        self.persist()
    }

    /// Move the pin at `from` to position `to` (drag to reorder).
    ///
    /// # Errors
    /// Persistence errors.
    #[sismo::function]
    pub async fn move_pin(&mut self, from: usize, to: usize) -> Result<()> {
        if from >= self.data.index.pins.order.len() {
            return Ok(());
        }
        let id = self.data.index.pins.order.remove(from);
        let to = to.min(self.data.index.pins.order.len());
        self.data.index.pins.order.insert(to, id);
        self.data.index.pins.updated_at = super::now_millis();
        self.persist()
    }
}

impl LibraryData {
    /// Pinned items in display order, skipping any that left.
    #[must_use]
    pub fn pinned(&self) -> Vec<ItemId> {
        self.index
            .pins
            .order
            .iter()
            .copied()
            .filter(|id| self.item(*id).is_some_and(|i| i.trashed.is_none()))
            .collect()
    }

    #[must_use]
    pub fn is_pinned(&self, id: ItemId) -> bool {
        self.index.pins.order.contains(&id)
    }
}
