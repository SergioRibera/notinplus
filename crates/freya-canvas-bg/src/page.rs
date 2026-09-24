//! Per-page renderers for paginated backgrounds.
//!
//! A [`PageProvider`] paints exactly one page inside a world-space
//! rectangle handed down by a composite background (e.g.
//! [`StackedPagesBackground`](crate::StackedPagesBackground)).
//!
//! The two shapes shipped here — [`SolidPageProvider`] for blank
//! drawing paper and the [`paper`] module for standard sizes — cover
//! the everyday "notebook with pages" case without any external deps.
//! Third-party backends (PDF, image tiles) implement the trait
//! themselves and slot into the same composite.

use freya_engine::prelude::{Paint, PaintStyle, Rect as SkRect};

use crate::{Canvas, Color, Rect};

/// Optional page border painted on top of the fill.
#[derive(Debug, Clone, Copy)]
pub struct PageBorder {
    /// Stroke color.
    pub color: Color,
    /// Stroke width in world units.
    pub width: f32,
}

/// Overlay pattern rendered inside the page fill.
#[derive(Debug, Clone, Copy)]
pub enum Grid {
    /// Orthogonal ruled grid.
    Lines {
        /// Distance between adjacent lines in world units.
        spacing: f32,
        /// Line color.
        color: Color,
        /// Line width in world units.
        width: f32,
    },
    /// Dotted grid (dot at every `spacing × spacing` lattice point).
    Dots {
        /// Distance between adjacent dots along each axis.
        spacing: f32,
        /// Dot color.
        color: Color,
        /// Dot radius in world units.
        radius: f32,
    },
}

/// Blank drawing paper. Owns a fill color, an optional [`Grid`], and
/// an optional [`PageBorder`].
#[derive(Debug, Clone)]
pub struct SolidPageProvider {
    size: (f32, f32),
    fill: Color,
    border: Option<PageBorder>,
    grid: Option<Grid>,
}

impl SolidPageProvider {
    /// New provider with the given world-space size and fill color.
    #[must_use]
    pub const fn new(size: (f32, f32), fill: Color) -> Self {
        Self { size, fill, border: None, grid: None }
    }

    /// Attach a border overlay.
    #[must_use]
    pub const fn with_border(mut self, border: PageBorder) -> Self {
        self.border = Some(border);
        self
    }

    /// Attach a grid overlay.
    #[must_use]
    pub const fn with_grid(mut self, grid: Grid) -> Self {
        self.grid = Some(grid);
        self
    }
}

/// One page's worth of world-space rendering.
///
/// A [`crate::CanvasBackground`] composite (or any host) hands the
/// implementation a world-space rectangle and current zoom scale; the
/// provider paints the page contents inside that rectangle.
///
/// Implementations must be thread-safe so composite backgrounds can
/// hand them across worker threads.
pub trait PageProvider: Send + Sync + std::fmt::Debug {
    /// Natural size of this page in world units. Composite layouts
    /// use it to compute where the page sits inside the stack.
    fn natural_size(&self) -> (f32, f32);

    /// Paint the page inside `rect` (world coords). `scale` is the
    /// current viewport zoom — implementations that cache bitmaps use
    /// it to pick a level of detail.
    fn paint(&self, canvas: &Canvas, rect: Rect, scale: f32);
}

impl PageProvider for SolidPageProvider {
    fn natural_size(&self) -> (f32, f32) {
        self.size
    }

    fn paint(&self, canvas: &Canvas, rect: Rect, _scale: f32) {
        let sk_rect = SkRect::from_ltrb(rect.min_x, rect.min_y, rect.max_x, rect.max_y);

        let mut fill = Paint::default();
        fill.set_color(self.fill).set_style(PaintStyle::Fill).set_anti_alias(true);
        canvas.draw_rect(sk_rect, &fill);

        if let Some(grid) = self.grid {
            canvas.save();
            canvas.clip_rect(sk_rect, None, Some(true));
            paint_grid(canvas, rect, grid);
            canvas.restore();
        }

        if let Some(border) = self.border {
            let mut stroke = Paint::default();
            stroke
                .set_color(border.color)
                .set_style(PaintStyle::Stroke)
                .set_stroke_width(border.width)
                .set_anti_alias(true);
            canvas.draw_rect(sk_rect, &stroke);
        }
    }
}

