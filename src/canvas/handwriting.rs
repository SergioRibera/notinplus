//! Handwriting smoothing on the [`Board`].
//!
//! While enabled, pen strokes collect into a pending letter. The letter
//! closes when the next stroke does not overlap it or the pen rests for
//! [`LETTER_IDLE`] (the UI owns that timer, see
//! [`Board::set_handwriting_waker`]). A closed letter is recognized and,
//! when it reads as a character, its strokes are replaced by an
//! [`InkGlyph`] in one undo step. Ink that reads as nothing stays.

use std::sync::OnceLock;
use std::time::Duration;

use freya_engine::prelude::{Canvas, Paint, PaintStyle};
use notein_handwriting::{Point, Recognizer};

use super::Board;
use crate::brush::BrushKind;
use crate::doc::Layer;
use crate::doc_op::DocOp;
use crate::ids::StrokeId;
use crate::ink_glyph::{GlyphFont, InkGlyph, InkGlyphId};

/// Pen rest that closes the pending letter.
pub const LETTER_IDLE: Duration = Duration::from_millis(1200);
/// Distance of the best match past which the ink is not a character.
/// Around the 99th percentile of correct matches in `glyph-bake eval`.
const REJECT_DISTANCE: f32 = 3.0;
/// Most strokes a letter takes; `UJIpenchars2` tops out at 5.
const MAX_STROKES: usize = 5;
/// Slack around the pending letter, as a fraction of its size, where a
/// new stroke still belongs to it. Vertical slack is wider so tittles
/// and accents written after the body join it.
const JOIN_SLACK_X: f32 = 0.05;
const JOIN_SLACK_Y: f32 = 0.7;

/// Candidates scanned for one that passes [`fits_ink_size`].
const CANDIDATES: usize = 8;

/// Largest ink, in pen widths, that can be `glyph`. The recognizer
/// normalizes size away, so a stem drawn bottom-up reads as `.`; only
/// the absolute size tells a dot from a stroke.
fn fits_ink_size(glyph: char, pen_widths: f32) -> bool {
    match glyph {
        '.' => pen_widths <= 3.0,
        ',' | '\'' | '`' => pen_widths <= 7.0,
        _ => true,
    }
}

/// Bundled recognizer, decoded once. `None` (logged) if the archive is
/// stale; re-run `glyph-bake templates`.
fn recognizer() -> Option<&'static Recognizer> {
    static RECOGNIZER: OnceLock<Option<Recognizer>> = OnceLock::new();
    RECOGNIZER
        .get_or_init(|| {
            Recognizer::from_bytes(include_bytes!("../../assets/handwriting/templates.rkyv"))
                .inspect_err(|err| log::error!("handwriting templates: {err}"))
                .ok()
        })
        .as_ref()
}

/// Board-side state of handwriting smoothing.
#[derive(Debug, Default)]
pub(super) struct Handwriting {
    enabled: bool,
    /// Strokes of the letter being written, in writing order.
    pending: Vec<StrokeId>,
    /// Bumped whenever `pending` changes; the idle timer only closes
    /// the letter it was armed for.
    generation: u64,
    waker: Option<flume::Sender<u64>>,
}

/// World-space bounds.
#[derive(Debug, Clone, Copy)]
struct Bounds {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

impl Bounds {
    fn of(points: impl IntoIterator<Item = (f32, f32)>) -> Option<Self> {
        points.into_iter().fold(None, |bounds, (x, y)| {
            Some(bounds.map_or(
                Self {
                    min_x: x,
                    min_y: y,
                    max_x: x,
                    max_y: y,
                },
                |b: Self| Self {
                    min_x: b.min_x.min(x),
                    min_y: b.min_y.min(y),
                    max_x: b.max_x.max(x),
                    max_y: b.max_y.max(y),
                },
            ))
        })
    }

    fn width(self) -> f32 {
        self.max_x - self.min_x
    }

    fn height(self) -> f32 {
        self.max_y - self.min_y
    }
}

/// Brushes whose strokes can be letters.
const fn writes(kind: BrushKind) -> bool {
    matches!(kind, BrushKind::Pen | BrushKind::Pencil | BrushKind::Marker)
}

impl Board {
    #[must_use]
    pub const fn handwriting_enabled(&self) -> bool {
        self.handwriting.enabled
    }

    /// Turn smoothing on or off. Turning it off closes the pending
    /// letter right away.
    pub fn set_handwriting(&mut self, enabled: bool) {
        if !enabled {
            self.close_letter();
        }
        self.handwriting.enabled = enabled;
    }

