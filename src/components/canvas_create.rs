//! Bottom-sheet for creating a new canvas / PDF-backed canvas.
//!
//! Modeled after the reference screenshot: editable title at the top,
//! close (X) + confirm (✓) accents on the sides, a preview card, a
//! swatch strip for background color, and a grid of paper patterns.
//! Emits a [`CreateCanvasRequest`] the caller uses to instantiate the
//! item — actual persistence lives in `home.rs` so the sheet stays
//! generic across the "new canvas" and "imported PDF" flows.

use freya::prelude::*;

use crate::hooks::use_safe_area_insets;
use crate::library::{BackgroundStyle, ItemKind, TagId};

use super::color_wheel::{auto_color, DEFAULT_SWATCHES};
use super::modal::{Modal, ModalController};

const BASIC_SWATCHES: [Color; 5] = [
    DEFAULT_SWATCHES[0],
    DEFAULT_SWATCHES[6], // mustard
    DEFAULT_SWATCHES[5], // teal
    DEFAULT_SWATCHES[2], // salmon
    Color::from_rgb(30, 30, 34),
];

const PATTERN_OPTIONS: [(BackgroundStyle, &str); 4] = [
    (BackgroundStyle::Blank, "Blanco"),
    (BackgroundStyle::Line, "Línea"),
    (BackgroundStyle::Grid, "Cuadrado"),
    (BackgroundStyle::DotGrid, "Cuadrícula de puntos"),
];

/// Payload fired on `Confirmar`. `kind` is echoed back so the caller
/// doesn't have to remember which flow launched the sheet.
#[derive(Clone, Debug)]
pub struct CreateCanvasRequest {
    pub kind: ItemKind,
    pub name: String,
    pub color: Color,
    pub background: BackgroundStyle,
    pub tags: Vec<TagId>,
}

/// Builder for the sheet.
#[derive(Clone)]
pub struct CanvasCreateSheet {
    kind: ItemKind,
    default_name: String,
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
            on_confirm: on_confirm.into(),
            on_cancel: None,
        }
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

    /// Push through the shared [`ModalController`] as a bottom sheet.
    pub fn open(self) {
        let on_cancel = self.on_cancel.clone();
        ModalController::get().open(
            Modal::new(self)
                .bottom_sheet()
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
        let background = use_state(BackgroundStyle::default);

        let pad = *use_safe_area_insets().read();
        let cur_name = name.read().clone();
        let effective_color = custom_color.read().unwrap_or_else(|| auto_color(&cur_name));
        let cur_background = *background.read();

        let submit_disabled = cur_name.trim().is_empty();
        let submit_cb = self.on_confirm.clone();
        let kind = self.kind;
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
                    tags: Vec::new(),
                });
                ModalController::get().close();
            }
        };

        let cancel = |_| ModalController::get().close();

        rect()
            .vertical()
            .width(Size::fill())
            .background(Color::from_rgb(30, 30, 34))
            .with_corner_radius(20.0)
            .padding((14.0, 24.0, 24.0 + pad.bottom, 24.0))
            .spacing(18.0)
            .cross_align(Alignment::Center)
            .child(drag_handle())
            .child(top_bar(name, submit_disabled, cancel, submit))
            .child(tab_row())
            .child(preview_card(effective_color, cur_background))
            .child(swatch_row(effective_color, custom_color))
            .child(pattern_grid(cur_background, background, matches!(kind, ItemKind::PdfCanvas)))
    }
}

// ---------------------------------------------------------------------------
// Sub-elements
// ---------------------------------------------------------------------------

fn drag_handle() -> impl IntoElement {
    rect()
        .width(Size::px(36.0))
        .height(Size::px(4.0))
        .background(Color::from_rgb(80, 80, 90))
        .with_corner_radius(2.0)
}

