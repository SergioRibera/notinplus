//! Lasso: circle ink, text boxes and images to select them, then move,
//! recolor, duplicate, delete them or send them to another layer.

use freya_engine::prelude::{Canvas, Color as SkColor, Paint, PaintStyle, PathBuilder, PathEffect};

use notin_beautify::{Context, Outcome, Pt, StrokeShape, beautify_all};

use super::{Board, Step};
use crate::brush::Stroke;
use crate::doc_op::DocOp;
use crate::ids::StrokeId;
use crate::image_box::ImageBoxId;
use crate::text_box::{TextBoxId, TextFrame};

/// Share of a stroke's points that must fall inside the loop.
const INSIDE_SHARE: f32 = 0.6;
/// Screen pixels around the selection that still grab it.
const GRAB_SLACK: f32 = 12.0;
/// World offset of a duplicate.
const DUPLICATE_OFFSET: f32 = 24.0;
/// World distance from a text box's left edge to where its lines start.
const TEXT_START: f32 = 16.0;

/// What the lasso holds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub strokes: Vec<StrokeId>,
    pub texts: Vec<TextBoxId>,
    pub images: Vec<ImageBoxId>,
    /// World bounds `(min_x, min_y, max_x, max_y)`.
    pub bounds: (f32, f32, f32, f32),
    /// World offset of a move in progress.
    offset: (f32, f32),
    grab: Option<(f32, f32)>,
}

impl Selection {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.strokes.is_empty() && self.texts.is_empty() && self.images.is_empty()
    }

    /// World offset the selection is dragged by, while it is.
    const fn drag(&self) -> Option<(f32, f32)> {
        match self.grab {
            Some(_) => Some(self.offset),
            None => None,
        }
    }

    /// Current world offset of selected ink being dragged.
    #[must_use]
    pub const fn offset(&self) -> (f32, f32) {
        self.offset
    }
}

/// A selected stroke as beautifying reads it.
struct SelectedInk {
    id: StrokeId,
    points: Vec<Pt>,
    /// Points with their pressure.
    ink: Vec<(Pt, u8)>,
}

/// Lasso state on the board: the loop being drawn and what it caught.
#[derive(Debug, Default)]
pub(super) struct LassoState {
    path: Vec<(f32, f32)>,
    pub(super) selection: Option<Selection>,
    /// Theme accent the loop and frame are drawn in, once the host set it.
    accent: Option<SkColor>,
}

/// Even-odd point in polygon.
fn inside(polygon: &[(f32, f32)], (x, y): (f32, f32)) -> bool {
    let mut odd = false;
    let mut j = polygon.len().wrapping_sub(1);
    for (i, &(xi, yi)) in polygon.iter().enumerate() {
        let (xj, yj) = polygon[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            odd = !odd;
        }
        j = i;
    }
    odd
}