    /// Channel the idle timer listens on. Each stroke added to the
    /// pending letter sends its generation; after [`LETTER_IDLE`] with
    /// no newer one, the timer calls [`Self::flush_handwriting`].
    pub fn set_handwriting_waker(&mut self, waker: flume::Sender<u64>) {
        self.handwriting.waker = Some(waker);
    }

    /// Close the pending letter if it is still the one armed as
    /// `generation` and the pen is up.
    pub fn flush_handwriting(&mut self, generation: u64) {
        if generation == self.handwriting.generation && self.active.is_none() {
            self.close_letter();
        }
    }

    /// Track a just-committed stroke as part of the pending letter.
    /// Close the previous letter first when the stroke does not overlap it
    /// (decided on the whole stroke, so a `t` bar starting left of its stem
    /// still joins).
    pub(super) fn handwriting_after_stroke(&mut self, id: StrokeId, kind: BrushKind) {
        if !self.handwriting.enabled || !writes(kind) {
            return;
        }
        if !self.handwriting.pending.is_empty() && !self.joins_pending(id) {
            self.close_letter();
        }
        let handwriting = &mut self.handwriting;
        handwriting.pending.push(id);
        if handwriting.pending.len() > MAX_STROKES {
            // Too many strokes for a letter: this is a drawing.
            handwriting.pending.clear();
        }
        handwriting.generation += 1;
        if let Some(waker) = &handwriting.waker {
            let _ = waker.send(handwriting.generation);
        }
    }

    /// Drop the pending letter without recognizing it (undo, reload).
    pub(super) fn forget_handwriting(&mut self) {
        self.handwriting.pending.clear();
        self.handwriting.generation += 1;
    }

    /// Whether stroke `id` overlaps the pending letter grown by the join
    /// slack. The horizontal slack never drops below two pen widths so a
    /// tittle drifting off its stem still lands.
    fn joins_pending(&self, id: StrokeId) -> bool {
        let Some(stroke) = self.doc.find_stroke(id) else {
            return true;
        };
        let (Some(pending), Some(new)) = (
            self.pending_bounds(),
            Bounds::of(stroke.points.iter().map(|p| (p.x, p.y))),
        ) else {
            return true;
        };
        let pen_width = self
            .doc
            .preset(stroke.brush)
            .map_or(0.0, |preset| preset.max_width * preset.size_scale);
        let size = pending.width().max(pending.height());
        let slack_x = (JOIN_SLACK_X * size).max(2.0 * pen_width);
        let slack_y = JOIN_SLACK_Y * size;
        new.min_x <= pending.max_x + slack_x
            && new.max_x >= pending.min_x - slack_x
            && new.min_y <= pending.max_y + slack_y
            && new.max_y >= pending.min_y - slack_y
    }

    fn pending_bounds(&self) -> Option<Bounds> {
        Bounds::of(
            self.handwriting
                .pending
                .iter()
                .filter_map(|id| self.doc.find_stroke(*id))
                .flat_map(|stroke| stroke.points.iter().map(|p| (p.x, p.y))),
        )
    }

    /// Recognize the pending letter and swap its ink for a glyph.
    fn close_letter(&mut self) {
        let ids = std::mem::take(&mut self.handwriting.pending);
        self.handwriting.generation += 1;
        let Some(recognizer) = recognizer() else {
            return;
        };
        let strokes: Vec<(u32, crate::brush::Stroke)> = ids
            .iter()
            .filter_map(|id| self.doc.locate_stroke(*id))
            .collect();
        let Some(&(layer_id, ref first)) = strokes.first() else {
            return;
        };
        if strokes.iter().any(|(layer, _)| *layer != layer_id) {
            return;
        }
        let ink: Vec<Vec<Point>> = strokes
            .iter()
            .map(|(_, stroke)| stroke.points.iter().map(|p| Point::new(p.x, p.y)).collect())
            .collect();
        let Some(centerline) = Bounds::of(ink.iter().flatten().map(|p| (p.x, p.y))) else {
            return;
        };
        let pen_width = self
            .doc
            .preset(first.brush)
            .map_or(0.0, |preset| preset.max_width * preset.size_scale);
        let pen_widths = centerline.width().max(centerline.height()) / pen_width.max(f32::EPSILON);
        let Some(best) = recognizer
            .recognize(&ink, None, CANDIDATES)
            .into_iter()
            .find(|candidate| fits_ink_size(candidate.glyph, pen_widths))
            .filter(|candidate| candidate.distance <= REJECT_DISTANCE)
        else {
            return;
        };
        // The outline includes the font's stroke weight, so fit it over the
        // inked area rather than the bare centerline (a `.` has none).
        let pad = pen_width * 0.5;
        let bounds = Bounds {
            min_x: centerline.min_x - pad,
            min_y: centerline.min_y - pad,
            max_x: centerline.max_x + pad,
            max_y: centerline.max_y + pad,
        };
        let Some(glyph) = place(best.glyph, GlyphFont::selected(), bounds, first.color) else {
            return;
        };
        let mut ops: Vec<DocOp> = strokes
            .iter()
            .map(|(_, stroke)| DocOp::RemoveStroke { id: stroke.id })
            .collect();
        ops.push(DocOp::AddGlyph { layer_id, glyph });
        self.apply_ops_as_one(ops);
    }

