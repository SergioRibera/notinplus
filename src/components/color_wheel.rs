//! HSVA color picker: draggable SV square, hue bar, alpha bar, plus hex
//! and alpha percentage inputs. Optional swatch strip on top.
//!
//! The public builder keeps the `ColorWheel` name for backwards
//! compatibility. `.allow_custom(true)` toggles the full HSVA panel;
//! `.allow_custom(false)` leaves only the swatch strip.

use freya::prelude::*;

use super::FormInput;
use super::theme::{BORDER, SURFACE_TERTIARY, TEXT_PRIMARY, TEXT_SECONDARY};

/// Deterministically pick a swatch for `name`. Two callers passing the
/// same string always get the same color.
#[must_use]
pub fn auto_color(name: &str) -> Color {
    let mut hash: u32 = 2_166_136_261;
    for byte in name.trim().bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    DEFAULT_SWATCHES[(hash as usize) % DEFAULT_SWATCHES.len()]
}

/// Canonical folder / item swatch strip.
pub const DEFAULT_SWATCHES: [Color; 7] = [
    Color::from_rgb(245, 240, 235),
    Color::from_rgb(55, 65, 80),
    Color::from_rgb(245, 195, 175),
    Color::from_rgb(215, 105, 105),
    Color::from_rgb(100, 130, 200),
    Color::from_rgb(150, 205, 205),
    Color::from_rgb(230, 190, 105),
];

const SV_HEIGHT: f32 = 200.0;
const BAR_HEIGHT: f32 = 18.0;
const MARKER_SIZE: f32 = 14.0;
const DEFAULT_WIDTH: f32 = 240.0;

/// Internal HSVA state — keeping hue/sat/val separate avoids drift when
/// converting through RGB (loses hue at S=0 or V=0).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Hsva {
    h: f32,
    s: f32,
    v: f32,
    a: f32,
}

impl Hsva {
    fn from_color(c: Color) -> Self {
        let hsv = c.to_hsv();
        Self {
            h: hsv.h,
            s: hsv.s,
            v: hsv.v,
            a: f32::from(c.a()) / 255.0,
        }
    }

    fn to_color(self) -> Color {
        let rgb = Color::from_hsv(self.h, self.s, self.v);
        with_alpha(rgb, self.a)
    }

    fn to_rgb_opaque(self) -> Color {
        Color::from_hsv(self.h, self.s, self.v)
    }
}

fn with_alpha(c: Color, a: f32) -> Color {
    let a_u8 = (a * 255.0).round().clamp(0.0, 255.0) as u8;
    Color::from_argb(a_u8, c.r(), c.g(), c.b())
}

fn parse_hex_rgb(input: &str) -> Option<(u8, u8, u8)> {
    let trimmed = input.trim().trim_start_matches('#');
    if trimmed.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&trimmed[0..2], 16).ok()?;
    let g = u8::from_str_radix(&trimmed[2..4], 16).ok()?;
    let b = u8::from_str_radix(&trimmed[4..6], 16).ok()?;
    Some((r, g, b))
}

fn format_hex_rgb(c: Color) -> String {
    format!("{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn format_alpha_pct(a: f32) -> String {
    let pct = (a * 100.0).round().clamp(0.0, 100.0) as u32;
    pct.to_string()
}

#[allow(clippy::cast_possible_truncation)]
fn to_f32(v: f64) -> f32 {
    v as f32
}

/// Convert a "cursor position" (marker center coord) into a top-left
/// offset for a `MARKER_SIZE` marker, clamped so the marker never
/// overshoots the track bounds.
fn center_to_topleft(center: f32, track: f32) -> f32 {
    let max = (track - MARKER_SIZE).max(0.0);
    (center - MARKER_SIZE / 2.0).clamp(0.0, max)
}

fn nonzero_or(value: f32, fallback: f32) -> f32 {
    if value > 0.0 { value } else { fallback }
}

#[derive(Clone, Copy, Default, PartialEq)]
enum DragTarget {
    #[default]
    None,
    Sv,
    Hue,
    Alpha,
}

/// Builder for the color picker.
#[derive(Clone)]
pub struct ColorWheel {
    swatches: Vec<Color>,
    initial: Color,
    allow_custom: bool,
    width: f32,
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
            .field("width", &self.width)
            .finish_non_exhaustive()
    }
}

