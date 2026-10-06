//! Brush type system.
//!
//! A [`BrushPreset`] fully describes a drawing tool (kind + default
//! color + width envelope + opacity + spacing). Each stroke references
//! a preset via a compact [`BrushId`] into the document's shared
//! registry and carries its own instance-specific color override —
//! authoring an "orange marker" does not fork the marker preset.
//!
//! All wire types (`BrushId`, `BrushKind`, `BrushPreset`, `InkPoint`,
//! `Stroke`) are `#[istmo::message]` so a document round-trips through
//! `bincode` without an intermediate DTO layer.

use freya::prelude::Color;

use crate::ids::StrokeId;

/// Zero-based index into [`crate::doc::Doc::brushes`]. `u16` caps
/// per-document presets at 65 535 — orders of magnitude past any
/// realistic ceiling — and keeps every [`Stroke`] header at two bytes.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
pub struct BrushId(pub u16);

/// Which drawing tool family a preset belongs to. Determines rendering
/// mode (draw / multiply) and — post-Phase-3 — whether input is routed
/// to the paint path or the vector eraser.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum BrushKind {
    Pencil,
    Marker,
    Eraser,
    Pen,
    Highlighter,
    /// Third-party brush identity. External crates register a
    /// [`crate::render::BrushRenderer`] at this id in the
    /// [`crate::render::BrushRegistry`]; unknown ids fall back to the
    /// default ribbon renderer so an old doc with a plugin no longer
    /// loaded still opens.
    Custom(u16),
    /// Geometric primitive drawn from a small set of anchor points
    /// rather than a continuous polyline. The paired [`ShapeMode`]
    /// picks which primitive; every shape shares the same paint
    /// (stroke width + colour from the preset) but a different
    /// geometry builder in [`crate::render::ShapeBrush`].
    Shape(ShapeMode),
}

/// Geometric family selected when the brush kind is
/// [`BrushKind::Shape`].
///
/// Every mode consumes exactly two anchor samples: the pen-down
/// position (anchor A) and the current cursor (anchor B, updated
/// live during drag). Extending post-commit editing to N-point
/// polygons lives on a separate follow-up — the two-anchor rubber
/// band is enough for the primitives in this list.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum ShapeMode {
    /// Straight solid segment A→B.
    #[default]
    Line,
    /// Straight dashed segment A→B.
    Dashed,
    /// Solid segment A→B terminating in an arrowhead at B.
    Arrow,
    /// Axis-aligned outline rectangle with corners at A and B.
    Rect,
    /// Outline circle centred at A with radius `|AB|`.
    Circle,
}

/// Pressure→width shaping applied on top of the linear width envelope.
/// Kept on [`BrushConfig::Pen`] so a user can tune stylus feel without
/// mutating the shared preset.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PressureCurve {
    #[default]
    Linear,
    Soft,
    Hard,
}

impl PressureCurve {
    /// Reshape a normalised pressure sample. `Soft` bulges the mid-range
    /// up (γ<1), `Hard` bulges it down (γ>1). Input outside `[0, 1]` is
    /// clamped before shaping.
    #[must_use]
    pub fn apply(self, pressure: f32) -> f32 {
        let p = pressure.clamp(0.0, 1.0);
        match self {
            Self::Linear => p,
            Self::Soft => p.sqrt(),
            Self::Hard => p * p,
        }
    }
}

/// How the eraser consumes input samples. Governs whether a gesture
/// clips per-sample, selects a whole stroke on tap, or drags a
/// selection rectangle.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum EraserMode {
    /// Vector-erase circles at every sample; touched strokes split at
    /// the boundary. Default; matches the pre-config behaviour.
    #[default]
    Point,
    /// Tap → remove the topmost stroke under the cursor whole.
    Stroke,
    /// Drag a rectangle → remove every stroke intersecting it. The
    /// selection primitive is intentionally isolated so free-form
    /// selection can slot in later without touching call sites.
    SelectionRect,
}

/// Highlighter tip shape. `Bevel` picks the perpendicular offset from a
/// user-configured angle so a stroke drawn along the bevel direction
/// stays thin.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum HighlighterTip {
    Round,
    Bevel { angle_deg: f32 },
}

