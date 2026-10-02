//! Centered modal for creating a new canvas / PDF-backed canvas.
//!
//! Mirrors the folder-create modal shape: preview card on top, title
//! form field, swatch strip for background color, grid of paper
//! patterns rendered live with the chosen color, tag picker, and
//! Cancel / Confirmar action row. Emits a [`CreateCanvasRequest`] the
//! caller uses to instantiate the item — actual persistence lives in
//! `home.rs` so the sheet stays generic across the "new canvas" and
//! "imported PDF" flows.

use freya::prelude::*;
use freya_canvas_bg::{PatternKind, Rect as BgRect, paint_preview, pattern::DEFAULT_INK};
use freya_engine::prelude::Color as SkColor;

use crate::library::{BackgroundStyle, ItemKind};

use super::color_wheel::{ColorWheel, DEFAULT_SWATCHES, auto_color, color_swatch_strip};
use super::form_input::FormInput;
use super::modal::{Modal, ModalController};
use super::tag_picker::TagPicker;

const PATTERN_OPTIONS: [(BackgroundStyle, &str); 4] = [
    (BackgroundStyle::Blank, "Blanco"),
    (BackgroundStyle::Line, "Línea"),
    (BackgroundStyle::Grid, "Cuadrado"),
    (BackgroundStyle::DotGrid, "Puntos"),
];

/// Payload fired on `Confirmar`. `kind` is echoed back so the caller
/// doesn't have to remember which flow launched the sheet.
#[derive(Clone, Debug)]
pub struct CreateCanvasRequest {
    pub kind: ItemKind,
    pub name: String,
    pub color: Color,
    pub background: BackgroundStyle,
    pub tag_names: Vec<String>,
}

/// Builder for the sheet.
#[derive(Clone)]
pub struct CanvasCreateSheet {
    kind: ItemKind,
    default_name: String,
    available_tags: Vec<String>,
    on_confirm: Callback<CreateCanvasRequest, ()>,
    on_cancel: Option<NoArgCallback<()>>,
}

impl PartialEq for CanvasCreateSheet {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl std::fmt::Debug for CanvasCreateSheet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CanvasCreateSheet")
            .field("kind", &self.kind)
            .field("default_name", &self.default_name)
            .finish_non_exhaustive()
    }
}

impl CanvasCreateSheet {
    #[must_use]
    pub fn new(on_confirm: impl Into<Callback<CreateCanvasRequest, ()>>) -> Self {
        Self {
            kind: ItemKind::Canvas,
            default_name: "Sin título".to_owned(),
            available_tags: Vec::new(),
            on_confirm: on_confirm.into(),
            on_cancel: None,
        }
    }

    /// Existing tag names to surface as suggestions in the tag picker.
    #[must_use]
    pub fn available_tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.available_tags = tags.into_iter().map(Into::into).collect();
        self
    }

    /// Distinguish the "new canvas" flow from the "imported PDF" flow.
    /// The sheet hides the paper-pattern grid for `PdfCanvas` since a
    /// PDF supplies its own page background.
    #[must_use]
    pub const fn kind(mut self, kind: ItemKind) -> Self {
        self.kind = kind;
        self
    }

    #[must_use]
    pub fn default_name(mut self, name: impl Into<String>) -> Self {
        self.default_name = name.into();
        self
    }

    #[must_use]
    pub fn on_cancel(mut self, cb: impl Into<NoArgCallback<()>>) -> Self {
        self.on_cancel = Some(cb.into());
        self
    }

    /// Push through the shared [`ModalController`] as a centered card.
    pub fn open(self) {
        let on_cancel = self.on_cancel.clone();
        ModalController::get().open(
            Modal::new(self)
                .center()
                .width(480.0)
                .dismiss_on_backdrop(true)
                .on_close(move || {
                    if let Some(cb) = on_cancel.clone() {
                        cb.call();
                    }
                }),
        );
    }
}