impl Default for ColorWheel {
    fn default() -> Self {
        Self {
            swatches: DEFAULT_SWATCHES.to_vec(),
            initial: DEFAULT_SWATCHES[0],
            allow_custom: true,
            width: DEFAULT_WIDTH,
            on_change: None,
        }
    }
}

impl ColorWheel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the swatch strip. Empty vec hides the strip.
    #[must_use]
    pub fn swatches(mut self, swatches: impl Into<Vec<Color>>) -> Self {
        self.swatches = swatches.into();
        self
    }

    /// Initially-selected color.
    #[must_use]
    pub const fn initial(mut self, color: Color) -> Self {
        self.initial = color;
        self
    }

    /// Show the HSVA panel below the swatches.
    #[must_use]
    pub const fn allow_custom(mut self, allow: bool) -> Self {
        self.allow_custom = allow;
        self
    }

    /// Panel width in pixels.
    #[must_use]
    pub const fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Alias kept for callers still using the old wheel API.
    #[must_use]
    pub const fn diameter(self, diameter: f32) -> Self {
        self.width(diameter)
    }

    /// Fires on every commit — swatch pick, drag, or valid input submit.
    #[must_use]
    pub fn on_change(mut self, cb: impl Into<Callback<Color, ()>>) -> Self {
        self.on_change = Some(cb.into());
        self
    }
}

