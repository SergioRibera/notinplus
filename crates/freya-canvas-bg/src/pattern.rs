//! Infinite-canvas pattern backgrounds: horizontal ruled lines, orthogonal
//! grids, and dotted grids.
//!
//! Each background carries a solid surface color (surfaced via
//! [`CanvasBackground::viewport_backdrop`] so the host paints it in one pass
//! outside the world transform) and draws its pattern lazily inside the
//! visible rect during [`CanvasBackground::paint`]. Content is unbounded —
//! panning is not clamped.
//!
//! Pattern parameters (spacing, color, stroke width, dot radius) live on
//! each background and default to values chosen to look like generic
//! notebook paper at 1:1 zoom.

use freya_engine::prelude::{Paint, PaintStyle, Rect as SkRect};

use crate::{BgPaintCtx, CanvasBackground, Color, Rect};

/// Default paper tint used when the caller does not override it.
pub const DEFAULT_PAPER: Color = Color::from_rgb(250, 250, 248);
/// Default translucent ink used for both line and dot patterns.
pub const DEFAULT_INK: Color = Color::from_argb(60, 60, 90, 140);
/// Default pattern pitch in world units. Matches the ruled/dotted paper
/// defaults exposed under [`crate::page::paper`].
pub const DEFAULT_SPACING: f32 = 24.0;

/// Horizontal ruled lines on an unbounded surface.
#[derive(Debug, Clone, Copy)]
pub struct LinedBackground {
    surface: Color,
    ink: Color,
    spacing: f32,
    width: f32,
}

impl LinedBackground {
    /// Build with the given surface color and default ruling.
    #[must_use]
    pub const fn new(surface: Color) -> Self {
        Self {
            surface,
            ink: DEFAULT_INK,
            spacing: DEFAULT_SPACING,
            width: 0.5,
        }
    }

    /// Override ink color, spacing (world units), and stroke width.
    #[must_use]
    pub const fn with_ruling(mut self, ink: Color, spacing: f32, width: f32) -> Self {
        self.ink = ink;
        self.spacing = spacing;
        self.width = width;
        self
    }
}

impl Default for LinedBackground {
    fn default() -> Self {
        Self::new(DEFAULT_PAPER)
    }
}

impl CanvasBackground for LinedBackground {
    fn viewport_backdrop(&self) -> Option<Color> {
        Some(self.surface)
    }

    #[allow(clippy::cast_precision_loss)]
    fn paint(&self, cx: &mut BgPaintCtx<'_>) {
        if self.spacing <= 0.0 || self.width <= 0.0 {
            return;
        }
        let mut paint = Paint::default();
        paint
            .set_color(self.ink)
            .set_style(PaintStyle::Stroke)
            .set_stroke_width(self.width)
            .set_anti_alias(true);
        let (start_y, count_y) = grid_axis(cx.visible.min_y, cx.visible.max_y, self.spacing);
        for j in 0..count_y {
            let y = (j as f32).mul_add(self.spacing, start_y);
            cx.canvas
                .draw_line((cx.visible.min_x, y), (cx.visible.max_x, y), &paint);
        }
    }
}

/// Orthogonal grid on an unbounded surface.
#[derive(Debug, Clone, Copy)]
pub struct GridBackground {
    surface: Color,
    ink: Color,
    spacing: f32,
    width: f32,
}

impl GridBackground {
    /// Build with the given surface color and default grid.
    #[must_use]
    pub const fn new(surface: Color) -> Self {
        Self {
            surface,
            ink: DEFAULT_INK,
            spacing: DEFAULT_SPACING,
            width: 0.5,
        }
    }

    /// Override ink color, spacing (world units), and stroke width.
    #[must_use]
    pub const fn with_grid(mut self, ink: Color, spacing: f32, width: f32) -> Self {
        self.ink = ink;
        self.spacing = spacing;
        self.width = width;
        self
    }
}

