//! Freya canvas widget + shared drawing board.
//!
//! [`Board`] is the process-wide state — a list of committed strokes
//! plus the one under construction. It is `Arc<Mutex<Board>>` so the
//! Freya render closure and the [`crate::pen_pump`] background thread
//! both touch the same buffer.
//!
//! Input is single-sourced from `istmo-pen` — freya mouse / touch
//! handlers are intentionally NOT wired here. Every sample comes from
//! [`crate::pen_pump`], which drains the platform pen backend (Android
//! `PenCaptureView`, iOS `PenCaptureView`, Linux libinput) and pushes
//! into [`Board`]. Canvas paints in element-local coordinates; the
//! layout keeps the drawing surface fullscreen at `(0, 0)` so pen
//! samples in decor-view / fullscreen coordinates land on the same
//! pixels the finger / stylus touched.

use std::sync::{Arc, Mutex, OnceLock};

use flume::{Receiver, Sender};
use freya::prelude::*;
use freya_engine::prelude::{BlendMode, Color as SkColor, Paint, PaintStyle};

use crate::brush::{Brush, InkPoint, SegmentMode, SegmentPlan};

/// Coalescing wakeup handle.
///
/// Board mutations `ping` it; the root component drains the receiver
/// and asks Freya's platform for a window redraw. Bounded(1) so bursts
/// collapse to one wakeup — the next frame will paint the freshest
/// state anyway.
#[derive(Debug, Clone)]
pub struct RedrawNotifier(Sender<()>);

impl RedrawNotifier {
    #[must_use]
    pub fn new() -> (Self, Receiver<()>) {
        let (tx, rx) = flume::bounded(1);
        (Self(tx), rx)
    }

    fn ping(&self) {
        let _ = self.0.try_send(());
    }
}

/// One drawn stroke — the brush that painted it plus every point the
/// input device produced. Kept as raw points (not pre-baked segments)
/// so the canvas can re-render at any zoom or resolution.
#[derive(Debug, Clone)]
pub struct Stroke {
    pub brush: Brush,
    pub points: Vec<InkPoint>,
}

impl Stroke {
    fn new(brush: Brush, first: InkPoint) -> Self {
        Self {
            brush,
            points: vec![first],
        }
    }
}

/// Shared drawing state. Cheap to clone the [`Arc`] into any handler.
#[derive(Debug, Default)]
pub struct Board {
    strokes: Vec<Stroke>,
    active: Option<Stroke>,
    brush: Brush,
    notifier: Option<RedrawNotifier>,
}

static SHARED: OnceLock<Arc<Mutex<Board>>> = OnceLock::new();

impl Board {
    /// Process-wide board. Freya's root component and [`crate::pen_pump`]
    /// share this same instance so pen input and the render callback
    /// touch the same stroke buffer.
    #[must_use]
    pub fn shared() -> Arc<Mutex<Self>> {
        Arc::clone(SHARED.get_or_init(|| Arc::new(Mutex::new(Self::default()))))
    }

    /// Install the wakeup handle used to request repaints after each
    /// mutation. Called once at startup by the root component. Safe to
    /// re-install (replaces the previous notifier).
    pub fn set_notifier(&mut self, notifier: RedrawNotifier) {
        self.notifier = Some(notifier);
    }

    fn notify(&self) {
        if let Some(n) = &self.notifier {
            n.ping();
        }
    }

    pub const fn set_brush(&mut self, brush: Brush) {
        self.brush = brush;
    }

    #[must_use]
    pub const fn brush(&self) -> Brush {
        self.brush
    }

    pub fn begin(&mut self, point: InkPoint) {
        if let Some(active) = self.active.take() {
            self.strokes.push(active);
        }
        self.active = Some(Stroke::new(self.brush, point));
        self.notify();
    }

    pub fn extend(&mut self, point: InkPoint) {
        if let Some(active) = self.active.as_mut() {
            active.points.push(point);
            self.notify();
        }
    }

    pub fn end(&mut self) {
        if let Some(active) = self.active.take() {
            if !active.points.is_empty() {
                self.strokes.push(active);
            }
            self.notify();
        }
    }