impl Component for ColorWheel {
    fn render(&self) -> impl IntoElement {
        let initial = self.initial;
        let mut hsva = use_state(move || Hsva::from_color(initial));
        let mut dragging = use_state(DragTarget::default);
        let mut sv_area = use_state(Area::default);
        let mut hue_area = use_state(Area::default);
        let mut alpha_area = use_state(Area::default);
        let mut hex_input = use_state(move || format_hex_rgb(initial));
        let mut alpha_input = use_state(move || format_alpha_pct(f32::from(initial.a()) / 255.0));

        // Keep input strings in sync when the sliders drive changes.
        use_side_effect(move || {
            let cur = *hsva.read();
            hex_input.set_if_modified(format_hex_rgb(cur.to_color()));
            alpha_input.set_if_modified(format_alpha_pct(cur.a));
        });

        let current = *hsva.read();
        let current_color = current.to_color();
        let base_hue = Color::from_hsv(current.h, 1.0, 1.0);
        let opaque_rgb = current.to_rgb_opaque();
        let transparent_rgb = Color::from_argb(0, opaque_rgb.r(), opaque_rgb.g(), opaque_rgb.b());

        let on_change_field = self.on_change.clone();
        let commit = Callback::<Hsva, ()>::new(move |new_hsva: Hsva| {
            hsva.set_if_modified(new_hsva);
            if let Some(cb) = &on_change_field {
                cb.call(new_hsva.to_color());
            }
        });

        let update_sv = {
            let commit = commit.clone();
            move |coords: CursorPoint| {
                let area = sv_area.read().to_f64();
                if area.width() <= 0.0 || area.height() <= 0.0 {
                    return;
                }
                let s = to_f32(((coords.x - area.min_x()) / area.width()).clamp(0.0, 1.0));
                let ry = to_f32(((coords.y - area.min_y()) / area.height()).clamp(0.0, 1.0));
                let cur = *hsva.peek();
                commit.call(Hsva {
                    s,
                    v: 1.0 - ry,
                    ..cur
                });
            }
        };
        let update_hue = {
            let commit = commit.clone();
            move |coords: CursorPoint| {
                let area = hue_area.read().to_f64();
                if area.width() <= 0.0 {
                    return;
                }
                let rx = to_f32(((coords.x - area.min_x()) / area.width()).clamp(0.0, 1.0));
                let cur = *hsva.peek();
                commit.call(Hsva {
                    h: rx * 360.0,
                    ..cur
                });
            }
        };
        let update_alpha = {
            let commit = commit.clone();
            move |coords: CursorPoint| {
                let area = alpha_area.read().to_f64();
                if area.width() <= 0.0 {
                    return;
                }
                let rx = to_f32(((coords.x - area.min_x()) / area.width()).clamp(0.0, 1.0));
                let cur = *hsva.peek();
                commit.call(Hsva { a: rx, ..cur });
            }
        };

        let on_sv_pointer_down = {
            let update_sv = update_sv.clone();
            move |e: Event<PointerEventData>| {
                if !e.data().is_primary() {
                    return;
                }
                dragging.set(DragTarget::Sv);
                update_sv(e.global_location());
                e.stop_propagation();
                e.prevent_default();
            }
        };
        let on_hue_pointer_down = {
            let update_hue = update_hue.clone();
            move |e: Event<PointerEventData>| {
                if !e.data().is_primary() {
                    return;
                }
                dragging.set(DragTarget::Hue);
                update_hue(e.global_location());
                e.stop_propagation();
                e.prevent_default();
            }
        };
        let on_alpha_pointer_down = {
            let update_alpha = update_alpha.clone();
            move |e: Event<PointerEventData>| {
                if !e.data().is_primary() {
                    return;
                }
                dragging.set(DragTarget::Alpha);
                update_alpha(e.global_location());
                e.stop_propagation();
                e.prevent_default();
            }
        };

        let on_global_pointer_move = move |e: Event<PointerEventData>| match *dragging.read() {
            DragTarget::Sv => update_sv(e.global_location()),
            DragTarget::Hue => update_hue(e.global_location()),
            DragTarget::Alpha => update_alpha(e.global_location()),
            DragTarget::None => {}
        };
        let on_global_pointer_press = move |_: Event<PointerEventData>| {
            if *dragging.read() != DragTarget::None {
                dragging.set(DragTarget::None);
            }
        };

        // Hex + alpha inputs
        let hex_submit = {
            let commit = commit.clone();
            move |value: String| {
                if let Some((r, g, b)) = parse_hex_rgb(&value) {
                    let hsv = Color::from_rgb(r, g, b).to_hsv();
                    let cur = *hsva.peek();
                    commit.call(Hsva {
                        h: hsv.h,
                        s: hsv.s,
                        v: hsv.v,
                        ..cur
                    });
                }
            }
        };

        let swatch_fire = {
            let commit = commit.clone();
            Callback::<Color, ()>::new(move |c: Color| {
                let hsv = c.to_hsv();
                let cur = *hsva.peek();
                commit.call(Hsva {
                    h: hsv.h,
                    s: hsv.s,
                    v: hsv.v,
                    a: if c.a() == 255 {
                        cur.a
                    } else {
                        f32::from(c.a()) / 255.0
                    },
                });
            })
        };

        // -- element tree --
        let picker_width = self.width;
        let inner_width = picker_width - 2.0 * PANEL_PADDING;

        // Marker positions ride on the *measured* track (from on_sized) so
        // they stay under the cursor even when the parent hands the picker
        // a width that differs from `self.width`. Fall back to inner_width
        // on the first frame before on_sized has fired.
        let sv_track_w = nonzero_or(sv_area.read().width(), inner_width);
        let sv_track_h = nonzero_or(sv_area.read().height(), SV_HEIGHT);
        let hue_track_w = nonzero_or(hue_area.read().width(), inner_width);
        let alpha_track_w = nonzero_or(alpha_area.read().width(), inner_width);

        let sv_marker_left = center_to_topleft(current.s * sv_track_w, sv_track_w);
        let sv_marker_top = center_to_topleft((1.0 - current.v) * sv_track_h, sv_track_h);
        let hue_marker_left = center_to_topleft((current.h / 360.0) * hue_track_w, hue_track_w);
        let alpha_marker_left = center_to_topleft(current.a * alpha_track_w, alpha_track_w);

        let sv_pane = rect()
            .width(Size::fill())
            .height(Size::px(SV_HEIGHT))
            .with_corner_radius(8.0)
            .overflow(Overflow::Clip)
            .on_sized(move |e: Event<SizedEventData>| sv_area.set(e.area))
            .on_pointer_down(on_sv_pointer_down)
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::fill())
                    .background(
                        LinearGradient::new()
                            .angle(-90.0)
                            .stop((Color::from_rgb(255, 255, 255), 0.0))
                            .stop((base_hue, 100.0)),
                    )
                    .child(
                        rect()
                            .position(Position::new_absolute())
                            .width(Size::fill())
                            .height(Size::fill())
                            .background(
                                LinearGradient::new()
                                    .angle(0.0)
                                    .stop((Color::from_argb(0, 0, 0, 0), 0.0))
                                    .stop((Color::from_rgb(0, 0, 0), 100.0)),
                            ),
                    ),
            )
            .child(marker_dot(sv_marker_left, sv_marker_top, current_color));

        let hue_bar = rect()
            .width(Size::fill())
            .height(Size::px(BAR_HEIGHT))
            .on_sized(move |e: Event<SizedEventData>| hue_area.set(e.area))
            .on_pointer_down(on_hue_pointer_down)
            .child(
                rect()
                    .expanded()
                    .with_corner_radius(BAR_HEIGHT / 2.0)
                    .background(
                        LinearGradient::new()
                            .angle(-90.0)
                            .stop((Color::from_rgb(255, 0, 0), 0.0))
                            .stop((Color::from_rgb(255, 255, 0), 16.0))
                            .stop((Color::from_rgb(0, 255, 0), 33.0))
                            .stop((Color::from_rgb(0, 255, 255), 50.0))
                            .stop((Color::from_rgb(0, 0, 255), 66.0))
                            .stop((Color::from_rgb(255, 0, 255), 83.0))
                            .stop((Color::from_rgb(255, 0, 0), 100.0)),
                    ),
            )
            .child(marker_dot(
                hue_marker_left,
                (BAR_HEIGHT - MARKER_SIZE) / 2.0,
                base_hue,
            ));

        let alpha_bar = rect()
            .width(Size::fill())
            .height(Size::px(BAR_HEIGHT))
            .on_sized(move |e: Event<SizedEventData>| alpha_area.set(e.area))
            .on_pointer_down(on_alpha_pointer_down)
            .child(
                rect()
                    .expanded()
                    .with_corner_radius(BAR_HEIGHT / 2.0)
                    .background(
                        LinearGradient::new()
                            .angle(-90.0)
                            .stop((transparent_rgb, 0.0))
                            .stop((opaque_rgb, 100.0)),
                    ),
            )
            .child(marker_dot(
                alpha_marker_left,
                (BAR_HEIGHT - MARKER_SIZE) / 2.0,
                current_color,
            ));

        let hex_field = FormInput::new(hex_input)
            .width(Size::flex(1.))
            .flat()
            .on_change(hex_submit);

        let inputs_row = rect()
            .horizontal()
            .spacing(8.0)
            .cross_align(Alignment::Center)
            .child(
                rect()
                    .width(Size::px(32.0))
                    .height(Size::px(28.0))
                    .with_corner_radius(6.0)
                    .background(current_color)
                    .border(
                        Border::new()
                            .fill(BORDER)
                            .width(1.0)
                            .alignment(BorderAlignment::Inner),
                    ),
            )
            .child(label().color(TEXT_SECONDARY).font_size(12.0).text("Hex"))
            .child(hex_field);

        let mut root = rect()
            .vertical()
            .width(Size::px(picker_width))
            .spacing(12.0)
            .padding(PANEL_PADDING)
            .background(SURFACE_TERTIARY)
            .with_corner_radius(12.0)
            .color(TEXT_PRIMARY)
            .on_global_pointer_move(on_global_pointer_move)
            .on_global_pointer_press(on_global_pointer_press);

        if !self.swatches.is_empty() {
            root = root.child(swatch_strip(&self.swatches, current_color, swatch_fire));
        }
        if self.allow_custom {
            root = root
                .child(sv_pane)
                .child(hue_bar)
                .child(alpha_bar)
                .child(inputs_row);
        }
        root
    }
}

