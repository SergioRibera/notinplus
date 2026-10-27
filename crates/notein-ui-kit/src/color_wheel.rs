//! HSVA color picker: draggable SV square, hue bar, alpha bar, plus hex
//! and alpha percentage inputs. Optional swatch strip on top.
//!
//! The public builder keeps the `ColorWheel` name for backwards
//! compatibility. `.allow_custom(true)` toggles the full HSVA panel;
//! `.allow_custom(false)` leaves only the swatch strip.

use freya::prelude::*;

use crate::components::TapExt;
use crate::TextField;
use crate::theme::palette;

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

/// Canonical folder / item swatch strip. Tones match the darkened
/// highlighter-marker palette used by the Bible reader so the strip
/// doubles as the bookmark-highlight picker without re-authoring.
pub const DEFAULT_SWATCHES: [Color; 7] = [
    Color::from_rgb(0x6F, 0x63, 0x31),
    Color::from_rgb(0x6E, 0x38, 0x38),
    Color::from_rgb(0x3C, 0x5A, 0x7A),
    Color::from_rgb(0x2E, 0x5E, 0x54),
    Color::from_rgb(0x5C, 0x3F, 0x6E),
    Color::from_rgb(0x6E, 0x4F, 0x2D),
    Color::from_rgb(0x3C, 0x3C, 0x3C),
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
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to 0..=255
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

#[allow(clippy::cast_possible_truncation)]
const fn to_f32(v: f64) -> f32 {
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
    alpha: bool,
    bare: bool,
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
            alpha: true,
            bare: false,
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

    /// Show the transparency bar (on by default).
    #[must_use]
    pub const fn alpha(mut self, alpha: bool) -> Self {
        self.alpha = alpha;
        self
    }

    /// No panel of its own, for a card that already frames it.
    #[must_use]
    pub const fn bare(mut self) -> Self {
        self.bare = true;
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

impl DragTarget {
    /// The colour a pointer at `at` picks on this track, laid out at
    /// `area`; `None` before the track is measured.
    fn apply(self, cur: Hsva, area: Area, at: CursorPoint) -> Option<Hsva> {
        let area = area.to_f64();
        if area.width() <= 0.0 || (self == Self::Sv && area.height() <= 0.0) {
            return None;
        }
        let x = to_f32(((at.x - area.min_x()) / area.width()).clamp(0.0, 1.0));
        Some(match self {
            Self::Sv => {
                let y = to_f32(((at.y - area.min_y()) / area.height()).clamp(0.0, 1.0));
                Hsva {
                    s: x,
                    v: 1.0 - y,
                    ..cur
                }
            }
            Self::Hue => Hsva { h: x * 360.0, ..cur },
            Self::Alpha => Hsva { a: x, ..cur },
            Self::None => return None,
        })
    }
}

/// Where the three tracks were laid out.
#[derive(Clone, Copy, PartialEq)]
struct Tracks {
    sv: State<Area>,
    hue: State<Area>,
    alpha: State<Area>,
}

impl Tracks {
    fn area(self, target: DragTarget) -> Area {
        match target {
            DragTarget::Sv => *self.sv.peek(),
            DragTarget::Hue => *self.hue.peek(),
            DragTarget::Alpha => *self.alpha.peek(),
            DragTarget::None => Area::default(),
        }
    }
}

/// Keep `area` in step with the element's layout.
fn track_area(mut area: State<Area>) -> impl FnMut(Event<SizedEventData>) + 'static {
    move |e: Event<SizedEventData>| area.set_if_modified(e.area)
}

/// `color`'s hue, saturation and value with `cur`'s alpha.
fn with_hsv(cur: Hsva, color: Color) -> Hsva {
    let hsv = color.to_hsv();
    Hsva {
        h: hsv.h,
        s: hsv.s,
        v: hsv.v,
        ..cur
    }
}

impl Component for ColorWheel {
    fn render(&self) -> impl IntoElement {
        let initial = self.initial;
        let mut hsva = use_state(move || Hsva::from_color(initial));
        let mut dragging = use_state(DragTarget::default);
        let tracks = Tracks {
            sv: use_state(Area::default),
            hue: use_state(Area::default),
            alpha: use_state(Area::default),
        };
        let mut hex_input = use_state(move || format_hex_rgb(initial));
        // The field follows the sliders.
        use_side_effect(move || hex_input.set_if_modified(format_hex_rgb(hsva.read().to_color())));

        let current = *hsva.read();
        let on_change = self.on_change.clone();
        let commit = Callback::<Hsva, ()>::new(move |next: Hsva| {
            hsva.set_if_modified(next);
            if let Some(cb) = &on_change {
                cb.call(next.to_color());
            }
        });
        let update = {
            let commit = commit.clone();
            move |target: DragTarget, at: CursorPoint| {
                // Copied out first: a guard alive inside the `if let` would
                // still borrow `hsva` when `commit` writes it.
                let cur = *hsva.peek();
                if let Some(next) = target.apply(cur, tracks.area(target), at) {
                    commit.call(next);
                }
            }
        };
        let press = {
            let update = update.clone();
            move |target: DragTarget| {
                let update = update.clone();
                move |e: Event<PointerEventData>| {
                    if !e.data().is_primary() {
                        return;
                    }
                    dragging.set(target);
                    update(target, e.global_location());
                    e.stop_propagation();
                    e.prevent_default();
                }
            }
        };
        let hex_submit = {
            let commit = commit.clone();
            move |value: String| {
                if let Some((r, g, b)) = parse_hex_rgb(&value) {
                    let next = with_hsv(*hsva.peek(), Color::from_rgb(r, g, b));
                    commit.call(next);
                }
            }
        };
        let swatch_fire = Callback::<Color, ()>::new(move |c: Color| {
            let mut next = with_hsv(*hsva.peek(), c);
            if c.a() != 255 {
                next.a = f32::from(c.a()) / 255.0;
            }
            commit.call(next);
        });

        let padding = if self.bare { 0.0 } else { PANEL_PADDING };
        let inner_width = 2.0f32.mul_add(-padding, self.width);
        let mut root = rect()
            .vertical()
            .width(Size::px(self.width))
            .spacing(12.0)
            .padding(padding)
            .maybe(!self.bare, |r| r.background(palette().s3).corner_radius(12.0))
            .color(palette().t1)
            .on_global_pointer_move(move |e: Event<PointerEventData>| {
                let target = *dragging.peek();
                update(target, e.global_location());
            })
            .on_global_pointer_up(move |_: Event<PointerEventData>| {
                dragging.set_if_modified(DragTarget::None);
            });
        if !self.swatches.is_empty() {
            root = root.child(swatch_strip(&self.swatches, current.to_color(), &swatch_fire));
        }
        if self.allow_custom {
            let base_hue = Color::from_hsv(current.h, 1.0, 1.0);
            root = root
                .child(sv_pane(tracks.sv, current, inner_width, press(DragTarget::Sv)))
                .child(bar(
                    tracks.hue,
                    hue_gradient(),
                    (current.h / 360.0, inner_width),
                    base_hue,
                    press(DragTarget::Hue),
                ))
                .maybe_child(self.alpha.then(|| {
                    bar(
                        tracks.alpha,
                        alpha_gradient(current.to_rgb_opaque()),
                        (current.a, inner_width),
                        current.to_color(),
                        press(DragTarget::Alpha),
                    )
                }))
                .child(hex_row(current.to_color(), hex_input, hex_submit));
        }
        root
    }
}

/// Every hue, left to right.
fn hue_gradient() -> LinearGradient {
    [
        (255, 0, 0),
        (255, 255, 0),
        (0, 255, 0),
        (0, 255, 255),
        (0, 0, 255),
        (255, 0, 255),
        (255, 0, 0),
    ]
    .into_iter()
    .zip([0.0, 16.0, 33.0, 50.0, 66.0, 83.0, 100.0])
    .fold(LinearGradient::new().angle(-90.0), |g, ((r, gr, b), at)| {
        g.stop((Color::from_rgb(r, gr, b), at))
    })
}

/// `opaque` from transparent to solid.
fn alpha_gradient(opaque: Color) -> LinearGradient {
    LinearGradient::new()
        .angle(-90.0)
        .stop((Color::from_argb(0, opaque.r(), opaque.g(), opaque.b()), 0.0))
        .stop((opaque, 100.0))
}

/// The saturation × value square.
fn sv_pane(
    area: State<Area>,
    current: Hsva,
    fallback_width: f32,
    on_down: impl FnMut(Event<PointerEventData>) + 'static,
) -> Rect {
    let width = nonzero_or(area.read().width(), fallback_width);
    let height = nonzero_or(area.read().height(), SV_HEIGHT);
    rect()
        .width(Size::fill())
        .height(Size::px(SV_HEIGHT))
        .corner_radius(8.0)
        .overflow(Overflow::Clip)
        .on_sized(track_area(area))
        .on_pointer_down(on_down)
        .child(
            rect()
                .width(Size::fill())
                .height(Size::fill())
                .background(
                    LinearGradient::new()
                        .angle(-90.0)
                        .stop((Color::from_rgb(255, 255, 255), 0.0))
                        .stop((Color::from_hsv(current.h, 1.0, 1.0), 100.0)),
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
        .child(marker_dot(
            center_to_topleft(current.s * width, width),
            center_to_topleft((1.0 - current.v) * height, height),
            current.to_color(),
        ))
}

/// A horizontal track with its marker at `fraction` of the width.
fn bar(
    area: State<Area>,
    background: LinearGradient,
    (fraction, fallback_width): (f32, f32),
    marker: Color,
    on_down: impl FnMut(Event<PointerEventData>) + 'static,
) -> Rect {
    let width = nonzero_or(area.read().width(), fallback_width);
    rect()
        .width(Size::fill())
        .height(Size::px(BAR_HEIGHT))
        .on_sized(track_area(area))
        .on_pointer_down(on_down)
        .child(
            rect()
                .expanded()
                .corner_radius(BAR_HEIGHT / 2.0)
                .background(background),
        )
        .child(marker_dot(
            center_to_topleft(fraction * width, width),
            (BAR_HEIGHT - MARKER_SIZE) / 2.0,
            marker,
        ))
}

/// The chosen colour and its hex field.
fn hex_row(
    color: Color,
    hex_input: State<String>,
    on_submit: impl FnMut(String) + 'static,
) -> Rect {
    rect()
        .horizontal()
        .spacing(8.0)
        .cross_align(Alignment::Center)
        .child(
            rect()
                .width(Size::px(32.0))
                .height(Size::px(28.0))
                .corner_radius(6.0)
                .background(color)
                .border(
                    Border::new()
                        .fill(palette().line)
                        .width(1.0)
                        .alignment(BorderAlignment::Inner),
                ),
        )
        .child(label().color(palette().t2).font_size(12.0).text("Hex"))
        .child(
            rect()
                .width(Size::flex(1.))
                .child(TextField::new(hex_input).max_len(9).on_change(on_submit)),
        )
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
    fire: &Callback<Color, ()>,
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
        palette().focus
    } else {
        palette().line_strong
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
        .on_tap(move |_| handler(()))
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
        .on_tap(on_press)
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
        .on_tap(on_press);

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
