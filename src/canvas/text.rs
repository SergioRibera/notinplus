//! Text boxes on the [`Board`]: the edit session, tap detection for
//! [`InputMode::Text`], hit testing and the scene the UI lays out.
//!
//! Editing happens in a [`TextSession`]. Moves, resizes, rotations and
//! typing land on the session's copy of the box; the doc only changes when
//! the session ends, in one undo entry. A box created by a tap lives only in
//! the session until then, so an abandoned empty box never reaches the doc.

use std::time::{Duration, Instant};

use super::{Board, InputMode};
use crate::doc_op::DocOp;
use crate::text_box::{TEXT_FALLBACK_HEIGHT, TextBox, TextBoxId, TextFrame};

/// Longest press that still counts as a tap.
const TAP_MAX_DURATION: Duration = Duration::from_millis(300);
/// Movement (surface pixels) past which a press becomes a drag.
const TAP_SLOP_PX: f32 = 8.0;
/// Screen distance around the edited box that belongs to its handles, so a
/// tap on a handle does not end the session.
const SESSION_HANDLE_MARGIN_PX: f32 = 28.0;

/// A press that may still turn out to be a tap.
#[derive(Debug, Clone, Copy)]
pub(super) struct TapTrack {
    start: (f32, f32),
    at: Instant,
    moved: bool,
}

impl TapTrack {
    fn new(sx: f32, sy: f32) -> Self {
        Self {
            start: (sx, sy),
            at: Instant::now(),
            moved: false,
        }
    }

    fn update(&mut self, sx: f32, sy: f32) {
        if (sx - self.start.0).hypot(sy - self.start.1) > TAP_SLOP_PX {
            self.moved = true;
        }
    }

    fn is_tap(&self) -> bool {
        !self.moved && self.at.elapsed() <= TAP_MAX_DURATION
    }
}

/// A text box being edited.
#[derive(Debug, Clone)]
pub(super) struct TextSession {
    /// The box as the doc holds it, `None` while it is a fresh draft.
    original: Option<TextBox>,
    layer_id: u32,
    current: TextBox,
}

/// Text boxes of one layer, as the UI lays them out above the layer's
/// strokes.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLayerView {
    /// Position of the layer in paint order (`0` paints first).
    pub index: usize,
    pub opacity: f32,
    pub texts: Vec<SceneText>,
}

/// A box to lay out, with its last measured height in world units.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneText {
    pub text: TextBox,
    pub height: f32,
}

/// Everything the UI needs to lay text boxes out.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextScene {
    /// Visible layers holding at least one box, bottom to top.
    pub layers: Vec<TextLayerView>,
    /// Box under edit, if any.
    pub editing: Option<TextBoxId>,
    /// Total layer count, so paint passes can cover the layers above the
    /// last one with text.
    pub layer_count: usize,
    /// Board is in [`InputMode::Text`]: boxes show their bounds.
    pub text_mode: bool,
}

impl TextScene {
    /// Layer index ranges painted by each canvas pass: one pass below each
    /// layer's text boxes, plus a last pass up to the top layer.
    #[must_use]
    pub fn paint_passes(&self) -> Vec<std::ops::Range<usize>> {
        let mut passes = Vec::with_capacity(self.layers.len() + 1);
        let mut start = 0;
        for layer in &self.layers {
            passes.push(start..layer.index + 1);
            start = layer.index + 1;
        }
        passes.push(start..self.layer_count.max(start));
        passes
    }
}

impl Board {
    /// Enter [`InputMode::Text`], or go back to the mode used before it.
    /// Returns the new mode.
    pub fn toggle_text_mode(&mut self) -> InputMode {
        let next = if self.input_mode == InputMode::Text {
            self.mode_before_text
        } else {
            self.mode_before_text = self.input_mode;
            InputMode::Text
        };
        self.set_input_mode(next);
        next
    }