const PANEL_PADDING: f32 = 12.0;

fn marker_dot(left: f32, top: f32, fill: Color) -> Rect {
    rect()
        .position(Position::new_absolute().top(top).left(left))
        .layer(2_i16)
        .width(Size::px(MARKER_SIZE))
        .height(Size::px(MARKER_SIZE))
        .with_corner_radius(MARKER_SIZE / 2.0)
        .background(fill)
        .border(
            Border::new()
                .fill(Color::from_rgb(255, 255, 255))
                .width(2.0),
        )
}

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
                .fill(ring),
        )
        .on_press(move |_| handler(()))
}

/// Canonical palette strip reused across folder creation and the brush
/// popup: seven [`DEFAULT_SWATCHES`] followed by a rainbow "special"
/// slot that opens a floating [`ColorWheel`] picker.
///
/// `selected` + `special_is_selected` paint the ring around whichever
/// swatch (or the special slot) matches the current colour. `picker`
/// is an already-built [`ColorWheel`] element — pass `Some(...)` when
/// `picker_open` is `true`, `None` otherwise; the strip anchors it
/// beneath the special slot so it floats instead of pushing content.
pub fn color_swatch_strip<PresetCb, SpecialCb, Picker>(
    selected: Option<Color>,
    special: Option<Color>,
    special_is_selected: bool,
    picker_open: bool,
    on_preset: PresetCb,
    on_special: SpecialCb,
    picker: Option<Picker>,
) -> impl IntoElement
where
    PresetCb: FnMut(Color) + Clone + 'static,
    SpecialCb: FnMut(Event<PressEventData>) + 'static,
    Picker: IntoElement + 'static,
{
    let mut row = rect()
        .horizontal()
        .spacing(10.0)
        .cross_align(Alignment::Center);
    for &color in &DEFAULT_SWATCHES {
        let mut cb = on_preset.clone();
        let is_sel = selected == Some(color) && !special_is_selected;
        row = row.child(preset_swatch(color, is_sel, move |_| cb(color)));
    }
    row = row.child(
        Attached::new(special_swatch(
            special,
            special_is_selected,
            picker_open,
            on_special,
        ))
        .bottom()
        .maybe_child(picker),
    );
    row
}

