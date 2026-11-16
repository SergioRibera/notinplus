//! Beautified handwriting on the [`Board`].
//!
//! Pen strokes that look like writing gather into the open word. The
//! word closes when a stroke lands outside it or the pen rests for
//! [`WORD_IDLE`] (the UI owns that timer, see [`Board::set_word_waker`]).
//! A closed word is measured, corrected onto its line and every stroke
//! gets a [`StrokeFix`]; the raw ink is kept. The word being written
//! never moves, and words already placed are never adjusted again.

use std::time::{Duration, Instant};

use freya_engine::prelude::Canvas;
use notin_beautify::{
    Affine, Bounds, Line, Pt, Row, StrokeFix, StyleProfile, Target, WordMetrics, correct_word,
    is_writing, joins_word,
};

use super::Board;
use crate::brush::{BrushKind, Stroke};
use crate::doc_op::DocOp;
use crate::ids::StrokeId;

/// Pen rest that closes the open word.
pub const WORD_IDLE: Duration = Duration::from_millis(500);
/// How long a correction takes to settle on screen.
const SETTLE: Duration = Duration::from_millis(200);
/// X-height assumed before anything has been measured, world units.
const FALLBACK_X_HEIGHT: f32 = 16.0;
/// A word is set on a sheet rule only when its baseline is this close,
/// in x-heights.
const RULE_REACH: f32 = 0.6;

/// Brushes whose strokes can be writing.
pub(super) const fn writes(kind: BrushKind) -> bool {
    matches!(kind, BrushKind::Pen | BrushKind::Pencil | BrushKind::Marker)
}

/// Board-side state of handwriting beautification.
#[derive(Debug, Default)]
pub(super) struct Beautify {
    options: Options,
    /// Strokes of the word being written, in writing order.
    word: Vec<StrokeId>,
    /// Bumped whenever `word` changes; the idle timer only closes the
    /// word it was armed for.
    generation: u64,
    waker: Option<flume::Sender<u64>>,
    /// The line being written.
    row: Option<Row>,
    profile: StyleProfile,
    /// Corrections still settling on screen.
    settling: Vec<Settling>,
}

/// What beautification does, from the pen settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Beautify finished words.
    pub writing: bool,
    /// Snap a stroke to a figure when the pen holds still at its end.
    pub shapes: bool,
    /// Let corrections settle on screen instead of jumping.
    pub animate: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            writing: true,
            shapes: true,
            animate: true,
        }
    }
}

/// A stroke easing from its raw look into its fix.
#[derive(Debug, Clone, Copy)]
struct Settling {
    id: StrokeId,
    fix: StrokeFix,
    start: Instant,
}

impl Settling {
    /// Share of the way done, eased out; `None` once settled.
    fn progress(&self, now: Instant) -> Option<f32> {
        let t = now.duration_since(self.start).as_secs_f32() / SETTLE.as_secs_f32();
        (t < 1.0).then(|| 1.0 - (1.0 - t).powi(3))
    }
}

/// Raw positions of `stroke`.
fn raw(stroke: &Stroke) -> Vec<Pt> {
    stroke.points.iter().map(|p| Pt::new(p.x, p.y)).collect()
}

impl Board {
    #[must_use]
    pub const fn beautify_options(&self) -> Options {
        self.beautify.options
    }

    /// Apply the pen settings. Turning writing off leaves the open word
    /// as it is.
    pub fn set_beautify_options(&mut self, options: Options) {
        if !options.writing {
            self.beautify.word.clear();
        }
        self.beautify.options = options;
    }

    /// The writer's style as learned so far.
    #[must_use]
    pub const fn style_profile(&self) -> &StyleProfile {
        &self.beautify.profile
    }

    /// Restore the writer's style (from prefs).
    pub fn set_style_profile(&mut self, profile: StyleProfile) {
        self.beautify.profile = profile;
    }

    /// Channel the idle timer listens on. Each stroke added to the open
    /// word sends its generation; after [`WORD_IDLE`] with no newer one,
    /// the timer calls [`Self::flush_word`].
    pub fn set_word_waker(&mut self, waker: flume::Sender<u64>) {
        self.beautify.waker = Some(waker);
    }