    /// Bump the counter the text UI watches, after anything that changes
    /// what [`Self::text_scene`] returns.
    pub(super) const fn touch_text_scene(&mut self) {
        self.text_rev = self.text_rev.wrapping_add(1);
    }

    /// Changes whenever [`Self::text_scene`] may return something new.
    #[must_use]
    pub const fn text_revision(&self) -> u64 {
        self.text_rev
    }

    /// Snapshot of the text boxes to lay out, with the edit session's copy
    /// in place of the stored box.
    #[must_use]
    pub fn text_scene(&self) -> TextScene {
        let session = self.text_session.as_ref();
        let layers = self
            .doc
            .layers
            .iter()
            .enumerate()
            .filter(|(_, layer)| layer.visible)
            .filter_map(|(index, layer)| {
                let scene_text = |text: &TextBox| SceneText {
                    height: self.text_height(text.id),
                    text: text.clone(),
                };
                let mut texts: Vec<SceneText> = layer
                    .texts
                    .iter()
                    .map(|text| match session {
                        Some(s) if s.current.id == text.id => scene_text(&s.current),
                        _ => scene_text(text),
                    })
                    .collect();
                if let Some(s) = session
                    && s.original.is_none()
                    && s.layer_id == layer.id
                {
                    texts.push(scene_text(&s.current));
                }
                (!texts.is_empty()).then(|| TextLayerView {
                    index,
                    opacity: layer.opacity.clamp(0.0, 1.0),
                    texts,
                })
            })
            .collect();
        TextScene {
            layers,
            editing: session.map(|s| s.current.id),
            layer_count: self.doc.layers.len(),
            text_mode: self.input_mode == InputMode::Text,
        }
    }

    /// Record the laid-out height of a box, in world units. Hit testing
    /// and the edit handles use it; nothing is persisted.
    pub fn set_text_height(&mut self, id: TextBoxId, height: f32) {
        if !height.is_finite() || height <= 0.0 {
            return;
        }
        let previous = self.text_heights.insert(id, height);
        if previous.is_none_or(|previous| (previous - height).abs() > 0.25) {
            self.touch_text_scene();
            self.notify();
        }
    }

    fn text_height(&self, id: TextBoxId) -> f32 {
        self.text_heights
            .get(&id)
            .copied()
            .unwrap_or(TEXT_FALLBACK_HEIGHT)
    }

    /// Topmost box under the world point, on a visible and unlocked layer.
    fn editable_text_at(&self, wx: f32, wy: f32) -> Option<TextBoxId> {
        self.doc
            .layers
            .iter()
            .rev()
            .filter(|layer| layer.visible && !layer.locked)
            .flat_map(|layer| layer.texts.iter().rev())
            .find(|text| text.frame.contains(self.text_height(text.id), wx, wy, 0.0))
            .map(|text| text.id)
    }

    /// Box under edit, as currently edited.
    #[must_use]
    pub fn text_session(&self) -> Option<&TextBox> {
        self.text_session.as_ref().map(|s| &s.current)
    }

    /// Start editing a stored box.
    pub fn begin_text_edit(&mut self, id: TextBoxId) {
        self.end_text_session();
        if self.is_read_only() {
            return;
        }
        let Some((layer_id, text)) = self.doc.locate_text(id) else {
            return;
        };
        let text = text.clone();
        self.text_session = Some(TextSession {
            original: Some(text.clone()),
            layer_id,
            current: text,
        });
        self.touch_text_scene();
        self.notify();
    }

    /// Start a new box on the active layer with its first line centred on
    /// the world point. Ignored when the active layer is hidden or locked.
    pub fn begin_text_draft(&mut self, wx: f32, wy: f32) {
        self.end_text_session();
        if self.is_read_only() {
            return;
        }
        let layer_id = self.doc.active_layer;
        if !self
            .doc
            .layer(layer_id)
            .is_some_and(|layer| layer.visible && !layer.locked)
        {
            return;
        }
        let text = TextBox {
            id: TextBoxId::new_v4(),
            frame: TextFrame::at(wx, TEXT_FALLBACK_HEIGHT.mul_add(-0.5, wy)),
            markdown: String::new(),
            color: self.current_color,
        };
        self.text_session = Some(TextSession {
            original: None,
            layer_id,
            current: text,
        });
        self.touch_text_scene();
        self.notify();
    }

