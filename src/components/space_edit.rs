//! Centered sheet editing a mesa's name and colour.

use freya::prelude::*;

use crate::components::theme::{CARD_BG, PANEL_BG, TEXT_MUTED, TEXT_PRIMARY};
use crate::library::Rgba;

use super::color_wheel::DEFAULT_SWATCHES;
use super::form_input::FormInput;
use super::modal::{Modal, ModalController};

const SHEET_WIDTH: f32 = 360.0;

/// What the user confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpaceEditRequest {
    pub name: String,
    pub color: Option<Rgba>,
}

/// Builder for the sheet.
#[derive(Clone)]
pub struct SpaceEditSheet {
    title: String,
    name: String,
    color: Option<Rgba>,
    on_confirm: Callback<SpaceEditRequest, ()>,
}

impl PartialEq for SpaceEditSheet {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl std::fmt::Debug for SpaceEditSheet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpaceEditSheet")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl SpaceEditSheet {
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        color: Option<Rgba>,
        on_confirm: impl Into<Callback<SpaceEditRequest, ()>>,
    ) -> Self {
        Self {
            title: "Editar mesa".to_owned(),
            name: name.into(),
            color,
            on_confirm: on_confirm.into(),
        }
    }

    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn open(self) {
        ModalController::get().open(
            Modal::new(self)
                .center()
                .width(SHEET_WIDTH)
                .dismiss_on_backdrop(true),
        );
    }
}

impl Component for SpaceEditSheet {
    fn render(&self) -> impl IntoElement {
        let name = use_state({
            let initial = self.name.clone();
            move || initial
        });
        let color = use_state({
            let initial = self.color;
            move || initial
        });
        let current = *color.read();

        let mut swatches = rect()
            .horizontal()
            .width(Size::fill())
            .spacing(8.0)
            .cross_align(Alignment::Center)
            .child(swatch(None, current.is_none(), color));
        for preset in DEFAULT_SWATCHES {
            let rgba = [preset.r(), preset.g(), preset.b(), 255];
            swatches = swatches.child(swatch(Some(rgba), current == Some(rgba), color));
        }

        let on_confirm = self.on_confirm.clone();
        let submit = move |_: Event<PressEventData>| {
            let trimmed = name.read().trim().to_owned();
            if trimmed.is_empty() {
                return;
            }
            ModalController::get().close();
            on_confirm.call(SpaceEditRequest {
                name: trimmed,
                color: *color.read(),
            });
        };

        rect()
            .vertical()
            .width(Size::fill())
            .padding(16.0)
            .spacing(14.0)
            .background(PANEL_BG)
            .with_corner_radius(14.0)
            .color(TEXT_PRIMARY)
            .child(
                label()
                    .color(TEXT_PRIMARY)
                    .font_size(16.0)
                    .text(self.title.clone()),
            )
            .child(
                FormInput::new(name)
                    .auto_focus(true)
                    .placeholder("Nombre de la mesa")
                    .width(Size::fill()),
            )
            .child(label().color(TEXT_MUTED).font_size(12.0).text("Color"))
            .child(swatches)
            .child(
                rect()
                    .horizontal()
                    .width(Size::fill())
                    .spacing(8.0)
                    .main_align(Alignment::End)
                    .child(button("Cancelar", CARD_BG, |_| {
                        ModalController::get().close();
                    }))
                    .child(button("Guardar", crate::components::theme::PRIMARY, submit)),
            )
    }
}

fn swatch(value: Option<Rgba>, selected: bool, mut color: State<Option<Rgba>>) -> Rect {
    let fill = value.map_or(Color::from_rgb(70, 70, 80), |c| {
        Color::from_rgb(c[0], c[1], c[2])
    });
    let ring = if selected {
        Color::WHITE
    } else {
        Color::TRANSPARENT
    };
    rect()
        .width(Size::px(26.0))
        .height(Size::px(26.0))
        .with_corner_radius(13.0)
        .background(fill)
        .border(Border::new().width(2.0).fill(ring))
        .on_press(move |_| color.set(value))
}

fn button(
    text: &'static str,
    bg: Color,
    on_press: impl FnMut(Event<PressEventData>) + 'static,
) -> Rect {
    rect()
        .padding((8.0, 16.0))
        .with_corner_radius(8.0)
        .background(bg)
        .on_press(on_press)
        .child(label().color(TEXT_PRIMARY).font_size(13.0).text(text))
}
