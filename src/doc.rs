//! `.notinplus` document — the persisted shape of a drawing.
//!
//! Vector-only by design: every stroke is a sequence of quantised
//! [`InkPoint`]s referencing a [`BrushPreset`] in the shared registry.
//! Re-render at any zoom is lossless. Bincode 2 is the wire codec
//! (`istmo::message` prepends the derives); no serde in the tree.

use std::io;
use std::path::Path;

use bincode::config::{self, Configuration};

use crate::brush::{BrushId, BrushPreset, Stroke};

const CODEC: Configuration = config::standard();

/// A drawing. Brush presets are interned into `brushes`; strokes carry
/// only a [`BrushId`] into that table and their own colour override.
#[istmo::message]
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Doc {
    pub brushes: Vec<BrushPreset>,
    pub strokes: Vec<Stroke>,
    pub next_stroke_id: u32,
}

impl Doc {
    /// Insert `preset` if not already registered and return its
    /// [`BrushId`]. Deduping by structural equality keeps identical
    /// palette entries from bloating the registry when a doc is loaded
    /// and edited across sessions.
    /// # Panics
    /// If the registry already holds `u16::MAX` presets and the new
    /// entry would overflow. In practice a document never approaches
    /// this ceiling.
    pub fn register_brush(&mut self, preset: BrushPreset) -> BrushId {
        if let Some(idx) = self.brushes.iter().position(|b| *b == preset) {
            return BrushId(u16::try_from(idx).expect("brush registry overflow"));
        }
        let id = BrushId(u16::try_from(self.brushes.len()).expect("brush registry overflow"));
        self.brushes.push(preset);
        id
    }

    #[must_use]
    pub fn preset(&self, id: BrushId) -> Option<&BrushPreset> {
        self.brushes.get(id.0 as usize)
    }

    /// Append a fully-formed stroke, assigning it the next monotonic id.
    /// Returns the assigned id so callers can update the spatial index.
    pub fn push_stroke(
        &mut self,
        brush: BrushId,
        color: [u8; 4],
        points: Vec<crate::brush::InkPoint>,
    ) -> u32 {
        let id = self.next_stroke_id;
        self.next_stroke_id = self.next_stroke_id.wrapping_add(1);
        self.strokes.push(Stroke {
            id,
            brush,
            color,
            points,
        });
        id
    }

    /// Re-insert a pre-built stroke, respecting its `id`. Used by the
    /// eraser fragment path (fresh ids allocated by
    /// [`Self::allocate_stroke_id`]) and by undo/redo.
    pub fn insert_stroke(&mut self, stroke: Stroke) {
        self.strokes.push(stroke);
    }

    /// Reserve the next monotonic stroke id without pushing anything.
    /// Eraser splits allocate ids up-front so [`crate::spatial::SpatialIndex`]
    /// can index the fragments in the same call.
    pub const fn allocate_stroke_id(&mut self) -> u32 {
        let id = self.next_stroke_id;
        self.next_stroke_id = self.next_stroke_id.wrapping_add(1);
        id
    }

    #[must_use]
    pub fn find_stroke(&self, id: u32) -> Option<&Stroke> {
        self.strokes.iter().find(|s| s.id == id)
    }

    /// Remove the stroke with `id`. Preserves ordering of remaining
    /// strokes (paint order is z-order).
    pub fn remove_stroke(&mut self, id: u32) -> Option<Stroke> {
        let idx = self.strokes.iter().position(|s| s.id == id)?;
        Some(self.strokes.remove(idx))
    }

    pub fn clear(&mut self) {
        self.strokes.clear();
        self.next_stroke_id = 0;
    }

    /// # Errors
    /// I/O or bincode encode failures.
    pub fn save(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let bytes = bincode::encode_to_vec(self, CODEC)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, bytes)
    }

    /// # Errors
    /// I/O or bincode decode failures.
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let bytes = std::fs::read(path)?;
        let (doc, _) = bincode::decode_from_slice::<Self, _>(&bytes, CODEC)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::InkPoint;

    #[test]
    fn register_dedupes() {
        let mut d = Doc::default();
        let a = d.register_brush(BrushPreset::pen());
        let b = d.register_brush(BrushPreset::pen());
        assert_eq!(a, b);
        assert_eq!(d.brushes.len(), 1);
    }

    #[test]
    fn roundtrip() {
        let mut d = Doc::default();
        let bid = d.register_brush(BrushPreset::marker());
        let id = d.push_stroke(
            bid,
            [200, 20, 20, 255],
            vec![InkPoint::new(1.0, 2.0, 128, 32, 0)],
        );
        let bytes = bincode::encode_to_vec(&d, CODEC).unwrap();
        let (d2, _) = bincode::decode_from_slice::<Doc, _>(&bytes, CODEC).unwrap();
        assert_eq!(d, d2);
        assert!(d2.find_stroke(id).is_some());
    }
}