    /// Close the open word if it is still the one armed as `generation`
    /// and the pen is up.
    #[sismo::function("beautify::flush_word")]
    pub fn flush_word(&mut self, generation: u64) {
        if generation == self.beautify.generation && self.active.is_none() {
            self.close_word();
        }
    }

    /// Before `stroke` is committed: close the open word when the stroke
    /// does not continue it.
    pub(super) fn beautify_before_commit(&mut self, stroke: &Stroke) {
        if self.beautify.word.is_empty() {
            return;
        }
        let continues = self.writing_stroke(stroke) && {
            let points = raw(stroke);
            match (self.word_bounds(), Bounds::of(&points)) {
                (Some(word), Some(new)) => joins_word(word, new, self.x_height_hint()),
                _ => false,
            }
        };
        if !continues {
            self.close_word();
        }
    }

    /// After stroke `id` is committed: add it to the open word if it is
    /// writing.
    pub(super) fn beautify_after_commit(&mut self, id: StrokeId) {
        let Some(stroke) = self.doc.find_stroke(id) else {
            return;
        };
        if !self.beautify.options.writing || self.read_only || !self.writing_stroke(stroke) {
            return;
        }
        let beautify = &mut self.beautify;
        beautify.word.push(id);
        beautify.generation += 1;
        if let Some(waker) = &beautify.waker {
            let _ = waker.send(beautify.generation);
        }
    }

    /// Drop the open word without correcting it (undo, reload) and
    /// forget the line, since the ink under it may have changed.
    pub(super) fn forget_word(&mut self) {
        self.beautify.word.clear();
        self.beautify.row = None;
        self.beautify.generation += 1;
    }

    /// Whether `stroke` could be part of a written word.
    fn writing_stroke(&self, stroke: &Stroke) -> bool {
        if stroke.is_corrected() {
            return false;
        }
        let Some(preset) = self.doc.preset(stroke.brush) else {
            return false;
        };
        writes(preset.kind)
            && Bounds::of(&raw(stroke)).is_some_and(|b| is_writing(b, Some(self.x_height_hint())))
    }

    fn word_bounds(&self) -> Option<Bounds> {
        self.beautify
            .word
            .iter()
            .filter_map(|id| self.doc.find_stroke(*id))
            .filter_map(|s| Bounds::of(&raw(s)))
            .reduce(Bounds::union)
    }

    /// Best guess of the writing size: the line's, the writer's, or the
    /// open word's own.
    fn x_height_hint(&self) -> f32 {
        self.beautify
            .row
            .as_ref()
            .and_then(Row::x_height)
            .or_else(|| self.beautify.profile.x_height())
            .or_else(|| {
                let strokes: Vec<Vec<Pt>> = self
                    .beautify
                    .word
                    .iter()
                    .filter_map(|id| self.doc.find_stroke(*id))
                    .map(raw)
                    .collect();
                let refs: Vec<&[Pt]> = strokes.iter().map(Vec::as_slice).collect();
                WordMetrics::measure(&refs).map(|m| m.x_height)
            })
            .unwrap_or(FALLBACK_X_HEIGHT)
    }

    /// Correct the open word onto its line.
    #[sismo::function("beautify::close_word")]
    fn close_word(&mut self) {
        let ids = std::mem::take(&mut self.beautify.word);
        self.beautify.generation += 1;
        if ids.is_empty() || self.read_only {
            return;
        }
        let strokes: Vec<Vec<Pt>> = ids
            .iter()
            .filter_map(|id| self.doc.find_stroke(*id))
            .map(raw)
            .collect();
        if strokes.len() != ids.len() {
            return;
        }
        let refs: Vec<&[Pt]> = strokes.iter().map(Vec::as_slice).collect();
        let Some(measured) = WordMetrics::measure(&refs) else {
            return;
        };
        let (target, ruled, continues) = self.word_target(&measured);
        let placed = correct_word(&refs, &target).or_else(|| {
            // Too far from the sheet rule or the line: still straighten
            // it where it stands.
            (target != Target::default())
                .then(|| {
                    correct_word(
                        &refs,
                        &Target {
                            slant: target.slant,
                            ..Target::default()
                        },
                    )
                })
                .flatten()
        });
        let Some(placed) = placed else {
            return;
        };
        let level_slant = measured.slant - measured.baseline.angle();
        self.beautify.profile.record(level_slant, measured.x_height);
        let row = match self.beautify.row.take() {
            Some(row) if continues => row,
            _ => Row::new(placed.baseline, ruled),
        };
        let row = self.beautify.row.insert(row);
        row.place(placed.bounds, placed.x_height);
        self.apply_word_fix(&ids, placed.fix);
        self.ping_prefs();
    }

