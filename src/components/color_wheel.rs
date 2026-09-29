//! HSV color-wheel picker with a canonical swatch strip.
//!
//! Built for two-hop reuse:
//!
//! * folder / item creation flows drop it into a modal alongside a name
//!   input to let the user override the auto-derived color;
//! * the drawing canvas will consume the same widget for brush color
//!   selection later.
//!
//! MVP wheel is single-tap; drag-to-hue and a value slider will land
//! once the folder-create modal wants them. The value channel is
//! currently pinned at `1.0` (max brightness) so tapping the wheel
//! always lands on a punchy hue.

use freya::prelude::*;
use freya_engine::prelude::{Color as SkColor, Paint, PaintStyle, Point as SkPoint, Shader, TileMode};

/// Deterministically pick a swatch for `name`. Two callers passing
/// the same string always get the same color, so the folder / item
/// grid stays visually stable while the user types.
#[must_use]
pub fn auto_color(name: &str) -> Color {
    let mut hash: u32 = 2_166_136_261;
    for byte in name.trim().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    DEFAULT_SWATCHES[(hash as usize) % DEFAULT_SWATCHES.len()]
}

/// Canonical folder / item swatch strip — matches the seven presets
/// in the Nebo/Noteshelf-style folder dialog. Also used as the input
/// space for auto-color derivation (name hash → index).
pub const DEFAULT_SWATCHES: [Color; 7] = [
    Color::from_rgb(245, 240, 235),
    Color::from_rgb(55, 65, 80),
    Color::from_rgb(245, 195, 175),
    Color::from_rgb(215, 105, 105),
    Color::from_rgb(100, 130, 200),
    Color::from_rgb(150, 205, 205),
    Color::from_rgb(230, 190, 105),
];

/// Builder for the picker. Follows the freya-widget convention — no
/// `.build()`, chain setters, embed as `.child(ColorWheel::new()...)`
/// wherever an [`IntoElement`] is expected.
#[derive(Clone)]
pub struct ColorWheel {
    swatches: Vec<Color>,
    initial: Color,
    allow_custom: bool,
    diameter: f32,
    on_change: Option<Callback<Color, ()>>,
}

impl PartialEq for ColorWheel {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl std::fmt::Debug for ColorWheel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ColorWheel")
            .field("swatches", &self.swatches)
            .field("initial", &self.initial)
            .field("allow_custom", &self.allow_custom)
            .field("diameter", &self.diameter)
            .finish_non_exhaustive()
    }
}

impl Default for ColorWheel {
    fn default() -> Self {
        Self {
            swatches: DEFAULT_SWATCHES.to_vec(),
            initial: DEFAULT_SWATCHES[0],
            allow_custom: true,
            diameter: 200.0,
            on_change: None,
        }
    }
}

impl ColorWheel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the swatch strip.
    #[must_use]
    pub fn swatches(mut self, swatches: impl Into<Vec<Color>>) -> Self {
        self.swatches = swatches.into();
        self
    }

    /// Initially-selected color. Only consulted on mount — later
    /// `on_change` firings drive the selection.
    #[must_use]
    pub const fn initial(mut self, color: Color) -> Self {
        self.initial = color;
        self
    }

    /// Show the HSV wheel below the swatches. Defaults to `true`; set
    /// to `false` for a swatch-only picker.
    #[must_use]
    pub const fn allow_custom(mut self, allow: bool) -> Self {
        self.allow_custom = allow;
        self
    }

    /// Wheel diameter in pixels.
    #[must_use]
    pub const fn diameter(mut self, diameter: f32) -> Self {
        self.diameter = diameter;
        self
    }

    /// Fired every time the selection changes — swatch tap or wheel
    /// tap.
    #[must_use]
    pub fn on_change(mut self, cb: impl Into<Callback<Color, ()>>) -> Self {
        self.on_change = Some(cb.into());
        self
    }
}

impl Component for ColorWheel {
    fn render(&self) -> impl IntoElement {
        let initial = self.initial;
        let mut selected = use_state(move || initial);
        let cur = *selected.read();

        let on_change = self.on_change.clone();
        let fire = Callback::<Color, ()>::new(move |c| {
            selected.set(c);
            if let Some(cb) = on_change.clone() {
                cb.call(c);
            }
        });

        let strip = swatch_strip(&self.swatches, cur, fire.clone());

        let mut root = rect()
            .vertical()
            .spacing(16.0)
            .cross_align(Alignment::Center)
            .child(strip);

        if self.allow_custom {
            root = root.child(wheel_canvas(self.diameter, cur, fire));
        }

        root
    }
}

// ---------------------------------------------------------------------------
// Swatch strip
// ---------------------------------------------------------------------------

fn swatch_strip(
    swatches: &[Color],
    selected: Color,
    fire: Callback<Color, ()>,
) -> impl IntoElement {
    let mut row = rect().horizontal().spacing(10.0);
    for &color in swatches {
        let cb = fire.clone();
        let is_selected = color == selected;
        row = row.child(swatch_dot(color, is_selected, move |()| cb.call(color)));
    }
    row
}