fn preset_swatch<F>(color: Color, selected: bool, on_press: F) -> impl IntoElement
where
    F: FnMut(Event<PressEventData>) + 'static,
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
                .fill(ring),
        )
        .on_press(on_press)
}

fn special_swatch<F>(
    special: Option<Color>,
    selected: bool,
    picker_open: bool,
    on_press: F,
) -> impl IntoElement
where
    F: FnMut(Event<PressEventData>) + 'static,
{
    let ring = if selected || picker_open {
        Color::from_rgb(70, 140, 250)
    } else {
        Color::from_rgb(70, 70, 78)
    };
    let base = rect()
        .width(Size::px(30.0))
        .height(Size::px(30.0))
        .with_corner_radius(15.0)
        .border(
            Border::new()
                .width(if selected || picker_open { 2.0 } else { 1.0 })
                .fill(ring),
        )
        .on_press(on_press);

    match special {
        Some(c) => base.background(c),
        None => base.background(
            LinearGradient::new()
                .angle(-90.0)
                .stop((Color::from_rgb(255, 0, 0), 0.0))
                .stop((Color::from_rgb(255, 255, 0), 16.0))
                .stop((Color::from_rgb(0, 255, 0), 33.0))
                .stop((Color::from_rgb(0, 255, 255), 50.0))
                .stop((Color::from_rgb(0, 0, 255), 66.0))
                .stop((Color::from_rgb(255, 0, 255), 83.0))
                .stop((Color::from_rgb(255, 0, 0), 100.0)),
        ),
    }
}