impl Default for HighlighterTip {
    fn default() -> Self {
        Self::Bevel { angle_deg: 45.0 }
    }
}

/// Per-variant tunables paired with a [`BrushKind`].
///
/// Intentionally outside the [`BrushPreset`] wire shape: presets travel
/// on disk (see [`crate::doc::Doc::save`]) and adding variant-scoped
/// fields there would churn the doc format every time the popup grows
/// a control. [`crate::canvas::Board`] owns a `BrushKind → BrushConfig`
/// map; UI writes through [`crate::canvas::Board::set_brush_config`],
/// renderers read via [`crate::canvas::Board::brush_config`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BrushConfig {
    /// Kinds with no per-variant knobs beyond colour/size.
    None,
    Pen {
        curve: PressureCurve,
    },
    Eraser {
        mode: EraserMode,
    },
    Highlighter {
        tip: HighlighterTip,
        straight: bool,
    },
}

impl BrushConfig {
    /// Baseline config for a kind. Every unset entry in
    /// [`crate::canvas::Board::brush_config`] resolves through this so
    /// callers never see an implicit "no config" that means different
    /// things per kind.
    #[must_use]
    pub const fn default_for(kind: BrushKind) -> Self {
        match kind {
            BrushKind::Pen => Self::Pen {
                curve: PressureCurve::Linear,
            },
            BrushKind::Eraser => Self::Eraser {
                mode: EraserMode::Point,
            },
            BrushKind::Highlighter => Self::Highlighter {
                tip: HighlighterTip::Round,
                straight: false,
            },
            BrushKind::Pencil | BrushKind::Marker | BrushKind::Custom(_) | BrushKind::Shape(_) => {
                Self::None
            }
        }
    }

    /// Which [`BrushKind`] this config's variant belongs to, or `None`
    /// for [`Self::None`] which is valid for every "no knobs" kind.
    #[must_use]
    pub const fn kind(&self) -> Option<BrushKind> {
        match self {
            Self::None => None,
            Self::Pen { .. } => Some(BrushKind::Pen),
            Self::Eraser { .. } => Some(BrushKind::Eraser),
            Self::Highlighter { .. } => Some(BrushKind::Highlighter),
        }
    }
}

/// Full calibration of a drawing tool. Everything the renderer needs to
/// paint a segment lives here; nothing tool-specific lives elsewhere.
///
/// `color` is the preset default (used to seed the swatch when a
/// palette entry is first picked). Actual stroke colour comes from
/// [`Stroke::color`], so a user can tint the same preset without
/// mutating the shared entry.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BrushPreset {
    pub kind: BrushKind,
    pub color: [u8; 4],
    pub min_width: f32,
    pub max_width: f32,
    pub tilt_gain: f32,
    pub opacity: f32,
    pub spacing: f32,
    /// User-tunable size multiplier. Applied on top of the width
    /// envelope so a Pencil at scale 2.0 draws twice as thick without
    /// forking the preset. Persisted with the doc so a reopened file
    /// re-renders identically.
    pub size_scale: f32,
}

impl Default for BrushPreset {
    fn default() -> Self {
        Self::pen()
    }
}

impl BrushPreset {
    #[must_use]
    pub const fn pencil() -> Self {
        Self {
            kind: BrushKind::Pencil,
            color: [60, 60, 60, 255],
            min_width: 0.8,
            max_width: 2.2,
            tilt_gain: 0.6,
            opacity: 0.78,
            spacing: 0.05,
            size_scale: 1.0,
        }
    }

    #[must_use]
    pub const fn marker() -> Self {
        Self {
            kind: BrushKind::Marker,
            color: [20, 20, 20, 255],
            min_width: 3.0,
            max_width: 14.0,
            tilt_gain: 4.0,
            opacity: 1.0,
            spacing: 0.08,
            size_scale: 1.0,
        }
    }

    #[must_use]
    pub const fn eraser() -> Self {
        Self {
            kind: BrushKind::Eraser,
            color: [0, 0, 0, 0],
            min_width: 6.0,
            max_width: 24.0,
            tilt_gain: 0.0,
            opacity: 1.0,
            spacing: 0.1,
            size_scale: 1.0,
        }
    }