    /// Keep the edited body. Does not touch the doc until the session ends.
    pub fn set_session_markdown(&mut self, markdown: &str) {
        if let Some(session) = self.text_session.as_mut()
            && session.current.markdown != markdown
        {
            markdown.clone_into(&mut session.current.markdown);
        }
    }

    /// Move, resize or rotate the edited box.
    pub fn set_session_frame(&mut self, frame: TextFrame) {
        let Some(session) = self.text_session.as_mut() else {
            return;
        };
        if session.current.frame == frame {
            return;
        }
        session.current.frame = frame;
        self.touch_text_scene();
        self.notify();
    }

    /// Close the edit session and write it to the doc as one undo entry. A
    /// blank box is dropped (and deleted when it was stored).
    pub fn end_text_session(&mut self) {
        let Some(session) = self.text_session.take() else {
            return;
        };
        let TextSession {
            original,
            layer_id,
            current,
        } = session;
        let ops = match original {
            None if current.is_blank() => Vec::new(),
            None => vec![DocOp::AddText {
                layer_id,
                text: current,
            }],
            Some(original) if current.is_blank() => vec![DocOp::DeleteText { id: original.id }],
            Some(original) => {
                let mut ops = Vec::new();
                if original.frame != current.frame {
                    ops.push(DocOp::SetTextFrame {
                        id: current.id,
                        frame: current.frame,
                    });
                }
                if original.markdown != current.markdown || original.color != current.color {
                    ops.push(DocOp::UpdateText {
                        id: current.id,
                        markdown: current.markdown,
                        color: current.color,
                    });
                }
                ops
            }
        };
        self.apply_ops_as_one(ops);
        self.touch_text_scene();
        self.notify();
    }

    /// Discard the edited box: a draft vanishes, a stored box is deleted.
    pub fn delete_session_text(&mut self) {
        let Some(session) = self.text_session.take() else {
            return;
        };
        if let Some(original) = session.original {
            self.apply_ops_as_one(vec![DocOp::DeleteText { id: original.id }]);
        }
        self.touch_text_scene();
        self.notify();
    }

    /// Emit `ops` as a single undo entry and ping autosave. No-op when
    /// empty.
    pub(super) fn apply_ops_as_one(&mut self, ops: Vec<DocOp>) {
        if ops.is_empty() {
            return;
        }
        self.undo.begin();
        for op in ops {
            self.apply_op(op);
        }
        self.undo.commit();
        self.notify_commit();
    }

    /// React to a tap in [`InputMode::Text`] at the surface point: a tap on
    /// the edited box (or its handles) is left to the editor, a tap
    /// elsewhere ends the session and opens the box under it, and a tap on
    /// empty canvas with no session starts a new box.
    pub fn text_tap(&mut self, sx: f32, sy: f32) {
        let (wx, wy) = self.viewport.screen_to_world(sx, sy);
        if let Some(session) = &self.text_session {
            let height = self.text_height(session.current.id);
            let margin = SESSION_HANDLE_MARGIN_PX / self.viewport.scale;
            if session.current.frame.contains(height, wx, wy, margin) {
                return;
            }
            self.end_text_session();
            if let Some(id) = self.editable_text_at(wx, wy) {
                self.begin_text_edit(id);
            }
            return;
        }
        match self.editable_text_at(wx, wy) {
            Some(id) => self.begin_text_edit(id),
            None => self.begin_text_draft(wx, wy),
        }
    }

    /// Start tracking a press that may become a text tap.
    pub fn text_press_down(&mut self, sx: f32, sy: f32) {
        self.tap = Some(TapTrack::new(sx, sy));
    }