    /// Where the word measured as `m` should go: `(target, set on a sheet
    /// rule, continues the current line)`.
    fn word_target(&self, m: &WordMetrics) -> (Target, bool, bool) {
        let profile = &self.beautify.profile;
        let slant = profile.slant();
        if let Some(row) = &self.beautify.row
            && row.accepts(m.bounds, m.x_height)
        {
            let after = row
                .gap()
                .zip(row.last_word())
                .map(|(gap, last)| (last.max_x, gap));
            let target = Target {
                baseline: Some(row.baseline),
                x_height: row.x_height(),
                slant,
                after,
            };
            return (target, row.ruled, true);
        }
        let center = m.bounds.center();
        let own = m.baseline.y_at(center.x);
        let rule = self
            .background
            .rule_spacing_at(center.x, center.y)
            .map(|spacing| (own / spacing).round() * spacing)
            .filter(|y| (y - own).abs() <= RULE_REACH * m.x_height);
        let target = Target {
            baseline: rule.map(Line::horizontal),
            x_height: profile.x_height(),
            slant,
            after: None,
        };
        (target, rule.is_some(), false)
    }

    /// Give every stroke of a word `fix`, folded into the undo step of
    /// the word's last stroke, and let it settle on screen.
    fn apply_word_fix(&mut self, ids: &[StrokeId], fix: StrokeFix) {
        let mut steps = Vec::new();
        for id in ids {
            let op = DocOp::SetStrokeFix {
                id: *id,
                fix: Some(fix),
            };
            let undo = self.doc.inverse_before_apply(&op);
            self.apply_op_untracked(op.clone());
            steps.push((op, undo));
        }
        let last = ids.last().copied();
        let tops_last = self.undo.top().is_some_and(|entry| {
            entry
                .forward
                .iter()
                .any(|op| matches!(op, DocOp::PushStroke { stroke, .. } if Some(stroke.id) == last))
        });
        let forward: Vec<DocOp> = steps.iter().map(|(op, _)| op.clone()).collect();
        // Undone newest first.
        let inverse: Vec<DocOp> = steps
            .iter()
            .rev()
            .flat_map(|(_, undo)| undo.clone())
            .collect();
        if !(tops_last && self.undo.amend_top(forward, inverse)) {
            self.undo.begin();
            for (op, undo) in steps {
                self.undo.record(op, undo);
            }
            self.undo.commit();
        }
        if self.beautify.options.animate {
            let start = Instant::now();
            self.beautify.settling.extend(ids.iter().map(|id| Settling {
                id: *id,
                fix,
                start,
            }));
            self.ink_rev += 1;
        }
        self.notify_commit();
    }

    /// Whether stroke `id` is still settling (painted as an overlay).
    pub(super) fn is_settling(&self, id: StrokeId) -> bool {
        let now = Instant::now();
        self.beautify
            .settling
            .iter()
            .any(|s| s.id == id && s.progress(now).is_some())
    }

    /// Forget corrections that have settled, so their strokes go back
    /// to the cached ink.
    pub(super) fn prune_settled(&mut self) {
        let now = Instant::now();
        let before = self.beautify.settling.len();
        self.beautify.settling.retain(|s| s.progress(now).is_some());
        if self.beautify.settling.len() != before {
            self.ink_rev += 1;
        }
    }

