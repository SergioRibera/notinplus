//! Brush type system.
//!
//! Every drawing tool is a variant of [`Brush`]. Each variant owns its
//! calibration (width envelope, opacity, pressure response) as a
//! dedicated struct — the enum's shape encodes which parameters exist
//! for which tool, so a marker cannot accidentally borrow eraser fields
//! and vice versa.
//!
//! Sample colouring, opacity, and width all flow through [`Brush::plan`]
//! which returns a [`SegmentPlan`] describing how a single segment
//! between two [`InkPoint`]s should be painted. Callers hand the plan to
//! their renderer — the brushes themselves are backend-agnostic.

use freya::prelude::Color;

/// Normalised pressure sample. Zero when the input device reports no
/// pressure signal; consumers still receive a value so downstream code
/// never has to branch on presence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pressure(f32);

impl Pressure {
    #[must_use]
    pub const fn new(raw: f32) -> Self {
        Self(raw.clamp(0.0, 1.0))
    }

    #[must_use]
    pub const fn get(self) -> f32 {
        self.0
    }
}

impl Default for Pressure {
    fn default() -> Self {
        Self(0.5)
    }
}

impl From<f32> for Pressure {
    fn from(value: f32) -> Self {
        Self::new(value)
    }
}

/// A single point in an ink stroke. Coordinates are in Freya logical
/// space (points / dp), matching what the canvas widget receives from
/// mouse and pen events.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InkPoint {
    pub x: f32,
    pub y: f32,
    pub pressure: Pressure,
    pub tilt: f32,
}

impl InkPoint {
    #[must_use]
    pub const fn new(x: f32, y: f32, pressure: Pressure, tilt: f32) -> Self {
        Self {
            x,
            y,
            pressure,
            tilt,
        }
    }

    #[must_use]
    pub fn distance_to(self, other: Self) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx.hypot(dy)
    }
}

/// Instructions for rendering one segment of a stroke.
///
/// Produced by [`Brush::plan`]. The canvas layer turns this into skia
/// paint calls; keeping the plan renderer-agnostic lets tests and
/// non-skia targets exercise brush logic without pulling in the whole
/// rendering stack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentPlan {
    pub color: Color,
    pub width: f32,
    pub opacity: f32,
    pub mode: SegmentMode,
}

/// How a segment interacts with what is already on the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentMode {
    /// Standard source-over paint.
    Draw,
    /// Multiply-style paint so overlapping colours darken naturally —
    /// the highlighter effect.
    Multiply,
    /// Clear pixels underneath. Consumers translate this to the
    /// backend's cleanest erase primitive (skia `BlendMode::Clear` or a
    /// destination-out paint).
    Erase,
}

/// A drawing tool. Each variant carries its own calibration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Brush {
    /// Thin, grainy line with light pressure sensitivity. The go-to for
    /// sketching before committing to ink.
    Pencil(PencilStyle),
    /// Broad, opaque strokes with strong pressure and tilt response.
    /// The traditional "pincel" of a paint app.
    Marker(MarkerStyle),
    /// Removes ink. Width follows pressure directly so the user can
    /// scrub finer or broader without switching tools.
    Eraser(EraserStyle),
    /// Constant-weight ink line — the fountain pen / bolígrafo. Pressure
    /// modulates a narrow band; tilt is ignored so signature-style
    /// strokes stay predictable.
    Pen(PenStyle),
    /// Semi-transparent, wide multiplicative stroke. Overlaps darken;
    /// pressure does not change opacity so the user can lay down flat
    /// blocks of colour.
    Highlighter(HighlighterStyle),
}

impl Default for Brush {
    fn default() -> Self {
        Self::Pen(PenStyle::default())
    }
}