    /// Ids of the active layer's glyphs an eraser circle touches.
    pub(super) fn glyphs_touching_circle(&self, x: f32, y: f32, radius: f32) -> Vec<InkGlyphId> {
        self.active_editable_glyphs()
            .filter(|glyph| glyph.touches_circle(x, y, radius))
            .map(|glyph| glyph.id)
            .collect()
    }

    /// Ids of the active layer's glyphs whose bounds overlap the rect.
    pub(super) fn glyphs_in_rect(
        &self,
        min_x: f32,
        min_y: f32,
        max_x: f32,
        max_y: f32,
    ) -> Vec<InkGlyphId> {
        self.active_editable_glyphs()
            .filter(|glyph| {
                glyph.bounds().is_some_and(|(x0, y0, x1, y1)| {
                    x0 <= max_x && x1 >= min_x && y0 <= max_y && y1 >= min_y
                })
            })
            .map(|glyph| glyph.id)
            .collect()
    }

    fn active_editable_glyphs(&self) -> impl Iterator<Item = &InkGlyph> {
        self.doc
            .layer(self.doc.active_layer)
            .filter(|layer| layer.visible && !layer.locked)
            .into_iter()
            .flat_map(|layer| layer.glyphs.iter())
    }

    /// Delete glyphs as part of the caller's undo transaction.
    pub(super) fn erase_glyphs(&mut self, ids: &[InkGlyphId]) {
        for id in ids {
            self.apply_op(DocOp::DeleteGlyph { id: *id });
        }
    }

    /// Keep the outline cache in step with a glyph op just applied.
    pub(super) fn cache_glyph(&mut self, glyph: &InkGlyph) {
        if let Some(path) = glyph.path() {
            self.glyph_paths.insert(glyph.id, path);
        }
    }

    /// Glyphs of `layer`, filled in their ink colour.
    pub(super) fn paint_glyphs(&self, canvas: &Canvas, layer: &Layer, opacity: f32) {
        for glyph in &layer.glyphs {
            let Some(path) = self.glyph_paths.get(&glyph.id) else {
                continue;
            };
            let [r, g, b, a] = glyph.color;
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "alpha scaled by an opacity clamped to 0..=1"
            )]
            let alpha = (f32::from(a) * opacity).round() as u8;
            let mut paint = Paint::default();
            paint.set_anti_alias(true);
            paint.set_style(PaintStyle::Fill);
            paint.set_argb(alpha, r, g, b);
            canvas.draw_path(path, &paint);
        }
    }
}