fn union(a: Option<(f32, f32, f32, f32)>, b: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    a.map_or(b, |a| {
        (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
    })
}

impl Board {
    /// The current lasso selection, if any.
    #[must_use]
    pub const fn selection(&self) -> Option<&Selection> {
        self.lasso.selection.as_ref()
    }

    /// Selection bounds in surface pixels, for placing its action bar.
    #[must_use]
    #[sismo::function("lasso::selection_screen_bounds")]
    pub fn selection_screen_bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let s = self.lasso.selection.as_ref()?;
        let (x0, y0) = self.world_to_screen(s.bounds.0 + s.offset.0, s.bounds.1 + s.offset.1);
        let (x1, y1) = self.world_to_screen(s.bounds.2 + s.offset.0, s.bounds.3 + s.offset.1);
        Some((x0, y0, x1, y1))
    }

    #[sismo::function("lasso::clear_selection")]
    pub fn clear_selection(&mut self) {
        if self.lasso.selection.take().is_some() || !self.lasso.path.is_empty() {
            self.lasso.path.clear();
            self.notify();
        }
    }

    /// Whether surface point `(sx, sy)` grabs the selection.
    #[must_use]
    #[sismo::function("lasso::selection_hit")]
    pub fn selection_hit(&self, sx: f32, sy: f32) -> bool {
        self.selection_screen_bounds()
            .is_some_and(|(x0, y0, x1, y1)| {
                sx >= x0 - GRAB_SLACK
                    && sx <= x1 + GRAB_SLACK
                    && sy >= y0 - GRAB_SLACK
                    && sy <= y1 + GRAB_SLACK
            })
    }

    /// Start a lasso stroke at surface point `(sx, sy)`, or grab the
    /// selection when the point is on it.
    #[sismo::function("lasso::down")]
    pub fn lasso_down(&mut self, sx: f32, sy: f32) {
        if self.selection_hit(sx, sy) {
            let world = self.screen_to_world(sx, sy);
            if let Some(selection) = self.lasso.selection.as_mut() {
                selection.grab = Some(world);
            }
            return;
        }
        self.lasso.selection = None;
        self.lasso.path = vec![self.screen_to_world(sx, sy)];
        self.notify();
    }

    #[sismo::function("lasso::move")]
    pub fn lasso_move(&mut self, sx: f32, sy: f32) {
        let world = self.screen_to_world(sx, sy);
        if let Some(selection) = self.lasso.selection.as_mut()
            && let Some(grab) = selection.grab
        {
            selection.offset = (world.0 - grab.0, world.1 - grab.1);
            if !selection.texts.is_empty() {
                // Text boxes are laid out by the UI from the scene.
                self.touch_text_scene();
            }
            self.notify();
            return;
        }
        if !self.lasso.path.is_empty() {
            self.lasso.path.push(world);
            self.notify();
        }
    }

    /// Finish the lasso: close the loop and select what it caught, or
    /// drop the selection where a drag left it.
    #[sismo::function("lasso::up")]
    pub fn lasso_up(&mut self) {
        if let Some(selection) = self.lasso.selection.as_mut()
            && selection.grab.take().is_some()
        {
            let offset = std::mem::take(&mut selection.offset);
            if offset.0.abs() > f32::EPSILON || offset.1.abs() > f32::EPSILON {
                self.translate_selection(offset);
            }
            self.notify();
            return;
        }
        let path = std::mem::take(&mut self.lasso.path);
        self.lasso.selection = (path.len() >= 3)
            .then(|| self.select_in(&path))
            .filter(|s| !s.is_empty());
        self.notify();
    }

    #[sismo::function("lasso::cancel")]
    pub fn lasso_cancel(&mut self) {
        self.lasso.path.clear();
        if let Some(selection) = self.lasso.selection.as_mut() {
            selection.grab = None;
            selection.offset = (0.0, 0.0);
        }
        self.notify();
    }

    #[sismo::function("lasso::select_in")]
    fn select_in(&self, polygon: &[(f32, f32)]) -> Selection {
        let mut selection = Selection::default();
        let mut bounds = None;
        for layer in self.doc.layers.iter().filter(|l| l.visible && !l.locked) {
            for stroke in &layer.strokes {
                let points = stroke.display_points();
                #[allow(clippy::cast_precision_loss)] // point counts
                let total = points.len().max(1) as f32;
                #[allow(clippy::cast_precision_loss)]
                let hits = points
                    .iter()
                    .filter(|p| inside(polygon, (p.x, p.y)))
                    .count() as f32;
                if hits / total >= INSIDE_SHARE {
                    selection.strokes.push(stroke.id);
                    for p in points.iter() {
                        bounds = Some(union(bounds, (p.x, p.y, p.x, p.y)));
                    }
                }
            }
            // A box is caught when the loop goes around its centre or the
            // start of its lines: short text leaves the centre of a wide box
            // blank.
            for text in &layer.texts {
                let height = self.text_height(text.id);
                let start = TEXT_START.min(text.frame.width * 0.5);
                let line_start = text.frame.to_world(height, start, height * 0.5);
                if inside(polygon, text.frame.center(height)) || inside(polygon, line_start) {
                    selection.texts.push(text.id);
                    bounds = Some(union(bounds, text.frame.bounds(height)));
                }
            }
            for image in &layer.images {
                if inside(polygon, image.frame.center(image.height)) {
                    selection.images.push(image.id);
                    bounds = Some(union(bounds, image.frame.bounds(image.height)));
                }
            }
        }
        selection.bounds = bounds.unwrap_or_default();
        selection
    }

    #[sismo::function("lasso::translate_selection")]
    fn translate_selection(&mut self, (dx, dy): (f32, f32)) {
        let Some(selection) = self.lasso.selection.clone() else {
            return;
        };
        let moved = |frame: TextFrame| TextFrame {
            x: frame.x + dx,
            y: frame.y + dy,
            ..frame
        };
        let mut ops: Vec<DocOp> = selection
            .strokes
            .iter()
            .map(|id| DocOp::MoveStroke { id: *id, dx, dy })
            .collect();
        ops.extend(selection.texts.iter().filter_map(|id| {
            let text = self.doc.text(*id)?;
            Some(DocOp::SetTextFrame {
                id: *id,
                frame: moved(text.frame),
            })
        }));
        ops.extend(selection.images.iter().filter_map(|id| {
            let image = self.doc.image(*id)?;
            Some(DocOp::SetImageFrame {
                id: *id,
                frame: moved(image.frame),
                height: image.height,
            })
        }));
        self.apply_ops_as_one(ops);
        if let Some(s) = self.lasso.selection.as_mut() {
            s.bounds = (
                s.bounds.0 + dx,
                s.bounds.1 + dy,
                s.bounds.2 + dx,
                s.bounds.3 + dy,
            );
        }
    }

    /// Paint the selected ink in `color`.
    #[sismo::function("lasso::recolor")]
    pub fn selection_recolor(&mut self, color: [u8; 4]) {
        let Some(selection) = self.lasso.selection.clone() else {
            return;
        };
        let mut ops = Vec::new();
        for id in &selection.strokes {
            if let Some((layer_id, mut stroke)) = self.doc.locate_stroke(*id) {
                stroke.color = color;
                ops.push(DocOp::RemoveStroke { id: *id });
                ops.push(DocOp::PushStroke { layer_id, stroke });
            }
        }
        ops.extend(selection.texts.iter().filter_map(|id| {
            let text = self.doc.text(*id)?;
            Some(DocOp::UpdateText {
                id: *id,
                markdown: text.markdown.clone(),
                color,
            })
        }));
        self.apply_ops_as_one(ops);
    }

    /// Copy the selected ink a little down and right; the copy becomes
    /// the selection.
    #[sismo::function("lasso::duplicate")]
    pub fn selection_duplicate(&mut self) {
        let Some(selection) = self.lasso.selection.clone() else {
            return;
        };
        let d = DUPLICATE_OFFSET;
        let mut ops = Vec::new();
        let mut copy = Selection {
            bounds: (
                selection.bounds.0 + d,
                selection.bounds.1 + d,
                selection.bounds.2 + d,
                selection.bounds.3 + d,
            ),
            ..Selection::default()
        };
        for id in &selection.strokes {
            if let Some((layer_id, stroke)) = self.doc.locate_stroke(*id) {
                let mut stroke = stroke.translated(d, d);
                stroke.id = StrokeId::new_v4();
                copy.strokes.push(stroke.id);
                ops.push(DocOp::PushStroke { layer_id, stroke });
            }
        }
        let shifted = |frame: TextFrame| TextFrame {
            x: frame.x + d,
            y: frame.y + d,
            ..frame
        };
        for id in &selection.texts {
            if let Some((layer_id, text)) = self.doc.locate_text(*id) {
                let mut text = text.clone();
                text.id = TextBoxId::new_v4();
                text.frame = shifted(text.frame);
                if let Some(height) = self.text_heights.get(id).copied() {
                    self.text_heights.insert(text.id, height);
                }
                copy.texts.push(text.id);
                ops.push(DocOp::AddText { layer_id, text });
            }
        }
        for id in &selection.images {
            if let Some((layer_id, image)) = self.doc.locate_image(*id) {
                let mut image = image.clone();
                image.id = ImageBoxId::new_v4();
                image.frame = shifted(image.frame);
                copy.images.push(image.id);
                ops.push(DocOp::AddImage { layer_id, image });
            }
        }
        self.apply_ops_as_one(ops);
        self.lasso.selection = Some(copy);
    }

    /// Delete the selection.
    #[sismo::function("lasso::delete")]
    pub fn selection_delete(&mut self) {
        let Some(selection) = self.lasso.selection.take() else {
            return;
        };
        let ops = selection
            .strokes
            .iter()
            .map(|id| DocOp::RemoveStroke { id: *id })
            .chain(
                selection
                    .texts
                    .iter()
                    .map(|id| DocOp::DeleteText { id: *id }),
            )
            .chain(
                selection
                    .images
                    .iter()
                    .map(|id| DocOp::DeleteImage { id: *id }),
            )
            .collect();
        self.apply_ops_as_one(ops);
    }

    /// Layer every selected item can go to, one `step` away from its own:
    /// the nearest visible, unlocked layer that way.
    fn layer_for_step(&self, from: u32, step: Step) -> Option<u32> {
        let mut at = from;
        loop {
            at = self.layer_beside(at, step)?;
            let layer = self.doc.layer(at)?;
            if layer.visible && !layer.locked {
                return Some(at);
            }
        }
    }

    /// Send each selected item to the next layer `step` from its own
    /// (skipping hidden and locked ones), as one undo step. It lands on top
    /// of what that layer already holds. Items with nowhere to go stay.
    /// `false` when nothing moved.
    #[sismo::function("lasso::step_layer")]
    pub fn selection_step_layer(&mut self, step: Step) -> bool {
        let Some(selection) = self.lasso.selection.clone() else {
            return false;
        };
        let mut ops = Vec::new();
        for id in &selection.strokes {
            if let Some((from, stroke)) = self.doc.locate_stroke(*id)
                && let Some(layer_id) = self.layer_for_step(from, step)
            {
                ops.push(DocOp::RemoveStroke { id: *id });
                ops.push(DocOp::PushStroke { layer_id, stroke });
            }
        }
        for id in &selection.texts {
            if let Some((from, text)) = self.doc.locate_text(*id)
                && let Some(layer_id) = self.layer_for_step(from, step)
            {
                let text = text.clone();
                ops.push(DocOp::DeleteText { id: *id });
                ops.push(DocOp::AddText { layer_id, text });
            }
        }
        for id in &selection.images {
            if let Some((from, image)) = self.doc.locate_image(*id)
                && let Some(layer_id) = self.layer_for_step(from, step)
            {
                let image = image.clone();
                ops.push(DocOp::DeleteImage { id: *id });
                ops.push(DocOp::AddImage { layer_id, image });
            }
        }
        if ops.is_empty() {
            return false;
        }
        self.apply_ops_as_one(ops);
        self.notify();
        true
    }

    /// Beautify the selected ink: handwriting is corrected word by word
    /// on its lines, loose strokes that pass as figures snap. Works on any
    /// ink (old, received, written with the switch off) and recomputes
    /// ink already beautified. One undo step.
    #[sismo::function("lasso::beautify")]
    pub fn selection_beautify(&mut self) {
        let Some(selection) = self.lasso.selection.clone() else {
            return;
        };
        let strokes: Vec<SelectedInk> = selection
            .strokes
            .iter()
            .filter_map(|id| self.doc.find_stroke(*id))
            .filter(|s| {
                self.doc
                    .preset(s.brush)
                    .is_some_and(|p| super::beautify::writes(p.kind))
            })
            .map(|s| {
                let ink: Vec<(Pt, u8)> = s
                    .points
                    .iter()
                    .map(|p| (Pt::new(p.x, p.y), p.pressure))
                    .collect();
                SelectedInk {
                    id: s.id,
                    points: ink.iter().map(|(p, _)| *p).collect(),
                    ink,
                }
            })
            .collect();
        let refs: Vec<&[Pt]> = strokes.iter().map(|s| s.points.as_slice()).collect();
        let background = std::sync::Arc::clone(&self.background);
        let rules = move |p: Pt| background.rule_spacing_at(p.x, p.y);
        let context = Context {
            slant: self.style_profile().slant(),
            rules: Some(&rules),
        };
        let mut ops = Vec::new();
        for outcome in beautify_all(&refs, &context) {
            match outcome {
                Outcome::Word {
                    strokes: members,
                    fix,
                } => {
                    for i in members {
                        let id = strokes[i].id;
                        ops.push(DocOp::SetStrokeShape { id, shape: None });
                        ops.push(DocOp::SetStrokeFix { id, fix: Some(fix) });
                    }
                }
                Outcome::Shape { stroke, geom } => {
                    let SelectedInk { id, ink, .. } = &strokes[stroke];
                    ops.push(DocOp::SetStrokeFix { id: *id, fix: None });
                    ops.push(DocOp::SetStrokeShape {
                        id: *id,
                        shape: Some(StrokeShape::from_ink(geom, ink)),
                    });
                }
            }
        }
        // Skip what would not change, so a second press is a no-op.
        ops.retain(|op| match op {
            DocOp::SetStrokeFix { id, fix } => {
                self.doc.find_stroke(*id).is_some_and(|s| s.fix != *fix)
            }
            DocOp::SetStrokeShape { id, shape } => {
                self.doc.find_stroke(*id).is_some_and(|s| s.shape != *shape)
            }
            _ => true,
        });
        self.apply_ops_as_one(ops);
        self.refresh_selection_bounds();
    }

    /// Bring the selected ink back to how it was drawn.
    #[sismo::function("lasso::original")]
    pub fn selection_original(&mut self) {
        let Some(selection) = self.lasso.selection.clone() else {
            return;
        };
        let mut ops = Vec::new();
        for stroke in selection
            .strokes
            .iter()
            .filter_map(|id| self.doc.find_stroke(*id))
        {
            if stroke.fix.is_some() {
                ops.push(DocOp::SetStrokeFix {
                    id: stroke.id,
                    fix: None,
                });
            }
            if stroke.shape.is_some() {
                ops.push(DocOp::SetStrokeShape {
                    id: stroke.id,
                    shape: None,
                });
            }
        }
        self.apply_ops_as_one(ops);
        self.refresh_selection_bounds();
    }

    /// Whether any selected stroke shows a correction.
    #[must_use]
    pub fn selection_is_corrected(&self) -> bool {
        self.lasso.selection.as_ref().is_some_and(|s| {
            s.strokes
                .iter()
                .filter_map(|id| self.doc.find_stroke(*id))
                .any(Stroke::is_corrected)
        })
    }

    /// Selection frame around the selected ink as it is now shown.
    fn refresh_selection_bounds(&mut self) {
        let Some(selection) = self.lasso.selection.as_ref() else {
            return;
        };
        let ink = selection
            .strokes
            .iter()
            .filter_map(|id| self.doc.find_stroke(*id))
            .flat_map(|s| s.display_points().into_owned())
            .map(|p| (p.x, p.y, p.x, p.y));
        let texts = selection.texts.iter().filter_map(|id| {
            let text = self.doc.text(*id)?;
            Some(text.frame.bounds(self.text_height(*id)))
        });
        let images = selection.images.iter().filter_map(|id| {
            let image = self.doc.image(*id)?;
            Some(image.frame.bounds(image.height))
        });
        let bounds = ink
            .chain(texts)
            .chain(images)
            .fold(None, |acc, b| Some(union(acc, b)));
        if let (Some(bounds), Some(selection)) = (bounds, self.lasso.selection.as_mut()) {
            selection.bounds = bounds;
        }
        self.notify();
    }

    /// Whether a selection is being dragged.
    pub(super) fn lasso_dragging(&self) -> bool {
        self.lasso
            .selection
            .as_ref()
            .is_some_and(|s| s.grab.is_some())
    }

    /// Whether stroke `id` is being dragged, and by how much.
    pub(super) fn drag_offset_of_stroke(&self, id: StrokeId) -> Option<(f32, f32)> {
        self.lasso
            .selection
            .as_ref()
            .filter(|s| s.strokes.contains(&id))
            .and_then(Selection::drag)
    }

    /// Whether text box `id` is being dragged, and by how much.
    pub(super) fn drag_offset_of_text(&self, id: TextBoxId) -> Option<(f32, f32)> {
        self.lasso
            .selection
            .as_ref()
            .filter(|s| s.texts.contains(&id))
            .and_then(Selection::drag)
    }

    /// Whether image `id` is being dragged, and by how much.
    pub(super) fn drag_offset_of_image(&self, id: ImageBoxId) -> Option<(f32, f32)> {
        self.lasso
            .selection
            .as_ref()
            .filter(|s| s.images.contains(&id))
            .and_then(Selection::drag)
    }

    /// Draw the lasso in the theme accent `[r, g, b]`.
    pub const fn set_lasso_accent(&mut self, [r, g, b]: [u8; 3]) {
        self.lasso.accent = Some(SkColor::from_rgb(r, g, b));
    }

    /// The loop being drawn and the selection frame, in world space.
    #[sismo::function("lasso::paint")]
    pub(super) fn paint_lasso(&self, canvas: &Canvas) {
        let scale = self.viewport.scale.max(f32::EPSILON);
        let accent = self
            .lasso
            .accent
            .unwrap_or_else(|| SkColor::from_rgb(0x7B, 0x8C, 0xFF));
        let mut line = Paint::default();
        line.set_anti_alias(true);
        line.set_style(PaintStyle::Stroke);
        line.set_stroke_width(1.5 / scale);
        line.set_color(accent.with_a(230));
        line.set_path_effect(PathEffect::dash(&[6.0 / scale, 4.0 / scale], 0.0));
        if self.lasso.path.len() > 1 {
            let mut builder = PathBuilder::new();
            let (x, y) = self.lasso.path[0];
            builder.move_to((x, y));
            for &(x, y) in &self.lasso.path[1..] {
                builder.line_to((x, y));
            }
            canvas.draw_path(&builder.detach(), &line);
        }
        if let Some(s) = &self.lasso.selection {
            let pad = 6.0 / scale;
            let (x0, y0, x1, y1) = (
                s.bounds.0 + s.offset.0 - pad,
                s.bounds.1 + s.offset.1 - pad,
                s.bounds.2 + s.offset.0 + pad,
                s.bounds.3 + s.offset.1 + pad,
            );
            let mut fill = Paint::default();
            fill.set_color(accent.with_a(24));
            canvas.draw_rect(freya_engine::prelude::Rect::new(x0, y0, x1, y1), &fill);
            canvas.draw_rect(freya_engine::prelude::Rect::new(x0, y0, x1, y1), &line);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::inside;

    #[test]
    fn point_in_square() {
        let square = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!(inside(&square, (5.0, 5.0)));
        assert!(!inside(&square, (15.0, 5.0)));
        assert!(!inside(&square, (-1.0, -1.0)));
    }
}

#[cfg(test)]
mod board_tests {
    use crate::brush::InkPoint;
    use crate::canvas::{Board, InputMode};

    fn draw(board: &mut Board, from: (f32, f32), to: (f32, f32)) {
        board.begin_screen(InkPoint::new(from.0, from.1, 128, 0, 0));
        board.extend_screen(InkPoint::new(
            f32::midpoint(from.0, to.0),
            f32::midpoint(from.1, to.1),
            128,
            0,
            8_000,
        ));
        board.extend_screen(InkPoint::new(to.0, to.1, 128, 0, 8_000));
        board.end();
    }

    fn lasso(board: &mut Board, points: &[(f32, f32)]) {
        board.lasso_down(points[0].0, points[0].1);
        for p in &points[1..] {
            board.lasso_move(p.0, p.1);
        }
        board.lasso_up();
    }

    const SQUARE: [(f32, f32); 5] = [
        (0.0, 0.0),
        (100.0, 0.0),
        (100.0, 100.0),
        (0.0, 100.0),
        (0.0, 0.0),
    ];

    #[test]
    fn lasso_selects_moves_duplicates_and_deletes() {
        let mut board = Board::default();
        draw(&mut board, (20.0, 20.0), (60.0, 60.0));
        draw(&mut board, (300.0, 300.0), (340.0, 340.0));
        board.set_input_mode(InputMode::Lasso);
        lasso(&mut board, &SQUARE);
        let selected = board.selection().map(|s| s.strokes.len());
        assert_eq!(selected, Some(1), "only the stroke inside the loop");

        board.lasso_down(40.0, 40.0);
        board.lasso_move(140.0, 40.0);
        board.lasso_up();
        let moved = board
            .doc()
            .strokes()
            .map(|s| s.points[0].x)
            .fold(f32::MAX, f32::min);
        assert!(
            (moved - 120.0).abs() < 0.5,
            "moved by 100 world units, got {moved}"
        );
        assert!(board.undo(), "the move is one undo step");
        let back = board
            .doc()
            .strokes()
            .map(|s| s.points[0].x)
            .fold(f32::MAX, f32::min);
        assert!((back - 20.0).abs() < 0.5);

        board.selection_duplicate();
        assert_eq!(board.doc().stroke_count(), 3);
        board.selection_delete();
        assert_eq!(board.doc().stroke_count(), 2);
    }

    fn circle(board: &mut Board, (cx, cy): (f32, f32), r: f32) {
        let at = |i: u8| {
            let t = std::f32::consts::TAU * f32::from(i) / 60.0;
            InkPoint::new(r.mul_add(t.cos(), cx), r.mul_add(t.sin(), cy), 128, 0, 0)
        };
        board.begin_screen(at(0));
        for i in 1..=60 {
            board.extend_screen(at(i));
        }
        board.end();
    }

    #[test]
    fn beautify_and_original_on_a_selection() {
        let mut board = Board::default();
        board.set_beautify_options(crate::canvas::BeautifyOptions {
            writing: false,
            ..crate::canvas::BeautifyOptions::default()
        });
        circle(&mut board, (300.0, 300.0), 80.0);
        board.set_input_mode(InputMode::Lasso);
        lasso(
            &mut board,
            &[
                (150.0, 150.0),
                (450.0, 150.0),
                (450.0, 450.0),
                (150.0, 450.0),
                (150.0, 150.0),
            ],
        );
        let depth = board.undo_depth();
        board.selection_beautify();
        assert_eq!(board.undo_depth(), depth + 1, "one step");
        assert!(board.doc().strokes().all(|s| s.shape.is_some()));
        assert!(board.selection_is_corrected());
        board.selection_beautify();
        assert_eq!(board.undo_depth(), depth + 1, "nothing left to change");
        board.selection_original();
        assert!(board.doc().strokes().all(|s| !s.is_corrected()));
        assert_eq!(board.undo_depth(), depth + 2);
    }

    #[test]
    fn leaving_lasso_drops_the_selection() {
        let mut board = Board::default();
        draw(&mut board, (20.0, 20.0), (60.0, 60.0));
        board.set_input_mode(InputMode::Lasso);
        lasso(&mut board, &SQUARE);
        assert!(board.selection().is_some());
        board.set_input_mode(InputMode::Draw);
        assert!(board.selection().is_none());
    }

    /// A board with a stroke, a text box and an image inside [`SQUARE`] on
    /// the bottom layer, and an empty layer above it.
    fn mixed_board() -> (Board, u32, u32) {
        let mut board = Board {
            last_surface_size: (100.0, 100.0),
            ..Board::default()
        };
        let bottom = board.doc().active_layer;
        draw(&mut board, (20.0, 20.0), (60.0, 60.0));
        board.begin_text_draft(30.0, 30.0);
        board.set_session_markdown("hola");
        board.end_text_session();
        assert!(board.add_image_at_view_center("a.png", (20, 20)));
        board.end_image_session();
        let top = board.add_layer();
        (board, bottom, top)
    }

    fn holder(board: &Board) -> (Option<u32>, Option<u32>, Option<u32>) {
        let doc = board.doc();
        let layer_of =
            |f: &dyn Fn(&crate::doc::Layer) -> bool| doc.layers.iter().find(|l| f(l)).map(|l| l.id);
        (
            layer_of(&|l| !l.strokes.is_empty()),
            layer_of(&|l| !l.texts.is_empty()),
            layer_of(&|l| !l.images.is_empty()),
        )
    }

    #[test]
    fn lasso_takes_text_boxes_and_images_too() {
        let (mut board, ..) = mixed_board();
        board.set_input_mode(InputMode::Lasso);
        lasso(&mut board, &SQUARE);
        let selection = board.selection().expect("selection");
        assert_eq!(
            (
                selection.strokes.len(),
                selection.texts.len(),
                selection.images.len()
            ),
            (1, 1, 1)
        );
        board.selection_delete();
        let doc = board.doc();
        assert!(
            doc.layers
                .iter()
                .all(|l| l.texts.is_empty() && l.images.is_empty())
        );
        assert_eq!(doc.stroke_count(), 0);
    }

    #[test]
    fn selection_goes_up_and_down_a_layer_as_one_step() {
        let (mut board, bottom, top) = mixed_board();
        board.set_input_mode(InputMode::Lasso);
        lasso(&mut board, &SQUARE);
        let depth = board.undo_depth();

        assert!(!board.selection_step_layer(crate::canvas::Step::Down));
        assert!(board.selection_step_layer(crate::canvas::Step::Up));
        assert_eq!(holder(&board), (Some(top), Some(top), Some(top)));
        assert_eq!(board.undo_depth(), depth + 1);
        assert!(!board.selection_step_layer(crate::canvas::Step::Up));

        assert!(board.undo());
        assert_eq!(holder(&board), (Some(bottom), Some(bottom), Some(bottom)));
    }

    #[test]
    fn locked_layers_are_skipped() {
        let (mut board, bottom, middle) = mixed_board();
        let top = board.add_layer();
        board.set_layer_locked(middle, true);
        board.set_input_mode(InputMode::Lasso);
        lasso(&mut board, &SQUARE);
        assert!(board.selection_step_layer(crate::canvas::Step::Up));
        assert_ne!(holder(&board).0, Some(bottom));
        assert_eq!(holder(&board), (Some(top), Some(top), Some(top)));
    }
}
