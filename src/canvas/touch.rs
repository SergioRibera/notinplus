//! Fingers on the canvas. One finger pans, two pinch-zoom, a quick
//! two-finger double tap undoes. Where no pen has been seen (or the
//! user asked for it) one finger writes instead, and "Mover" held in
//! the dock turns it back into panning.

use std::time::{Duration, Instant};

use super::{Board, InputMode};
use crate::brush::InkPoint;

/// Longest two-finger touch that still counts as a tap.
const TAP_TIME: Duration = Duration::from_millis(280);
/// Most a two-finger tap may move (surface pixels).
const TAP_SLOP: f32 = 14.0;
/// Two taps closer than this are a double tap.
const DOUBLE_TAP: Duration = Duration::from_millis(450);
/// Pressure a finger draws with.
const FINGER_PRESSURE: u8 = 140;

/// Finger state beyond the pan/pinch tracker.
#[derive(Debug, Default)]
pub(super) struct TouchState {
    /// A single finger writes instead of panning.
    finger_draws: bool,
    /// "Mover" is held: fingers pan even when they would write.
    pan_hold: bool,
    /// The finger currently writing.
    writing: Option<u64>,
    /// A two-finger contact in progress: when it started, where, and
    /// whether it moved enough to be a pinch.
    two: Option<(Instant, (f32, f32), f32, bool)>,
    last_two_tap: Option<Instant>,
}

impl Board {
    /// Let one finger write (no pen on this device, or the user's
    /// choice).
    pub const fn set_finger_draws(&mut self, draws: bool) {
        self.touch.finger_draws = draws;
    }

    #[must_use]
    pub const fn finger_draws(&self) -> bool {
        self.touch.finger_draws
    }

    /// Hold "Mover": fingers pan while it is down.
    pub const fn set_pan_hold(&mut self, hold: bool) {
        self.touch.pan_hold = hold;
    }

    const fn finger_writes(&self) -> bool {
        self.touch.finger_draws && !self.touch.pan_hold && !self.read_only
    }

    fn finger_begin(&mut self, id: u64, sx: f32, sy: f32) {
        self.touch.writing = Some(id);
        match self.input_mode {
            InputMode::Lasso => self.lasso_down(sx, sy),
            InputMode::Text => self.text_press_down(sx, sy),
            InputMode::Draw | InputMode::Pan | InputMode::Select => {
                self.begin_screen(InkPoint::new(sx, sy, FINGER_PRESSURE, 0, 0));
            }
        }
    }

    fn finger_extend(&mut self, sx: f32, sy: f32) {
        match self.input_mode {
            InputMode::Lasso => self.lasso_move(sx, sy),
            InputMode::Text => self.text_press_move(sx, sy),
            InputMode::Draw | InputMode::Pan | InputMode::Select => {
                self.extend_screen(InkPoint::new(sx, sy, FINGER_PRESSURE, 0, 8_000));
            }
        }
    }

    fn finger_end(&mut self, sx: f32, sy: f32) {
        self.touch.writing = None;
        match self.input_mode {
            InputMode::Lasso => self.lasso_up(),
            InputMode::Text => self.text_press_up(sx, sy),
            InputMode::Draw | InputMode::Pan | InputMode::Select => self.end(),
        }
    }

    fn finger_abort(&mut self) {
        if self.touch.writing.take().is_some() {
            match self.input_mode {
                InputMode::Lasso => self.lasso_cancel(),
                InputMode::Text => self.text_press_cancel(),
                InputMode::Draw | InputMode::Pan | InputMode::Select => self.cancel(),
            }
        }
    }

    /// Register a new touch point.
    pub fn touch_down(&mut self, id: u64, sx: f32, sy: f32) {
        self.finger_positions.insert(id, (sx, sy));
        let n = self.finger_positions.len();
        if n == 1 {
            if self.finger_writes() {
                self.finger_begin(id, sx, sy);
            } else {
                self.touch_pan_finger = Some(id);
                self.pan_begin(sx, sy);
                if self.input_mode == InputMode::Text {
                    self.text_press_down(sx, sy);
                }
            }
        } else {
            self.text_press_cancel();
        }
        if n >= 2 && !self.gesture_active {
            self.gesture_active = true;
            self.finger_abort();
            self.cancel();
            self.pan_end();
            self.touch_pan_finger = None;
            if n == 2 {
                let baseline = self.compute_gesture_baseline();
                self.touch.two = baseline.map(|b| (Instant::now(), b.centroid, b.distance, false));
            } else {
                self.touch.two = None;
            }
        }
        self.gesture_baseline = self.compute_gesture_baseline();
    }