    pub fn cancel(&mut self) {
        if self.active.take().is_some() {
            self.notify();
        }
    }

    pub fn clear(&mut self) {
        self.strokes.clear();
        self.active = None;
        self.notify();
    }

    /// Paint every committed stroke plus the active one onto `canvas`.
    /// A stable order is preserved so the eraser's `Clear` paint only
    /// wipes strokes drawn *before* it — matching the visual mental
    /// model of a physical eraser.
    pub fn paint(&self, canvas: &freya_engine::prelude::Canvas) {
        for stroke in &self.strokes {
            paint_stroke(canvas, stroke);
        }
        if let Some(active) = &self.active {
            paint_stroke(canvas, active);
        }
    }
}

fn paint_stroke(canvas: &freya_engine::prelude::Canvas, stroke: &Stroke) {
    if stroke.points.len() < 2 {
        if let Some(only) = stroke.points.first() {
            paint_dot(canvas, stroke.brush, *only);
        }
        return;
    }
    for pair in stroke.points.windows(2) {
        let plan = stroke.brush.plan(pair[0], pair[1]);
        paint_segment(canvas, pair[0], pair[1], &plan);
    }
}

fn paint_dot(canvas: &freya_engine::prelude::Canvas, brush: Brush, point: InkPoint) {
    // A single-sample stroke still gets rendered as a filled circle so
    // taps register visibly. Reuse the same plan the segment path uses
    // for consistent width scaling.
    let plan = brush.plan(point, point);
    let mut dot = Paint::default();
    configure_paint(&mut dot, &plan);
    dot.set_style(PaintStyle::Fill);
    canvas.draw_circle((point.x, point.y), plan.width * 0.5, &dot);
}

fn paint_segment(
    canvas: &freya_engine::prelude::Canvas,
    from: InkPoint,
    to: InkPoint,
    plan: &SegmentPlan,
) {
    let mut paint = Paint::default();
    configure_paint(&mut paint, plan);
    canvas.draw_line((from.x, from.y), (to.x, to.y), &paint);
}

fn configure_paint(paint: &mut Paint, plan: &SegmentPlan) {
    paint.set_anti_alias(true);
    paint.set_style(PaintStyle::Stroke);
    paint.set_stroke_width(plan.width);

    let sk_color: SkColor = plan.color.into();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let alpha = (plan.opacity.clamp(0.0, 1.0) * 255.0) as u8;
    paint.set_color(sk_color.with_a(alpha));

    paint.set_blend_mode(match plan.mode {
        SegmentMode::Draw => BlendMode::SrcOver,
        SegmentMode::Multiply => BlendMode::Multiply,
        SegmentMode::Erase => BlendMode::Clear,
    });
}

/// Freya canvas element wired to a shared [`Board`].
///
/// The render closure captures the [`Arc`] and re-locks it every frame
/// — since `RenderCallback::eq` always returns `true`, we cannot rely
/// on Freya noticing state changes any other way.
///
/// No pointer / touch handlers here on purpose: every stroke sample
/// comes from `istmo-pen` via [`crate::pen_pump`]. That keeps a single
/// coordinate system in play (decor-view / fullscreen dp) and avoids
/// double-injecting a stroke when the OS also delivers touch to the
/// freya widget tree.
pub fn drawing_surface(board: &Arc<Mutex<Board>>) -> impl IntoElement {
    let render_board = Arc::clone(board);

    let inner = canvas(RenderCallback::new(move |ctx| {
        let guard = match render_board.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.paint(ctx.canvas);
    }))
    .width(Size::fill())
    .height(Size::fill());

    rect()
        .width(Size::fill())
        .height(Size::fill())
        .background(Color::from_rgb(250, 250, 248))
        .child(inner)
}

/// Lock the shared board, recovering transparently from mutex
/// poisoning. Every mutation site is a short vec push with no
/// panic-prone code — forward progress beats an unactionable error.
pub fn lock(board: &Arc<Mutex<Board>>) -> std::sync::MutexGuard<'_, Board> {
    match board.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}