#[allow(clippy::cast_precision_loss)] // grid indices never approach f32 mantissa limit
fn paint_grid(canvas: &Canvas, rect: Rect, grid: Grid) {
    match grid {
        Grid::Lines { spacing, color, width } => {
            if spacing <= 0.0 || width <= 0.0 {
                return;
            }
            let mut paint = Paint::default();
            paint
                .set_color(color)
                .set_style(PaintStyle::Stroke)
                .set_stroke_width(width)
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
        Grid::Dots { spacing, color, radius } => {
            if spacing <= 0.0 || radius <= 0.0 {
                return;
            }
            let mut paint = Paint::default();
            paint.set_color(color).set_style(PaintStyle::Fill).set_anti_alias(true);
            let (start_x, count_x) = grid_axis(rect.min_x, rect.max_x, spacing);
            let (start_y, count_y) = grid_axis(rect.min_y, rect.max_y, spacing);
            for j in 0..count_y {
                let y = (j as f32).mul_add(spacing, start_y);
                for i in 0..count_x {
                    let x = (i as f32).mul_add(spacing, start_x);
                    canvas.draw_circle((x, y), radius, &paint);
                }
            }
        }
    }
}

/// First grid stop on `[min, max)` aligned to `spacing` + the number
/// of stops that fit. Iterating `i` over `0..count` and computing
/// `start + i * spacing` avoids the float-drift `clippy::while_float`
/// flags on the naive `while x < max { x += spacing; }` loop.
fn grid_axis(min: f32, max: f32, spacing: f32) -> (f32, usize) {
    let start = (min / spacing).ceil() * spacing;
    let extent = max - start;
    if extent <= 0.0 {
        return (start, 0);
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let count = (extent / spacing).ceil() as usize;
    (start, count)
}

/// Standard paper sizes in PostScript points (72 dpi units).
///
/// The exposed factories return a [`SolidPageProvider`] with a subtle
/// 1-point translucent black border so pages stay visually distinct
/// against the viewer backdrop.
pub mod paper {
    use super::{Grid, PageBorder, SolidPageProvider};
    use crate::Color;

    /// A4 portrait — 210 × 297 mm expressed in points.
    pub const A4: (f32, f32) = (595.0, 842.0);
    /// US Letter portrait — 8.5 × 11 in.
    pub const LETTER: (f32, f32) = (612.0, 792.0);
    /// US Legal portrait — 8.5 × 14 in.
    pub const LEGAL: (f32, f32) = (612.0, 1_008.0);
    /// A5 portrait — 148 × 210 mm.
    pub const A5: (f32, f32) = (420.0, 595.0);

    const fn default_border() -> PageBorder {
        PageBorder { color: Color::from_argb(60, 0, 0, 0), width: 1.0 }
    }

    /// White A4 page with a subtle border.
    #[must_use]
    pub const fn a4() -> SolidPageProvider {
        SolidPageProvider::new(A4, Color::from_rgb(255, 255, 255)).with_border(default_border())
    }

    /// White US Letter page with a subtle border.
    #[must_use]
    pub const fn letter() -> SolidPageProvider {
        SolidPageProvider::new(LETTER, Color::from_rgb(255, 255, 255))
            .with_border(default_border())
    }

    /// White US Legal page with a subtle border.
    #[must_use]
    pub const fn legal() -> SolidPageProvider {
        SolidPageProvider::new(LEGAL, Color::from_rgb(255, 255, 255))
            .with_border(default_border())
    }

    /// White A5 page with a subtle border.
    #[must_use]
    pub const fn a5() -> SolidPageProvider {
        SolidPageProvider::new(A5, Color::from_rgb(255, 255, 255)).with_border(default_border())
    }

    /// A4 page with a 20pt ruled grid.
    #[must_use]
    pub const fn a4_ruled() -> SolidPageProvider {
        a4().with_grid(Grid::Lines {
            spacing: 20.0,
            color: Color::from_argb(40, 60, 90, 140),
            width: 0.5,
        })
    }

    /// A4 page with a 20pt dotted grid.
    #[must_use]
    pub const fn a4_dotted() -> SolidPageProvider {
        a4().with_grid(Grid::Dots {
            spacing: 20.0,
            color: Color::from_argb(80, 60, 60, 60),
            radius: 0.6,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solid_provider_reports_size() {
        let p = SolidPageProvider::new((100.0, 200.0), Color::from_rgb(255, 255, 255));
        assert_eq!(p.natural_size(), (100.0, 200.0));
    }

    #[test]
    fn paper_helpers_have_expected_sizes() {
        assert_eq!(paper::A4, (595.0, 842.0));
        assert_eq!(paper::LETTER, (612.0, 792.0));
        assert_eq!(paper::LEGAL, (612.0, 1_008.0));
        assert_eq!(paper::a4().natural_size(), paper::A4);
    }
}