    pub fn text_press_move(&mut self, sx: f32, sy: f32) {
        if let Some(tap) = self.tap.as_mut() {
            tap.update(sx, sy);
        }
    }

    /// Forget the tracked press: a second finger landed, the gesture was
    /// cancelled.
    pub const fn text_press_cancel(&mut self) {
        self.tap = None;
    }

    /// Finish the tracked press. Runs [`Self::text_tap`] when it was a tap
    /// and the board is still in text mode.
    pub fn text_press_up(&mut self, sx: f32, sy: f32) {
        let Some(mut tap) = self.tap.take() else {
            return;
        };
        tap.update(sx, sy);
        if tap.is_tap() && self.input_mode == InputMode::Text {
            self.text_tap(tap.start.0, tap.start.1);
        }
    }

    /// Ids of the active layer's boxes an eraser circle touches.
    pub(super) fn texts_touching_circle(&self, wx: f32, wy: f32, radius: f32) -> Vec<TextBoxId> {
        self.active_editable_texts()
            .filter(|text| {
                text.frame
                    .touches_circle(self.text_height(text.id), wx, wy, radius)
            })
            .map(|text| text.id)
            .collect()
    }

    /// Ids of the active layer's boxes whose bounds overlap the world rect.
    pub(super) fn texts_in_rect(
        &self,
        min_x: f32,
        min_y: f32,
        max_x: f32,
        max_y: f32,
    ) -> Vec<TextBoxId> {
        self.active_editable_texts()
            .filter(|text| {
                let (x0, y0, x1, y1) = text.frame.bounds(self.text_height(text.id));
                x0 <= max_x && x1 >= min_x && y0 <= max_y && y1 >= min_y
            })
            .map(|text| text.id)
            .collect()
    }

    fn active_editable_texts(&self) -> impl Iterator<Item = &TextBox> {
        self.doc
            .layer(self.doc.active_layer)
            .filter(|layer| layer.visible && !layer.locked)
            .into_iter()
            .flat_map(|layer| layer.texts.iter())
    }