    #[must_use]
    pub const fn pen() -> Self {
        Self {
            kind: BrushKind::Pen,
            color: [10, 15, 40, 255],
            min_width: 1.4,
            max_width: 2.6,
            tilt_gain: 0.0,
            opacity: 1.0,
            spacing: 0.05,
            size_scale: 1.0,
        }
    }

    #[must_use]
    pub const fn highlighter() -> Self {
        Self {
            kind: BrushKind::Highlighter,
            color: [255, 220, 60, 90],
            min_width: 16.0,
            max_width: 16.0,
            tilt_gain: 0.0,
            opacity: 0.35,
            spacing: 0.05,
            size_scale: 1.0,
        }
    }

    /// Baseline preset for the shape brush family. The width envelope
    /// stays flat — geometric primitives ignore pressure — and the
    /// colour matches the pen default so shapes drawn from the same
    /// palette entry read as annotation over freehand ink.
    #[must_use]
    pub const fn shape(mode: ShapeMode) -> Self {
        Self {
            kind: BrushKind::Shape(mode),
            color: [10, 15, 40, 255],
            min_width: 2.0,
            max_width: 2.0,
            tilt_gain: 0.0,
            opacity: 1.0,
            spacing: 0.0,
            size_scale: 1.0,
        }
    }

    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self.kind {
            BrushKind::Pencil => "Pencil",
            BrushKind::Marker => "Marker",
            BrushKind::Eraser => "Eraser",
            BrushKind::Pen => "Pen",
            BrushKind::Highlighter => "Highlighter",
            BrushKind::Custom(_) => "Custom",
            BrushKind::Shape(_) => "Shape",
        }
    }

    /// Erasers have no drawn colour — palette greys out the swatch.
    #[must_use]
    pub const fn palette_color(&self) -> Option<[u8; 4]> {
        match self.kind {
            BrushKind::Eraser => None,
            _ => Some(self.color),
        }
    }

    /// Width at a given normalised pressure/tilt pair. Multiplied by
    /// [`Self::size_scale`] so a user-tuned brush thickness applies
    /// uniformly across the pressure envelope. Used by the ribbon
    /// renderer per vertex and by the vector eraser (radius = width/2).
    #[must_use]
    pub fn width(&self, pressure: f32, tilt: f32) -> f32 {
        let base = self
            .tilt_gain
            .mul_add(tilt, lerp(self.min_width, self.max_width, pressure));
        base * self.size_scale.max(0.01)
    }

    /// Blend mode a stroke painted with this preset uses. Erasers get
    /// [`SegmentMode::Draw`] as a fallback — the ribbon renderer never
    /// sees an eraser preset (they're intercepted upstream).
    #[must_use]
    pub const fn segment_mode(&self) -> SegmentMode {
        match self.kind {
            BrushKind::Highlighter => SegmentMode::Multiply,
            BrushKind::Eraser
            | BrushKind::Pencil
            | BrushKind::Marker
            | BrushKind::Pen
            | BrushKind::Custom(_)
            | BrushKind::Shape(_) => SegmentMode::Draw,
        }
    }

    /// Render style for a full stroke — one paint config reused for
    /// every vertex circle and connecting trapezoid.
    #[must_use]
    pub const fn stroke_style(&self, stroke_color: [u8; 4]) -> StrokeStyle {
        StrokeStyle {
            color: color_from_rgba(stroke_color),
            opacity: self.opacity,
            mode: self.segment_mode(),
        }
    }

    /// Plan the segment from `from` to `to` using this preset. Retained
    /// for callers that still want a per-segment width (currently only
    /// tests).
    #[must_use]
    pub fn plan(&self, stroke_color: [u8; 4], from: InkPoint, to: InkPoint) -> SegmentPlan {
        let pressure = (from.pressure_f32() + to.pressure_f32()) * 0.5;
        let tilt = (from.tilt_f32() + to.tilt_f32()) * 0.5;
        let width = self.width(pressure, tilt);
        SegmentPlan {
            color: color_from_rgba(stroke_color),
            width,
            opacity: self.opacity,
            mode: self.segment_mode(),
        }
    }
}