fn swatch_dot<F>(color: Color, selected: bool, handler: F) -> impl IntoElement
where
    F: Fn(()) + 'static,
{
    let ring = if selected {
        Color::from_rgb(70, 140, 250)
    } else {
        Color::from_rgb(70, 70, 78)
    };
    rect()
        .width(Size::px(30.0))
        .height(Size::px(30.0))
        .background(color)
        .with_corner_radius(15.0)
        .border(
            Border::new()
                .width(if selected { 2.0 } else { 1.0 })
                .alignment(BorderAlignment::Outer)
                .fill(ring),
        )
        .on_press(move |_| handler(()))
}

// ---------------------------------------------------------------------------
// HSV wheel
// ---------------------------------------------------------------------------

fn wheel_canvas(diameter: f32, selected: Color, fire: Callback<Color, ()>) -> impl IntoElement {
    let radius = diameter / 2.0;

    let painter = canvas(RenderCallback::new(move |ctx| {
        let canvas = &ctx.canvas;
        let (w, h) = (ctx.size.width, ctx.size.height);
        let center = SkPoint::new(w / 2.0, h / 2.0);
        let r = (w.min(h) / 2.0) - 1.0;

        draw_hue_ring(canvas, center, r);
        draw_saturation_overlay(canvas, center, r);
        draw_selection_marker(canvas, selected, hsv_position_in(selected, r));
    }))
    .width(Size::px(diameter))
    .height(Size::px(diameter));

    rect()
        .width(Size::px(diameter))
        .height(Size::px(diameter))
        .child(painter)
        .on_press(move |e: Event<PressEventData>| {
            let loc = match &*e {
                PressEventData::Mouse(m) => m.element_location,
                PressEventData::Touch(t) => t.element_location,
                PressEventData::Keyboard(_) => return,
            };
            #[allow(clippy::cast_possible_truncation)]
            let x = loc.x as f32;
            #[allow(clippy::cast_possible_truncation)]
            let y = loc.y as f32;
            if let Some(c) = wheel_pick(x, y, radius) {
                fire.call(c);
            }
        })
}

fn draw_hue_ring(canvas: &freya_engine::prelude::Canvas, center: SkPoint, r: f32) {
    let hues: [SkColor; 7] = [
        SkColor::from_argb(255, 255, 0, 0),
        SkColor::from_argb(255, 255, 255, 0),
        SkColor::from_argb(255, 0, 255, 0),
        SkColor::from_argb(255, 0, 255, 255),
        SkColor::from_argb(255, 0, 0, 255),
        SkColor::from_argb(255, 255, 0, 255),
        SkColor::from_argb(255, 255, 0, 0),
    ];
    #[allow(deprecated)]
    let sweep = Shader::sweep_gradient(center, hues.as_slice(), None, TileMode::Clamp, None, None, None);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_style(PaintStyle::Fill);
    if let Some(s) = sweep {
        paint.set_shader(s);
    }
    canvas.draw_circle(center, r, &paint);
}

fn draw_saturation_overlay(canvas: &freya_engine::prelude::Canvas, center: SkPoint, r: f32) {
    let colors: [SkColor; 2] = [
        SkColor::from_argb(255, 255, 255, 255),
        SkColor::from_argb(0, 255, 255, 255),
    ];
    #[allow(deprecated)]
    let radial = Shader::radial_gradient(center, r, colors.as_slice(), None, TileMode::Clamp, None, None);
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_style(PaintStyle::Fill);
    if let Some(s) = radial {
        paint.set_shader(s);
    }
    canvas.draw_circle(center, r, &paint);
}

fn draw_selection_marker(canvas: &freya_engine::prelude::Canvas, selected: Color, (x, y): (f32, f32)) {
    let mut fill = Paint::default();
    fill.set_anti_alias(true);
    fill.set_style(PaintStyle::Fill);
    fill.set_color(SkColor::from_argb(selected.a(), selected.r(), selected.g(), selected.b()));
    canvas.draw_circle(SkPoint::new(x, y), 8.0, &fill);

    let mut ring = Paint::default();
    ring.set_anti_alias(true);
    ring.set_style(PaintStyle::Stroke);
    ring.set_stroke_width(2.0);
    ring.set_color(SkColor::from_argb(255, 255, 255, 255));
    canvas.draw_circle(SkPoint::new(x, y), 8.0, &ring);
}

fn hsv_position_in(color: Color, radius: f32) -> (f32, f32) {
    // Same math as `hsv_position`, but the caller passes the paint-time
    // radius (which accounts for the 1-px inset draw_hue_ring uses).
    let (h, s, _v) = rgb_to_hsv(color);
    let angle = h.to_radians();
    let dist = s * radius;
    (radius + angle.cos() * dist, radius + angle.sin() * dist)
}

fn wheel_pick(x: f32, y: f32, radius: f32) -> Option<Color> {
    let dx = x - radius;
    let dy = y - radius;
    let dist = (dx * dx + dy * dy).sqrt();
    if dist > radius {
        return None;
    }
    let angle = dy.atan2(dx).to_degrees();
    let hue = (angle + 360.0) % 360.0;
    let sat = (dist / radius).clamp(0.0, 1.0);
    Some(Color::from_hsv(hue, sat, 1.0))
}

fn rgb_to_hsv(color: Color) -> (f32, f32, f32) {
    let r = f32::from(color.r()) / 255.0;
    let g = f32::from(color.g()) / 255.0;
    let b = f32::from(color.b()) / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let v = max;
    let s = if max == 0.0 { 0.0 } else { d / max };
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * (((b - r) / d) + 2.0)
    } else {
        60.0 * (((r - g) / d) + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    (h, s, v)
}