impl Component for CanvasCreateSheet {
    fn render(&self) -> impl IntoElement {
        let default_name = self.default_name.clone();
        let name = use_state(move || default_name);
        let custom_color = use_state(|| Option::<Color>::None);
        let special_color = use_state(|| Option::<Color>::None);
        let picker_open = use_state(|| false);
        let background = use_state(BackgroundStyle::default);
        let tag_names = use_state(Vec::<String>::new);

        let cur_name = name.read().clone();
        let effective_color = custom_color.read().unwrap_or_else(|| auto_color(&cur_name));
        let cur_background = *background.read();

        let submit_disabled = cur_name.trim().is_empty();
        let submit_cb = self.on_confirm.clone();
        let kind = self.kind;
        let title_label = match kind {
            ItemKind::Canvas => "Nuevo lienzo",
            ItemKind::PdfCanvas => "Importar PDF",
        };
        let submit = {
            let cur_name = cur_name.clone();
            move |_| {
                if submit_disabled {
                    return;
                }
                submit_cb.call(CreateCanvasRequest {
                    kind,
                    name: cur_name.trim().to_owned(),
                    color: effective_color,
                    background: cur_background,
                    tag_names: tag_names.read().clone(),
                });
                ModalController::get().close();
            }
        };

        let cancel = move |_| ModalController::get().close();

        rect()
            .vertical()
            .width(Size::px(480.0))
            .background(Color::from_rgb(30, 30, 34))
            .with_corner_radius(18.0)
            .padding((24.0, 28.0))
            .spacing(18.0)
            .cross_align(Alignment::Center)
            .child(preview_card(effective_color, cur_background))
            .child(
                label()
                    .color(Color::from_rgb(230, 230, 240))
                    .font_size(15.0)
                    .text(title_label),
            )
            .child(
                FormInput::new(name)
                    .placeholder("Nombre del lienzo")
                    .width(Size::fill()),
            )
            .child(color_strip(
                effective_color,
                custom_color,
                special_color,
                picker_open,
            ))
            .child(pattern_grid(
                effective_color,
                cur_background,
                background,
                matches!(kind, ItemKind::PdfCanvas),
            ))
            .child(
                TagPicker::new(tag_names)
                    .label("Etiquetas")
                    .available(self.available_tags.clone()),
            )
            .child(
                rect()
                    .horizontal()
                    .width(Size::fill())
                    .main_align(Alignment::End)
                    .spacing(20.0)
                    .padding((6.0, 0.0))
                    .child(
                        rect().padding((8.0, 14.0)).on_press(cancel).child(
                            label()
                                .color(Color::from_rgb(120, 170, 255))
                                .font_size(15.0)
                                .text("Cancelar"),
                        ),
                    )
                    .child(
                        rect().padding((8.0, 14.0)).on_press(submit).child(
                            label()
                                .color(if submit_disabled {
                                    Color::from_rgb(90, 100, 130)
                                } else {
                                    Color::from_rgb(120, 170, 255)
                                })
                                .font_size(15.0)
                                .text("Confirmar"),
                        ),
                    ),
            )
    }
}

// ---------------------------------------------------------------------------
// Sub-elements
// ---------------------------------------------------------------------------

const fn to_pattern_kind(bg: BackgroundStyle) -> PatternKind {
    match bg {
        BackgroundStyle::Blank => PatternKind::Blank,
        BackgroundStyle::Line => PatternKind::Line,
        BackgroundStyle::Grid => PatternKind::Grid,
        BackgroundStyle::DotGrid => PatternKind::DotGrid,
    }
}

/// Small canvas that paints `kind` on `surface`. `pattern_scale` tunes
/// line spacing so the pattern reads at the swatch size (big preview
/// uses the full 14.0; the mini pattern grid shrinks to 7.0 so the
/// strokes stay dense inside an 88×112 card).
fn pattern_canvas(surface: Color, kind: PatternKind, pattern_scale: f32) -> Canvas {
    let surface: SkColor = surface.into();
    canvas(RenderCallback::new(move |ctx| {
        let r = BgRect {
            min_x: 0.0,
            min_y: 0.0,
            max_x: ctx.size.width,
            max_y: ctx.size.height,
        };
        paint_preview(ctx.canvas, r, surface, DEFAULT_INK, kind, pattern_scale);
    }))
    .position(Position::new_absolute().left(0.).top(0.))
    .width(Size::fill())
    .height(Size::fill())
}

fn preview_card(color: Color, background: BackgroundStyle) -> impl IntoElement {
    rect()
        .width(Size::px(150.0))
        .height(Size::px(200.0))
        .with_corner_radius(6.0)
        .overflow(Overflow::Clip)
        .border(
            Border::new()
                .width(1.0)
                .alignment(BorderAlignment::Inner)
                .fill(Color::from_rgb(70, 70, 80)),
        )
        .child(keyed_pattern_canvas(color, to_pattern_kind(background), 14.0))
}