fn top_bar<F, S>(
    name: State<String>,
    submit_disabled: bool,
    cancel: F,
    submit: S,
) -> impl IntoElement
where
    F: Fn(Event<PressEventData>) + 'static,
    S: Fn(Event<PressEventData>) + 'static,
{
    rect()
        .horizontal()
        .width(Size::fill())
        .cross_align(Alignment::Center)
        .spacing(12.0)
        .child(circle_icon("✕", Color::from_rgb(60, 60, 68), cancel))
        .child(
            rect()
                .width(Size::fill())
                .center()
                .child(Input::new(name).placeholder("Por favor, ingresa el título.")),
        )
        .child(circle_icon(
            "✓",
            if submit_disabled {
                Color::from_rgb(60, 70, 90)
            } else {
                Color::from_rgb(65, 130, 245)
            },
            submit,
        ))
}

fn circle_icon<F>(glyph: &'static str, bg: Color, handler: F) -> impl IntoElement
where
    F: Fn(Event<PressEventData>) + 'static,
{
    rect()
        .width(Size::px(36.0))
        .height(Size::px(36.0))
        .background(bg)
        .with_corner_radius(18.0)
        .center()
        .on_press(handler)
        .child(label().color(Color::WHITE).font_size(16.0).text(glyph))
}

fn tab_row() -> impl IntoElement {
    rect()
        .horizontal()
        .spacing(28.0)
        .child(
            label()
                .color(Color::from_rgb(70, 140, 250))
                .font_size(15.0)
                .text("Biblioteca de plantillas"),
        )
        .child(
            label()
                .color(Color::from_rgb(120, 120, 130))
                .font_size(15.0)
                .text("Mis plantillas"),
        )
}

fn preview_card(color: Color, background: BackgroundStyle) -> impl IntoElement {
    let _ = background; // pattern preview lands with the real renderer
    rect()
        .width(Size::px(150.0))
        .height(Size::px(200.0))
        .background(color)
        .with_corner_radius(6.0)
        .border(
            Border::new()
                .width(1.0)
                .alignment(BorderAlignment::Inner)
                .fill(Color::from_rgb(70, 70, 80)),
        )
}

fn swatch_row(effective: Color, custom: State<Option<Color>>) -> impl IntoElement {
    let mut row = rect()
        .horizontal()
        .spacing(10.0)
        .cross_align(Alignment::Center)
        .child(
            label()
                .color(Color::from_rgb(200, 200, 210))
                .font_size(13.0)
                .text("Básico"),
        );
    for &color in &BASIC_SWATCHES {
        let selected = color == effective;
        let mut custom = custom;
        row = row.child(swatch_dot(color, selected, move |()| custom.set(Some(color))));
    }
    row
}

fn swatch_dot<F>(color: Color, selected: bool, mut handler: F) -> impl IntoElement
where
    F: FnMut(()) + 'static,
{
    let ring = if selected {
        Color::from_rgb(70, 140, 250)
    } else {
        Color::from_rgb(70, 70, 80)
    };
    rect()
        .width(Size::px(26.0))
        .height(Size::px(26.0))
        .background(color)
        .with_corner_radius(6.0)
        .border(
            Border::new()
                .width(if selected { 2.0 } else { 1.0 })
                .alignment(BorderAlignment::Outer)
                .fill(ring),
        )
        .on_press(move |_| handler(()))
}

fn pattern_grid(
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
        grid = grid.child(pattern_card(name, selected, move |()| bg.set(kind)));
    }
    grid
}

fn pattern_card<F>(name: &'static str, selected: bool, mut handler: F) -> impl IntoElement
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
                .background(Color::from_rgb(245, 240, 235))
                .with_corner_radius(6.0)
                .border(
                    Border::new()
                        .width(if selected { 2.0 } else { 1.0 })
                        .alignment(BorderAlignment::Outer)
                        .fill(ring),
                ),
        )
        .child(
            label()
                .color(Color::from_rgb(220, 220, 230))
                .font_size(12.0)
                .text(name),
        )
}