impl Default for GridBackground {
    fn default() -> Self {
        Self::new(DEFAULT_PAPER)
    }
}

impl CanvasBackground for GridBackground {
    fn viewport_backdrop(&self) -> Option<Color> {
        Some(self.surface)
    }

    #[allow(clippy::cast_precision_loss)]
    fn paint(&self, cx: &mut BgPaintCtx<'_>) {
        if self.spacing <= 0.0 || self.width <= 0.0 {
            return;
        }
        let mut paint = Paint::default();
        paint
            .set_color(self.ink)
            .set_style(PaintStyle::Stroke)
            .set_stroke_width(self.width)
            .set_anti_alias(true);
        let (start_x, count_x) = grid_axis(cx.visible.min_x, cx.visible.max_x, self.spacing);
        for i in 0..count_x {
            let x = (i as f32).mul_add(self.spacing, start_x);
            cx.canvas
                .draw_line((x, cx.visible.min_y), (x, cx.visible.max_y), &paint);
        }
        let (start_y, count_y) = grid_axis(cx.visible.min_y, cx.visible.max_y, self.spacing);
        for j in 0..count_y {
            let y = (j as f32).mul_add(self.spacing, start_y);
            cx.canvas
                .draw_line((cx.visible.min_x, y), (cx.visible.max_x, y), &paint);
        }
    }
}

/// Dotted grid on an unbounded surface.
#[derive(Debug, Clone, Copy)]
pub struct DotGridBackground {
    surface: Color,
    ink: Color,
    spacing: f32,
    radius: f32,
}

impl DotGridBackground {
    /// Build with the given surface color and default dots.
    #[must_use]
    pub const fn new(surface: Color) -> Self {
        Self {
            surface,
            ink: Color::from_argb(120, 60, 60, 60),
            spacing: DEFAULT_SPACING,
            radius: 0.9,
        }
    }

    /// Override ink color, spacing (world units), and dot radius.
    #[must_use]
    pub const fn with_dots(mut self, ink: Color, spacing: f32, radius: f32) -> Self {
        self.ink = ink;
        self.spacing = spacing;
        self.radius = radius;
        self
    }
}

impl Default for DotGridBackground {
    fn default() -> Self {
        Self::new(DEFAULT_PAPER)
    }
}

impl CanvasBackground for DotGridBackground {
    fn viewport_backdrop(&self) -> Option<Color> {
        Some(self.surface)
    }

    #[allow(clippy::cast_precision_loss)]
    fn paint(&self, cx: &mut BgPaintCtx<'_>) {
        if self.spacing <= 0.0 || self.radius <= 0.0 {
            return;
        }
        let mut paint = Paint::default();
        paint
            .set_color(self.ink)
            .set_style(PaintStyle::Fill)
            .set_anti_alias(true);
        let (start_x, count_x) = grid_axis(cx.visible.min_x, cx.visible.max_x, self.spacing);
        let (start_y, count_y) = grid_axis(cx.visible.min_y, cx.visible.max_y, self.spacing);
        for j in 0..count_y {
            let y = (j as f32).mul_add(self.spacing, start_y);
            for i in 0..count_x {
                let x = (i as f32).mul_add(self.spacing, start_x);
                cx.canvas.draw_circle((x, y), self.radius, &paint);
            }
        }
    }
}

/// Paint any pattern into `rect`, clipping to it. Handy for previews
/// (thumbnails, sheet cards) that want to reuse the exact same rendering
/// logic used by the full-screen background.
///
/// The surface color is filled first, then the pattern is drawn on top.
#[derive(Debug, Clone, Copy)]
pub enum PatternKind {
    /// No overlay — surface only.
    Blank,
    /// Horizontal ruled lines.
    Line,
    /// Orthogonal grid.
    Grid,
    /// Dotted grid.
    DotGrid,
}