/// Human-readable name — for palette buttons and debug output.
impl Brush {
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Pencil(_) => "Pencil",
            Self::Marker(_) => "Marker",
            Self::Eraser(_) => "Eraser",
            Self::Pen(_) => "Pen",
            Self::Highlighter(_) => "Highlighter",
        }
    }

    /// Return the tool's active colour. Eraser has none — it clears
    /// pixels — so the palette can grey out the colour picker.
    #[must_use]
    pub const fn color(&self) -> Option<Color> {
        match self {
            Self::Pencil(s) => Some(s.color),
            Self::Marker(s) => Some(s.color),
            Self::Pen(s) => Some(s.color),
            Self::Highlighter(s) => Some(s.color),
            Self::Eraser(_) => None,
        }
    }

    /// Replace the tool's colour if it has one. Eraser calls are no-ops.
    pub const fn set_color(&mut self, color: Color) {
        match self {
            Self::Pencil(s) => s.color = color,
            Self::Marker(s) => s.color = color,
            Self::Pen(s) => s.color = color,
            Self::Highlighter(s) => s.color = color,
            Self::Eraser(_) => {}
        }
    }

    /// Plan how to paint the segment from `from` to `to`. The pressure
    /// used for width interpolation is the average of both endpoints —
    /// smoother than picking one endpoint arbitrarily.
    #[must_use]
    pub fn plan(&self, from: InkPoint, to: InkPoint) -> SegmentPlan {
        let pressure = (from.pressure.get() + to.pressure.get()) * 0.5;
        let tilt = (from.tilt + to.tilt) * 0.5;
        match *self {
            Self::Pencil(style) => style.plan(pressure, tilt),
            Self::Marker(style) => style.plan(pressure, tilt),
            Self::Eraser(style) => style.plan(pressure),
            Self::Pen(style) => style.plan(pressure),
            Self::Highlighter(style) => style.plan(),
        }
    }
}

fn lerp(min: f32, max: f32, t: f32) -> f32 {
    max.mul_add(t, min * (1.0 - t))
}

/// Calibration for [`Brush::Pencil`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PencilStyle {
    pub color: Color,
    pub min_width: f32,
    pub max_width: f32,
    pub tilt_gain: f32,
    pub base_opacity: f32,
}

impl Default for PencilStyle {
    fn default() -> Self {
        Self {
            color: Color::from_rgb(60, 60, 60),
            min_width: 0.8,
            max_width: 2.2,
            tilt_gain: 0.6,
            base_opacity: 0.78,
        }
    }
}

impl PencilStyle {
    fn plan(self, pressure: f32, tilt: f32) -> SegmentPlan {
        let width = self
            .tilt_gain
            .mul_add(tilt, lerp(self.min_width, self.max_width, pressure));
        SegmentPlan {
            color: self.color,
            width,
            opacity: self.base_opacity,
            mode: SegmentMode::Draw,
        }
    }
}

/// Calibration for [`Brush::Marker`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarkerStyle {
    pub color: Color,
    pub min_width: f32,
    pub max_width: f32,
    pub tilt_gain: f32,
}

impl Default for MarkerStyle {
    fn default() -> Self {
        Self {
            color: Color::from_rgb(20, 20, 20),
            min_width: 3.0,
            max_width: 14.0,
            tilt_gain: 4.0,
        }
    }
}

impl MarkerStyle {
    fn plan(self, pressure: f32, tilt: f32) -> SegmentPlan {
        let width = self
            .tilt_gain
            .mul_add(tilt, lerp(self.min_width, self.max_width, pressure));
        SegmentPlan {
            color: self.color,
            width,
            opacity: 1.0,
            mode: SegmentMode::Draw,
        }
    }
}

/// Calibration for [`Brush::Eraser`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EraserStyle {
    pub min_width: f32,
    pub max_width: f32,
}

impl Default for EraserStyle {
    fn default() -> Self {
        Self {
            min_width: 6.0,
            max_width: 24.0,
        }
    }
}

impl EraserStyle {
    fn plan(self, pressure: f32) -> SegmentPlan {
        SegmentPlan {
            color: Color::TRANSPARENT,
            width: lerp(self.min_width, self.max_width, pressure),
            opacity: 1.0,
            mode: SegmentMode::Erase,
        }
    }
}

/// Calibration for [`Brush::Pen`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PenStyle {
    pub color: Color,
    pub min_width: f32,
    pub max_width: f32,
}

impl Default for PenStyle {
    fn default() -> Self {
        Self {
            color: Color::from_rgb(10, 15, 40),
            min_width: 1.4,
            max_width: 2.6,
        }
    }
}

impl PenStyle {
    fn plan(self, pressure: f32) -> SegmentPlan {
        SegmentPlan {
            color: self.color,
            width: lerp(self.min_width, self.max_width, pressure),
            opacity: 1.0,
            mode: SegmentMode::Draw,
        }
    }
}

/// Calibration for [`Brush::Highlighter`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HighlighterStyle {
    pub color: Color,
    pub width: f32,
    pub opacity: f32,
}

impl Default for HighlighterStyle {
    fn default() -> Self {
        Self {
            color: Color::from_rgb(255, 220, 60),
            width: 16.0,
            opacity: 0.35,
        }
    }
}

impl HighlighterStyle {
    const fn plan(self) -> SegmentPlan {
        SegmentPlan {
            color: self.color,
            width: self.width,
            opacity: self.opacity,
            mode: SegmentMode::Multiply,
        }
    }
}