    /// Update a tracked finger.
    pub fn touch_move(&mut self, id: u64, sx: f32, sy: f32) {
        if !self.finger_positions.contains_key(&id) {
            return;
        }
        self.finger_positions.insert(id, (sx, sy));
        if self.gesture_active {
            let Some(new) = self.compute_gesture_baseline() else {
                return;
            };
            if let Some((_, centroid, distance, moved)) = self.touch.two.as_mut() {
                let travel = (new.centroid.0 - centroid.0).hypot(new.centroid.1 - centroid.1);
                if travel > TAP_SLOP || (new.distance - *distance).abs() > TAP_SLOP {
                    *moved = true;
                }
            }
            if let Some(prev) = self.gesture_baseline
                && prev.distance > 1.0
                && new.distance > 1.0
            {
                let factor = new.distance / prev.distance;
                if (factor - 1.0).abs() > 1e-4 {
                    self.viewport_zoom_at(new.centroid.0, new.centroid.1, factor);
                }
            }
            self.gesture_baseline = Some(new);
            return;
        }
        if self.touch.writing == Some(id) {
            self.finger_extend(sx, sy);
            return;
        }
        if self.touch_pan_finger == Some(id) {
            self.pan_move(sx, sy);
            self.text_press_move(sx, sy);
        }
    }

    /// Drop a finger from the tracker.
    pub fn touch_up(&mut self, id: u64) {
        let lifted = self.finger_positions.remove(&id);
        if self.touch.writing == Some(id) {
            let (sx, sy) = lifted.unwrap_or_default();
            self.finger_end(sx, sy);
        } else if let Some((sx, sy)) = lifted
            && self.touch_pan_finger == Some(id)
        {
            self.text_press_up(sx, sy);
        }
        if self.touch_pan_finger == Some(id) {
            self.touch_pan_finger = None;
            self.pan_end();
        }
        if self.finger_positions.is_empty() {
            self.two_finger_tap_ended();
        }
        if self.finger_positions.len() < 2 {
            if self.gesture_active {
                // A zoom just ended: repaint so the ink is drawn sharp again.
                self.notify();
            }
            self.gesture_active = false;
            self.gesture_baseline = None;
            if self.touch_pan_finger.is_none()
                && self.touch.writing.is_none()
                && let Some((&remaining, &(sx, sy))) = self.finger_positions.iter().next()
                && self.touch.two.is_none()
            {
                self.touch_pan_finger = Some(remaining);
                self.pan_begin(sx, sy);
            }
        } else {
            self.gesture_baseline = self.compute_gesture_baseline();
        }
    }

    /// The last finger of a two-finger contact lifted: a short, still
    /// contact is a tap, and two of them undo.
    fn two_finger_tap_ended(&mut self) {
        let Some((started, _, _, moved)) = self.touch.two.take() else {
            return;
        };
        if moved || started.elapsed() > TAP_TIME {
            self.touch.last_two_tap = None;
            return;
        }
        let now = Instant::now();
        match self.touch.last_two_tap {
            Some(previous) if now.duration_since(previous) <= DOUBLE_TAP => {
                self.touch.last_two_tap = None;
                self.undo();
            }
            _ => self.touch.last_two_tap = Some(now),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::canvas::Board;

    fn stroke_count(board: &Board) -> usize {
        board.doc().stroke_count()
    }

    #[test]
    fn finger_writes_when_allowed_and_pans_otherwise() {
        let mut board = Board::default();
        board.touch_down(1, 10.0, 10.0);
        board.touch_move(1, 40.0, 40.0);
        board.touch_up(1);
        assert_eq!(stroke_count(&board), 0, "a finger pans by default");

        board.set_finger_draws(true);
        board.touch_down(2, 10.0, 10.0);
        board.touch_move(2, 40.0, 40.0);
        board.touch_move(2, 60.0, 45.0);
        board.touch_up(2);
        assert_eq!(stroke_count(&board), 1, "a finger writes once allowed");

        board.set_pan_hold(true);
        board.touch_down(3, 10.0, 10.0);
        board.touch_move(3, 90.0, 90.0);
        board.touch_up(3);
        assert_eq!(stroke_count(&board), 1, "holding Mover pans");
    }

    #[test]
    fn two_finger_double_tap_undoes() {
        let mut board = Board::default();
        board.set_finger_draws(true);
        board.touch_down(1, 10.0, 10.0);
        board.touch_move(1, 50.0, 50.0);
        board.touch_up(1);
        assert_eq!(stroke_count(&board), 1);
        for _ in 0..2 {
            board.touch_down(10, 100.0, 100.0);
            board.touch_down(11, 160.0, 100.0);
            board.touch_up(10);
            board.touch_up(11);
        }
        assert_eq!(stroke_count(&board), 0, "the stroke was undone");
    }

    #[test]
    fn second_finger_cancels_the_finger_stroke() {
        let mut board = Board::default();
        board.set_finger_draws(true);
        board.touch_down(1, 10.0, 10.0);
        board.touch_move(1, 30.0, 30.0);
        board.touch_down(2, 80.0, 80.0);
        board.touch_up(1);
        board.touch_up(2);
        assert_eq!(stroke_count(&board), 0);
    }
}