/// Font glyph for `glyph` fitted over the ink `bounds`: scaled to the
/// ink height (width for flat marks like `-`) and centred on it.
fn place(glyph: char, font: GlyphFont, bounds: Bounds, color: [u8; 4]) -> Option<InkGlyph> {
    let outline = font.ink()?.glyph(glyph)?;
    let (min_x, min_y) = (f32::from(outline.min[0]), f32::from(outline.min[1]));
    let (max_x, max_y) = (f32::from(outline.max[0]), f32::from(outline.max[1]));
    let (width, height) = (max_x - min_x, max_y - min_y);
    let scale = if height >= width * 0.5 && height > 0.0 {
        bounds.height() / height
    } else if width > 0.0 {
        bounds.width() / width
    } else {
        return None;
    };
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let center_x = (bounds.min_x + bounds.max_x) * 0.5;
    let center_y = (bounds.min_y + bounds.max_y) * 0.5;
    Some(InkGlyph {
        id: InkGlyphId::new_v4(),
        glyph,
        font,
        x: ((min_x + max_x) * 0.5).mul_add(-scale, center_x),
        y: ((min_y + max_y) * 0.5).mul_add(-scale, center_y),
        scale,
        color,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brush::InkPoint;

    /// Draw a stroke through `points` (world units).
    fn stroke(board: &mut Board, points: &[(f32, f32)]) {
        let mut points = points.iter().map(|&(x, y)| InkPoint::new(x, y, 128, 0, 0));
        board.begin(points.next().expect("non-empty stroke"));
        for point in points {
            board.extend(point);
        }
        board.end();
    }

    /// A tall straight stem, readable as `l` / `1` / `I` / `|`.
    fn stem(x: f32) -> Vec<(f32, f32)> {
        (0..=20u8)
            .map(|step| (x, f32::from(step).mul_add(4.0, 100.0)))
            .collect()
    }

    fn glyphs(board: &Board) -> usize {
        board
            .doc()
            .layers
            .iter()
            .map(|layer| layer.glyphs.len())
            .sum()
    }

    #[test]
    fn idle_letter_becomes_a_glyph_in_one_undo_step() {
        let mut board = Board::default();
        board.set_handwriting(true);
        stroke(&mut board, &stem(50.0));
        let depth = board.undo_depth();
        board.flush_handwriting(board.handwriting.generation);
        assert_eq!(glyphs(&board), 1);
        assert_eq!(board.doc().stroke_count(), 0);
        assert_eq!(board.undo_depth(), depth + 1);
        assert!(board.undo());
        assert_eq!(glyphs(&board), 0);
        assert_eq!(board.doc().stroke_count(), 1);
    }

    #[test]
    fn stale_timer_does_not_close_a_growing_letter() {
        let mut board = Board::default();
        board.set_handwriting(true);
        stroke(&mut board, &stem(50.0));
        let armed = board.handwriting.generation;
        stroke(&mut board, &[(40.0, 120.0), (60.0, 120.0)]);
        board.flush_handwriting(armed);
        assert_eq!(board.doc().stroke_count(), 2);
    }

    #[test]
    fn stroke_far_away_closes_the_previous_letter() {
        let mut board = Board::default();
        board.set_handwriting(true);
        stroke(&mut board, &stem(50.0));
        stroke(&mut board, &stem(400.0));
        assert_eq!(glyphs(&board), 1);
        assert_eq!(board.handwriting.pending.len(), 1);
    }

    #[test]
    fn disabled_smoothing_leaves_ink_alone() {
        let mut board = Board::default();
        stroke(&mut board, &stem(50.0));
        board.flush_handwriting(board.handwriting.generation);
        assert_eq!(glyphs(&board), 0);
        assert_eq!(board.doc().stroke_count(), 1);
    }

    #[test]
    fn long_stem_drawn_upwards_is_not_a_dot() {
        let mut board = Board::default();
        board.set_handwriting(true);
        let mut up = stem(50.0);
        up.reverse();
        stroke(&mut board, &up);
        board.flush_handwriting(board.handwriting.generation);
        let glyph = board
            .doc()
            .layers
            .iter()
            .flat_map(|layer| layer.glyphs.iter())
            .next();
        assert!(glyph.is_none_or(|glyph| glyph.glyph != '.'));
    }

    #[test]
    fn offset_tittle_joins_its_stem() {
        let mut board = Board::default();
        board.set_handwriting(true);
        stroke(&mut board, &stem(50.0));
        stroke(&mut board, &[(53.0, 80.0), (53.5, 80.5)]);
        assert_eq!(board.handwriting.pending.len(), 2);
    }

    #[test]
    fn next_letter_close_by_closes_the_previous_one() {
        let mut board = Board::default();
        board.set_handwriting(true);
        stroke(&mut board, &stem(50.0));
        stroke(&mut board, &stem(62.0));
        assert_eq!(glyphs(&board), 1);
        assert_eq!(board.handwriting.pending.len(), 1);
    }

    #[test]
    fn tiny_dot_stays_a_dot() {
        assert!(fits_ink_size('.', 1.0));
        assert!(!fits_ink_size('.', 20.0));
        assert!(fits_ink_size('l', 20.0));
    }

    #[test]
    fn eraser_tap_removes_a_glyph() {
        let mut board = Board::default();
        board.set_handwriting(true);
        stroke(&mut board, &stem(50.0));
        board.flush_handwriting(board.handwriting.generation);
        let ids = board.glyphs_touching_circle(50.0, 140.0, 4.0);
        assert_eq!(ids.len(), 1);
        board.erase_glyphs(&ids);
        assert_eq!(glyphs(&board), 0);
    }
}