    /// Delete text boxes as part of the caller's undo transaction. The edit
    /// session is closed first when it holds one of them.
    pub(super) fn erase_texts(&mut self, ids: &[TextBoxId]) {
        if ids.is_empty() {
            return;
        }
        if self
            .text_session
            .as_ref()
            .is_some_and(|s| ids.contains(&s.current.id))
        {
            self.text_session = None;
        }
        for id in ids {
            self.apply_op(DocOp::DeleteText { id: *id });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board_with_text(markdown: &str) -> (Board, TextBoxId) {
        let mut board = Board::default();
        board.begin_text_draft(10.0, 10.0);
        board.set_session_markdown(markdown);
        let id = board.text_session().map(|t| t.id).expect("draft open");
        board.end_text_session();
        (board, id)
    }

    #[test]
    fn blank_draft_never_reaches_the_doc() {
        let (board, id) = board_with_text("   ");
        assert!(board.doc().text(id).is_none());
        assert_eq!(board.undo_depth(), 0);
    }

    #[test]
    fn edit_session_is_one_undo_step() {
        let (mut board, id) = board_with_text("hola");
        assert_eq!(board.undo_depth(), 1);

        board.begin_text_edit(id);
        board.set_session_markdown("hola mundo");
        let mut frame = board.text_session().expect("editing").frame;
        frame.width = 200.0;
        frame.rotation = 15.0;
        board.set_session_frame(frame);
        board.end_text_session();
        assert_eq!(board.undo_depth(), 2);

        board.undo();
        let text = board.doc().text(id).expect("still there");
        assert_eq!(text.markdown, "hola");
        assert!(text.frame.rotation.abs() < f32::EPSILON);
        board.undo();
        assert!(board.doc().text(id).is_none());
    }

    #[test]
    fn clearing_the_body_deletes_the_box() {
        let (mut board, id) = board_with_text("hola");
        board.begin_text_edit(id);
        board.set_session_markdown("");
        board.end_text_session();
        assert!(board.doc().text(id).is_none());
        board.undo();
        assert!(board.doc().text(id).is_some());
    }

    #[test]
    fn taps_create_end_and_reopen() {
        let mut board = Board::default();
        board.set_input_mode(InputMode::Text);
        board.text_tap(50.0, 50.0);
        let id = board.text_session().map(|t| t.id).expect("draft");
        board.set_session_markdown("x");
        // Far away: only closes the session.
        board.text_tap(900.0, 900.0);
        assert!(board.text_session().is_none());
        assert!(board.doc().text(id).is_some());
        // On the box: opens it again.
        board.text_tap(60.0, 50.0);
        assert_eq!(board.text_session().map(|t| t.id), Some(id));
    }

    #[test]
    fn drag_is_not_a_tap() {
        let mut board = Board::default();
        board.set_input_mode(InputMode::Text);
        board.text_press_down(10.0, 10.0);
        board.text_press_move(60.0, 10.0);
        board.text_press_up(60.0, 10.0);
        assert!(board.text_session().is_none());
    }

    #[test]
    fn leaving_text_mode_commits() {
        let mut board = Board::default();
        board.set_input_mode(InputMode::Text);
        board.text_tap(50.0, 50.0);
        board.set_session_markdown("x");
        assert_eq!(board.toggle_text_mode(), InputMode::Draw);
        assert!(board.text_session().is_none());
        assert_eq!(board.text_scene().layers.len(), 1);
    }

    #[test]
    fn marquee_eraser_deletes_boxes() {
        let (mut board, id) = board_with_text("hola");
        board.erase_strokes_in_rect(0.0, 0.0, 30.0, 30.0);
        assert!(board.doc().text(id).is_none());
        assert_eq!(board.undo_depth(), 2);
    }

    #[test]
    fn scene_splits_paint_around_text_layers() {
        let (mut board, _) = board_with_text("hola");
        board.add_layer();
        let passes = board.text_scene().paint_passes();
        assert_eq!(passes, vec![0..1, 1..2]);
    }
}

impl Board {
    /// Add a text box holding `markdown` at the centre of what the pane
    /// shows, as one undo step. Returns `false` when the canvas cannot
    /// take text (read only, hidden or locked layer, size unknown).
    pub fn insert_text_at_view_center(&mut self, markdown: &str) -> bool {
        let (sw, sh) = self.last_surface_size;
        if sw <= 0.0 || sh <= 0.0 || self.viewport.scale <= 0.0 {
            return false;
        }
        let wx = crate::text_box::TEXT_DEFAULT_WIDTH.mul_add(-0.5, sw.mul_add(0.5, -self.viewport.tx) / self.viewport.scale);
        let wy = sh.mul_add(0.5, -self.viewport.ty) / self.viewport.scale;
        self.begin_text_draft(wx, wy);
        if self.text_session.is_none() {
            return false;
        }
        self.set_session_markdown(markdown);
        self.end_text_session();
        true
    }
}

impl crate::doc::Doc {
    /// Append a text box with `markdown` below the existing ones on the
    /// active layer (for documents that are not open).
    pub fn append_text(&mut self, markdown: &str, color: [u8; 4]) {
        let layer_id = self.active_layer;
        let below = self.layer(layer_id).map_or(0.0, |layer| {
            layer
                .texts
                .iter()
                .map(|t| crate::text_box::TEXT_FALLBACK_HEIGHT.mul_add(3.0, t.frame.y))
                .fold(0.0_f32, f32::max)
        });
        self.emit(crate::doc_op::DocOp::AddText {
            layer_id,
            text: crate::text_box::TextBox {
                id: crate::text_box::TextBoxId::new_v4(),
                frame: crate::text_box::TextFrame::at(0.0, below),
                markdown: markdown.to_owned(),
                color,
            },
        });
    }
}