    /// Strokes still settling, part of the way into their fix. Asks for
    /// another frame while any is moving.
    #[sismo::function("beautify::paint_settling")]
    pub(super) fn paint_settling(&self, canvas: &Canvas) {
        let now = Instant::now();
        let mut moving = false;
        for settling in &self.beautify.settling {
            let Some(t) = settling.progress(now) else {
                continue;
            };
            moving = true;
            let Some(stroke) = self.doc.find_stroke(settling.id) else {
                continue;
            };
            let Some(preset) = self.doc.preset(stroke.brush) else {
                continue;
            };
            let shown = Stroke {
                fix: Some(StrokeFix {
                    affine: Affine::IDENTITY.lerp(&settling.fix.affine, t),
                    smooth: settling.fix.smooth,
                }),
                ..stroke.clone()
            };
            let path = self.brush_registry.stroke_path(preset, &shown);
            let paint = self
                .brush_registry
                .brush(preset.kind)
                .paint(preset, stroke.color);
            canvas.draw_path(&path, &paint);
        }
        if moving && let Some(notifier) = &self.notifier {
            notifier.ping();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::brush::{BrushPreset, InkPoint};
    use crate::canvas::Board;

    /// Write a small `n` with its left foot at `(x, y)`, tilted by `tilt`.
    fn letter_n(board: &mut Board, (x, y): (f32, f32), tilt: f32) {
        let h = 14.0;
        let mut pts = Vec::new();
        for i in 0..=8u8 {
            pts.push((x, (f32::from(i) / 8.0).mul_add(-h, y)));
        }
        for i in 0..=12u8 {
            let a = std::f32::consts::PI * (1.0 + f32::from(i) / 12.0);
            pts.push((
                (0.4 * h).mul_add(a.cos(), 0.4f32.mul_add(h, x)),
                0.4f32.mul_add(0.4 * h * a.sin(), 0.6f32.mul_add(-h, y)),
            ));
        }
        for i in 0..=8u8 {
            pts.push((
                0.8f32.mul_add(h, x),
                (f32::from(i) / 8.0).mul_add(0.6 * h, 0.6f32.mul_add(-h, y)),
            ));
        }
        let (sin, cos) = tilt.sin_cos();
        let mut points = pts.into_iter().map(|(px, py)| {
            let (dx, dy) = (px - x, py - y);
            InkPoint::new(
                dx.mul_add(cos, -dy * sin) + x,
                dx.mul_add(sin, dy * cos) + y,
                128,
                0,
                0,
            )
        });
        board.begin(points.next().expect("non-empty"));
        for p in points {
            board.extend(p);
        }
        board.end();
    }

    fn pen_board() -> Board {
        let mut board = Board::default();
        board.set_current_preset(BrushPreset::pen());
        board
    }

    #[test]
    fn rest_closes_the_word_and_fixes_it_in_the_last_strokes_step() {
        let mut board = pen_board();
        letter_n(&mut board, (100.0, 200.0), 0.1);
        letter_n(&mut board, (114.0, 201.5), 0.1);
        assert!(
            board.doc().strokes().all(|s| s.fix.is_none()),
            "open word stays raw"
        );
        let depth = board.undo_depth();
        board.flush_word(board.beautify.generation);
        assert!(board.doc().strokes().all(|s| s.fix.is_some()));
        assert_eq!(board.undo_depth(), depth, "no undo step of its own");
        assert!(board.undo());
        assert_eq!(board.doc().stroke_count(), 1);
        assert!(board.doc().strokes().all(|s| s.fix.is_none()));
    }

    #[test]
    fn a_stroke_far_away_closes_the_previous_word() {
        let mut board = pen_board();
        letter_n(&mut board, (100.0, 200.0), 0.0);
        letter_n(&mut board, (400.0, 200.0), 0.0);
        let fixed = board.doc().strokes().filter(|s| s.fix.is_some()).count();
        assert_eq!(fixed, 1);
    }

    #[test]
    fn stale_timer_leaves_the_word_open() {
        let mut board = pen_board();
        letter_n(&mut board, (100.0, 200.0), 0.0);
        let stale = board.beautify.generation;
        letter_n(&mut board, (114.0, 200.0), 0.0);
        board.flush_word(stale);
        assert!(board.doc().strokes().all(|s| s.fix.is_none()));
    }

    #[test]
    fn switched_off_leaves_ink_alone() {
        let mut board = pen_board();
        board.set_beautify_options(super::Options {
            writing: false,
            ..super::Options::default()
        });
        letter_n(&mut board, (100.0, 200.0), 0.1);
        board.flush_word(board.beautify.generation);
        assert!(board.doc().strokes().all(|s| s.fix.is_none()));
    }

    #[test]
    fn drawings_are_not_words() {
        let mut board = pen_board();
        board.begin(InkPoint::new(0.0, 0.0, 128, 0, 0));
        board.extend(InkPoint::new(300.0, 250.0, 128, 0, 0));
        board.end();
        board.flush_word(board.beautify.generation);
        assert!(board.doc().strokes().all(|s| s.fix.is_none()));
    }
}
