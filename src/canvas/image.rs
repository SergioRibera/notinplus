//! Images on the [`Board`]: selection in image mode (see [`InputMode`]), the
//! session that moves, resizes and rotates the selected image, hit testing
//! and the bits of the scene the UI lays out.
//!
//! A selected image is edited in an [`ImageSession`]: frame changes land on
//! the session's copy and reach the doc when the selection ends, as one
//! undo entry (the same rule text boxes follow in `canvas::text`). Adding
//! an image is its own undo entry; the new image is selected right away.
//!
//! [`InputMode`]: super::InputMode

use super::Board;
use crate::doc_op::DocOp;
use crate::image_box::{ImageBox, ImageBoxId};
use crate::text_box::TextFrame;

/// Screen distance around the selected image that belongs to its handles,
/// so a tap on a handle does not end the selection.
const SESSION_HANDLE_MARGIN_PX: f32 = 28.0;

/// Heights are world units: a difference below this is no change.
fn same_height(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

/// An image being moved, resized or rotated.
#[derive(Debug, Clone)]
pub(super) struct ImageSession {
    original: ImageBox,
    current: ImageBox,
}

impl Board {
    /// Image selected for editing, as currently edited.
    #[must_use]
    pub fn image_session(&self) -> Option<&ImageBox> {
        self.image_session.as_ref().map(|s| &s.current)
    }

    /// The session's copy of `image`, when it is the selected one.
    pub(super) fn session_image<'a>(&'a self, image: &'a ImageBox) -> &'a ImageBox {
        match &self.image_session {
            Some(s) if s.current.id == image.id => &s.current,
            _ => image,
        }
    }

    /// Select a stored image. Ends the previous selection first.
    #[sismo::function("canvas_image::select")]
    pub fn select_image(&mut self, id: ImageBoxId) {
        self.end_image_session();
        if self.is_read_only() {
            return;
        }
        let Some(image) = self.doc.image(id) else {
            return;
        };
        let image = image.clone();
        self.image_session = Some(ImageSession {
            original: image.clone(),
            current: image,
        });
        self.touch_text_scene();
        self.notify();
    }

    /// Move, resize or rotate the selected image.
    #[sismo::function("canvas_image::set_session_frame")]
    pub fn set_session_image_frame(&mut self, frame: TextFrame, height: f32) {
        let Some(session) = self.image_session.as_mut() else {
            return;
        };
        if session.current.frame == frame && same_height(session.current.height, height) {
            return;
        }
        session.current.frame = frame;
        session.current.height = height;
        self.touch_text_scene();
        self.notify();
    }

    /// Drop the selection, writing a changed placement to the doc as one
    /// undo entry.
    #[sismo::function("canvas_image::end_session")]
    pub fn end_image_session(&mut self) {
        let Some(ImageSession { original, current }) = self.image_session.take() else {
            return;
        };
        if original.frame != current.frame || !same_height(original.height, current.height) {
            self.apply_ops_as_one(vec![DocOp::SetImageFrame {
                id: current.id,
                frame: current.frame,
                height: current.height,
            }]);
        }
        self.touch_text_scene();
        self.notify();
    }

    /// Delete the selected image.
    #[sismo::function("canvas_image::delete_session")]
    pub fn delete_session_image(&mut self) {
        let Some(session) = self.image_session.take() else {
            return;
        };
        self.apply_ops_as_one(vec![DocOp::DeleteImage {
            id: session.original.id,
        }]);
        self.touch_text_scene();
        self.notify();
    }

    /// Put `asset` (`natural` pixels big) on the active layer, centred on
    /// what the pane shows, as one undo step, and select it. Returns
    /// `false` when the canvas cannot take it (read only, hidden or locked
    /// layer, size unknown).
    #[sismo::function("canvas_image::add_at_view_center")]
    pub fn add_image_at_view_center(&mut self, asset: &str, natural: (u32, u32)) -> bool {
        self.end_image_session();
        let (sw, sh) = self.last_surface_size;
        if self.is_read_only() || sw <= 0.0 || sh <= 0.0 || self.viewport.scale <= 0.0 {
            return false;
        }
        let layer_id = self.doc.active_layer;
        if !self
            .doc
            .layer(layer_id)
            .is_some_and(|layer| layer.visible && !layer.locked)
        {
            return false;
        }
        let cx = sw.mul_add(0.5, -self.viewport.tx) / self.viewport.scale;
        let cy = sh.mul_add(0.5, -self.viewport.ty) / self.viewport.scale;
        let image = ImageBox::centred(asset, natural, cx, cy);
        let id = image.id;
        self.apply_ops_as_one(vec![DocOp::AddImage { layer_id, image }]);
        self.select_image(id);
        true
    }

    /// Topmost image under the world point, on a visible and unlocked
    /// layer.
    #[sismo::function("canvas_image::editable_at")]
    fn editable_image_at(&self, wx: f32, wy: f32) -> Option<ImageBoxId> {
        self.doc
            .layers
            .iter()
            .rev()
            .filter(|layer| layer.visible && !layer.locked)
            .flat_map(|layer| layer.images.iter().rev())
            .map(|image| self.session_image(image))
            .find(|image| image.frame.contains(image.height, wx, wy, 0.0))
            .map(|image| image.id)
    }

    /// React to a tap in image mode at the surface point: a tap
    /// on the selected image (or its handles) is left to the handles, a tap
    /// on another image selects it, and a tap on empty canvas drops the
    /// selection.
    #[sismo::function("canvas_image::tap")]
    pub fn image_tap(&mut self, sx: f32, sy: f32) {
        let (wx, wy) = self.viewport.screen_to_world(sx, sy);
        if let Some(session) = &self.image_session {
            let margin = SESSION_HANDLE_MARGIN_PX / self.viewport.scale;
            let current = &session.current;
            if current.frame.contains(current.height, wx, wy, margin) {
                return;
            }
        }
        match self.editable_image_at(wx, wy) {
            Some(id) => self.select_image(id),
            None => self.end_image_session(),
        }
    }

    /// Ids of the active layer's images an eraser circle touches.
    #[sismo::function("canvas_image::touching_circle")]
    pub(super) fn images_touching_circle(&self, wx: f32, wy: f32, radius: f32) -> Vec<ImageBoxId> {
        self.active_editable_images()
            .filter(|image| image.frame.touches_circle(image.height, wx, wy, radius))
            .map(|image| image.id)
            .collect()
    }

    /// Ids of the active layer's images whose bounds overlap the world
    /// rect.
    #[sismo::function("canvas_image::in_rect")]
    pub(super) fn images_in_rect(
        &self,
        min_x: f32,
        min_y: f32,
        max_x: f32,
        max_y: f32,
    ) -> Vec<ImageBoxId> {
        self.active_editable_images()
            .filter(|image| {
                let (x0, y0, x1, y1) = image.frame.bounds(image.height);
                x0 <= max_x && x1 >= min_x && y0 <= max_y && y1 >= min_y
            })
            .map(|image| image.id)
            .collect()
    }

    fn active_editable_images(&self) -> impl Iterator<Item = &ImageBox> {
        self.doc
            .layer(self.doc.active_layer)
            .filter(|layer| layer.visible && !layer.locked)
            .into_iter()
            .flat_map(|layer| layer.images.iter())
    }

    /// Delete images as part of the caller's undo transaction. The
    /// selection is dropped first when it holds one of them.
    #[sismo::function("canvas_image::erase")]
    pub(super) fn erase_images(&mut self, ids: &[ImageBoxId]) {
        if ids.is_empty() {
            return;
        }
        if self
            .image_session
            .as_ref()
            .is_some_and(|s| ids.contains(&s.current.id))
        {
            self.image_session = None;
        }
        for id in ids {
            self.apply_op(DocOp::DeleteImage { id: *id });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::InputMode;
    use super::*;

    fn board_with_image() -> (Board, ImageBoxId) {
        let mut board = Board {
            last_surface_size: (400.0, 300.0),
            ..Default::default()
        };
        assert!(board.add_image_at_view_center("a.png", (200, 100)));
        let id = board.image_session().map(|i| i.id).expect("selected");
        (board, id)
    }

    #[test]
    fn adding_selects_and_is_one_undo_step() {
        let (mut board, id) = board_with_image();
        assert_eq!(board.undo_depth(), 1);
        let image = board.doc().image(id).expect("stored").clone();
        let (cx, cy) = image.frame.center(image.height);
        assert!((cx - 200.0).abs() < 1e-3 && (cy - 150.0).abs() < 1e-3);
        board.undo();
        assert!(board.doc().image(id).is_none());
        assert!(board.image_session().is_none());
    }

    #[test]
    fn frame_changes_land_when_the_selection_ends() {
        let (mut board, id) = board_with_image();
        let image = board.image_session().expect("selected").clone();
        let mut frame = image.frame;
        frame.x += 40.0;
        frame.rotation = 30.0;
        board.set_session_image_frame(frame, image.height * 2.0);
        assert_eq!(board.undo_depth(), 1);
        board.end_image_session();
        assert_eq!(board.undo_depth(), 2);
        let stored = board.doc().image(id).expect("stored");
        assert!((stored.frame.rotation - 30.0).abs() < f32::EPSILON);
        board.undo();
        let stored = board.doc().image(id).expect("still there");
        assert!(stored.frame.rotation.abs() < f32::EPSILON);
    }

    #[test]
    fn taps_select_and_drop() {
        let (mut board, id) = board_with_image();
        board.set_input_mode(InputMode::Image);
        board.end_image_session();
        assert!(board.image_session().is_none());
        board.image_tap(200.0, 150.0);
        assert_eq!(board.image_session().map(|i| i.id), Some(id));
        board.image_tap(5.0, 5.0);
        assert!(board.image_session().is_none());
    }

    #[test]
    fn deleting_the_selection_undoes() {
        let (mut board, id) = board_with_image();
        board.delete_session_image();
        assert!(board.doc().image(id).is_none());
        board.undo();
        assert!(board.doc().image(id).is_some());
    }

    #[test]
    fn marquee_eraser_deletes_images() {
        let (mut board, id) = board_with_image();
        board.end_image_session();
        board.erase_strokes_in_rect(190.0, 140.0, 210.0, 160.0);
        assert!(board.doc().image(id).is_none());
    }
}