/// Hash a `(Color, PatternKind)` pair into a canvas diff key so the
/// render callback is replaced instead of frozen on the first mount.
/// Freya's canvas element diff treats every `RenderCallback` as equal
/// (`PartialEq` always `true`), so swapping surface colour / pattern
/// otherwise never repaints the preview — a new key forces a fresh
/// element with the current closure baked in.
fn canvas_key(color: Color, kind: PatternKind) -> u64 {
    let kind_byte: u8 = match kind {
        PatternKind::Blank => 0,
        PatternKind::Line => 1,
        PatternKind::Grid => 2,
        PatternKind::DotGrid => 3,
    };
    (u64::from(color.r()) << 24)
        | (u64::from(color.g()) << 16)
        | (u64::from(color.b()) << 8)
        | u64::from(color.a())
        | (u64::from(kind_byte) << 56)
}

fn keyed_pattern_canvas(
    color: Color,
    kind: PatternKind,
    pattern_scale: f32,
) -> impl IntoElement {
    pattern_canvas(color, kind, pattern_scale).key(canvas_key(color, kind))
}

fn color_strip(
    effective: Color,
    custom_color: State<Option<Color>>,
    special_color: State<Option<Color>>,
    picker_open: State<bool>,
) -> impl IntoElement {
    let stored_special = *special_color.read();
    let is_open = *picker_open.read();
    let matches_preset = DEFAULT_SWATCHES.iter().any(|c| *c == effective);
    let display_special = stored_special.or_else(|| (!matches_preset).then_some(effective));
    let selected_is_special = !matches_preset;
    let selected_preset = matches_preset.then_some(effective);

    let mut custom_preset = custom_color;
    let mut special_preset = special_color;
    let mut picker_preset = picker_open;
    let on_preset = move |c: Color| {
        custom_preset.set(Some(c));
        special_preset.set(None);
        picker_preset.set(false);
    };

    let mut picker_toggle = picker_open;
    let on_special = move |_: Event<PressEventData>| {
        let cur = *picker_toggle.read();
        picker_toggle.set(!cur);
    };

    let mut custom_picker = custom_color;
    let mut special_picker = special_color;
    let picker_on_change = move |c: Color| {
        custom_picker.set(Some(c));
        special_picker.set(Some(c));
    };

    let dropdown = is_open.then(|| {
        ColorWheel::new()
            .initial(display_special.unwrap_or(effective))
            .swatches(Vec::<Color>::new())
            .allow_custom(true)
            .width(240.0)
            .on_change(picker_on_change)
    });

    rect()
        .horizontal()
        .spacing(10.0)
        .cross_align(Alignment::Center)
        .child(
            label()
                .color(Color::from_rgb(200, 200, 210))
                .font_size(13.0)
                .text("Básico"),
        )
        .child(color_swatch_strip(
            selected_preset,
            display_special,
            selected_is_special,
            is_open,
            on_preset,
            on_special,
            dropdown,
        ))
}

fn pattern_grid(
    effective_color: Color,
    current: BackgroundStyle,
    background: State<BackgroundStyle>,
    hidden: bool,
) -> impl IntoElement {
    let mut grid = rect()
        .horizontal()
        .spacing(14.0)
        .cross_align(Alignment::Center);
    if hidden {
        return grid;
    }
    for (kind, name) in PATTERN_OPTIONS {
        let selected = current == kind;
        let mut bg = background;
        grid = grid.child(pattern_card(
            name,
            effective_color,
            kind,
            selected,
            move |()| bg.set(kind),
        ));
    }
    grid
}

fn pattern_card<F>(
    name: &'static str,
    effective_color: Color,
    bg_style: BackgroundStyle,
    selected: bool,
    mut handler: F,
) -> impl IntoElement
where
    F: FnMut(()) + 'static,
{
    let ring = if selected {
        Color::from_rgb(70, 140, 250)
    } else {
        Color::from_rgb(70, 70, 80)
    };
    rect()
        .vertical()
        .spacing(6.0)
        .cross_align(Alignment::Center)
        .on_press(move |_| handler(()))
        .child(
            rect()
                .width(Size::px(88.0))
                .height(Size::px(112.0))
                .with_corner_radius(8.0)
                .overflow(Overflow::Clip)
                .child(keyed_pattern_canvas(
                    effective_color,
                    to_pattern_kind(bg_style),
                    7.0,
                ))
                .maybe(selected, |r| {
                    r.child(
                        rect()
                            .width(Size::fill())
                            .height(Size::fill())
                            .layer(Layer::Overlay)
                            .with_corner_radius(8.0)
                            .position(Position::new_absolute().top(0.).left(0.))
                            .border(Border::new().width(2.).fill(ring)),
                    )
                }),
        )
        .child(
            label()
                .color(Color::from_rgb(220, 220, 230))
                .font_size(12.0)
                .text(name),
        )
}