/// One quantised ink sample.
///
/// Coordinates stay in Freya logical units (`f32` points/dp);
/// pressure/tilt collapse to `u8` (0..=255) and the per-sample time
/// delta to `u16` µs, matching the ~7-byte target in `PLAN.md`. Phase 1
/// lands the wire shape; `dt_us` populates as timing surfaces from the
/// pen backend.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct InkPoint {
    pub x: f32,
    pub y: f32,
    pub pressure: u8,
    pub tilt: u8,
    pub dt_us: u16,
}

impl InkPoint {
    #[must_use]
    pub const fn new(x: f32, y: f32, pressure: u8, tilt: u8, dt_us: u16) -> Self {
        Self {
            x,
            y,
            pressure,
            tilt,
            dt_us,
        }
    }

    /// Build from unclamped `f32` pressure / tilt (`0.0..=1.0`).
    #[must_use]
    pub fn from_normalized(x: f32, y: f32, pressure: f32, tilt: f32, dt_us: u16) -> Self {
        Self {
            x,
            y,
            pressure: quantize_unit(pressure),
            tilt: quantize_unit(tilt),
            dt_us,
        }
    }

    #[must_use]
    pub fn distance_to(self, other: Self) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx.hypot(dy)
    }

    #[must_use]
    pub fn pressure_f32(self) -> f32 {
        f32::from(self.pressure) / 255.0
    }

    #[must_use]
    pub fn tilt_f32(self) -> f32 {
        f32::from(self.tilt) / 255.0
    }

    /// Replace the quantised pressure with a fresh normalised value.
    /// Used by [`crate::canvas::Board`] to bake a
    /// [`PressureCurve`] into incoming pen samples so the renderer
    /// stays curve-agnostic.
    #[must_use]
    pub fn with_pressure_f32(mut self, pressure: f32) -> Self {
        self.pressure = quantize_unit(pressure);
        self
    }
}

/// Endpoint style.
///
/// `Round` is the default for user-drawn strokes; `Flat` marks
/// endpoints produced by the vector eraser (splitting an existing
/// stroke at a circle boundary) so the renderer terminates the ribbon
/// perpendicular to the local tangent instead of bulging a semicircle
/// past the cut.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum CapStyle {
    #[default]
    Round,
    Flat,
    /// Third-party cap identity. External crates register a
    /// [`crate::render::CapRenderer`] at this id in the
    /// [`crate::render::CapRegistry`]; unknown ids fall back to
    /// [`Self::Round`].
    Custom(u16),
}

/// One drawn stroke. `points` are raw samples — every re-render walks
/// them through [`BrushPreset::plan`] so zoom/resolution changes never
/// bake into the geometry.
#[istmo::message]
#[derive(Clone, PartialEq, Debug)]
pub struct Stroke {
    pub id: StrokeId,
    pub brush: BrushId,
    pub color: [u8; 4],
    pub cap_start: CapStyle,
    pub cap_end: CapStyle,
    pub points: Vec<InkPoint>,
}

impl Stroke {
    #[must_use]
    pub fn new(id: StrokeId, brush: BrushId, color: [u8; 4], first: InkPoint) -> Self {
        Self {
            id,
            brush,
            color,
            cap_start: CapStyle::Round,
            cap_end: CapStyle::Round,
            points: vec![first],
        }
    }
}

/// Instructions for rendering one segment of a stroke.
///
/// Renderer-side type — carries a freya `Color` (not the wire
/// `[u8; 4]`) so paint calls skip a per-segment conversion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegmentPlan {
    pub color: Color,
    pub width: f32,
    pub opacity: f32,
    pub mode: SegmentMode,
}

/// One-per-stroke render style shared by every vertex circle and
/// trapezoid the ribbon renderer emits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeStyle {
    pub color: Color,
    pub opacity: f32,
    pub mode: SegmentMode,
}

/// How a segment interacts with what is already on the canvas.
///
/// The `Erase` variant is intentionally absent — Phase 3 replaced
/// alpha-clear erasure with a vector eraser that mutates the stroke
/// list directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentMode {
    Draw,
    Multiply,
}

fn lerp(min: f32, max: f32, t: f32) -> f32 {
    max.mul_add(t, min * (1.0 - t))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn quantize_unit(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

const fn color_from_rgba(rgba: [u8; 4]) -> Color {
    let [r, g, b, a] = rgba;
    Color::from_argb(a, r, g, b)
}