/// Fill `rect` with `surface` then overlay `kind` using the same math the
/// unbounded backgrounds use. Intended for small preview widgets — the
/// full-screen canvas uses the dedicated background impls above.
#[allow(clippy::cast_precision_loss)]
pub fn paint_preview(
    canvas: &crate::Canvas,
    rect: Rect,
    surface: Color,
    ink: Color,
    kind: PatternKind,
    spacing: f32,
) {
    let sk_rect = SkRect::from_ltrb(rect.min_x, rect.min_y, rect.max_x, rect.max_y);
    let mut fill = Paint::default();
    fill.set_color(surface)
        .set_style(PaintStyle::Fill)
        .set_anti_alias(true);
    canvas.draw_rect(sk_rect, &fill);

    if matches!(kind, PatternKind::Blank) || spacing <= 0.0 {
        return;
    }

    canvas.save();
    canvas.clip_rect(sk_rect, None, Some(true));
    match kind {
        PatternKind::Blank => {}
        PatternKind::Line => {
            let mut paint = Paint::default();
            paint
                .set_color(ink)
                .set_style(PaintStyle::Stroke)
                .set_stroke_width(0.75)
                .set_anti_alias(true);
            let (start_y, count_y) = grid_axis(rect.min_y, rect.max_y, spacing);
            for j in 0..count_y {
                let y = (j as f32).mul_add(spacing, start_y);
                canvas.draw_line((rect.min_x, y), (rect.max_x, y), &paint);
            }
        }
        PatternKind::Grid => {
            let mut paint = Paint::default();
            paint
                .set_color(ink)
                .set_style(PaintStyle::Stroke)
                .set_stroke_width(0.75)
                .set_anti_alias(true);
            let (start_x, count_x) = grid_axis(rect.min_x, rect.max_x, spacing);
            for i in 0..count_x {
                let x = (i as f32).mul_add(spacing, start_x);
                canvas.draw_line((x, rect.min_y), (x, rect.max_y), &paint);
            }
            let (start_y, count_y) = grid_axis(rect.min_y, rect.max_y, spacing);
            for j in 0..count_y {
                let y = (j as f32).mul_add(spacing, start_y);
                canvas.draw_line((rect.min_x, y), (rect.max_x, y), &paint);
            }
        }
        PatternKind::DotGrid => {
            let mut paint = Paint::default();
            paint
                .set_color(ink)
                .set_style(PaintStyle::Fill)
                .set_anti_alias(true);
            let (start_x, count_x) = grid_axis(rect.min_x, rect.max_x, spacing);
            let (start_y, count_y) = grid_axis(rect.min_y, rect.max_y, spacing);
            for j in 0..count_y {
                let y = (j as f32).mul_add(spacing, start_y);
                for i in 0..count_x {
                    let x = (i as f32).mul_add(spacing, start_x);
                    canvas.draw_circle((x, y), 1.1, &paint);
                }
            }
        }
    }
    canvas.restore();
}

#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn grid_axis(min: f32, max: f32, spacing: f32) -> (f32, usize) {
    let start = (min / spacing).ceil() * spacing;
    let extent = max - start;
    if extent <= 0.0 {
        return (start, 0);
    }
    let count = (extent / spacing).ceil() as usize;
    (start, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backgrounds_expose_backdrop_and_no_pages() {
        let l = LinedBackground::new(Color::from_rgb(1, 2, 3));
        let g = GridBackground::new(Color::from_rgb(4, 5, 6));
        let d = DotGridBackground::new(Color::from_rgb(7, 8, 9));
        assert_eq!(l.viewport_backdrop(), Some(Color::from_rgb(1, 2, 3)));
        assert_eq!(g.viewport_backdrop(), Some(Color::from_rgb(4, 5, 6)));
        assert_eq!(d.viewport_backdrop(), Some(Color::from_rgb(7, 8, 9)));
        assert!(l.pages().is_empty() && g.pages().is_empty() && d.pages().is_empty());
        assert!(l.content_bounds().is_none());
        assert!(g.content_bounds().is_none());
        assert!(d.content_bounds().is_none());
    }
}
