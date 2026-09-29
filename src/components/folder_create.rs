//! Modal sheet for `Crear nueva carpeta`.
//!
//! Collects a folder name plus an optional color override and hands
//! the result back to the caller through a [`Callback`]. The caller
//! (Home) owns the [`Library`] handle and performs the actual
//! `create_folder` call — this component only shapes the input.
//!
//! Color starts auto-derived from the current name (see
//! [`super::color_wheel::auto_color`]) and swaps to whatever the user
//! last tapped in the swatch strip. Tapping "Más colores" expands the
//! inline [`ColorWheel`] for a free-form HSV pick.

use freya::prelude::*;

use crate::library::TagId;

use super::color_wheel::{auto_color, ColorWheel, DEFAULT_SWATCHES};
use super::modal::{Modal, ModalController};

/// Payload the sheet emits when the user hits `Confirmar`. `tags` is
/// currently always empty — the tag-picker chip lands in a follow-up.
#[derive(Clone, Debug)]
pub struct CreateFolderRequest {
    pub name: String,
    pub color: Color,
    pub tags: Vec<TagId>,
}

/// Builder for the folder-creation sheet. Consumers configure it and
/// call [`FolderCreateSheet::open`] to push it through the shared
/// [`ModalController`].
#[derive(Clone)]
pub struct FolderCreateSheet {
    default_name: String,
    on_confirm: Callback<CreateFolderRequest, ()>,
    on_cancel: Option<NoArgCallback<()>>,
}

impl PartialEq for FolderCreateSheet {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl std::fmt::Debug for FolderCreateSheet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FolderCreateSheet")
            .field("default_name", &self.default_name)
            .finish_non_exhaustive()
    }
}

impl FolderCreateSheet {
    #[must_use]
    pub fn new(on_confirm: impl Into<Callback<CreateFolderRequest, ()>>) -> Self {
        Self {
            default_name: "Nueva carpeta".to_owned(),
            on_confirm: on_confirm.into(),
            on_cancel: None,
        }
    }

    /// Pre-fill the name input with `name`.
    #[must_use]
    pub fn default_name(mut self, name: impl Into<String>) -> Self {
        self.default_name = name.into();
        self
    }

    /// Fired when the user hits `Cancelar` or dismisses the modal.
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

impl Component for FolderCreateSheet {
    fn render(&self) -> impl IntoElement {
        let default_name = self.default_name.clone();
        let name = use_state(move || default_name);
        let custom_color = use_state(|| Option::<Color>::None);

        let cur_name = name.read().clone();
        let effective_color = custom_color.read().unwrap_or_else(|| auto_color(&cur_name));

        let submit_disabled = cur_name.trim().is_empty();
        let submit_cb = self.on_confirm.clone();
        let submit = {
            let cur_name = cur_name.clone();
            move |_| {
                if submit_disabled {
                    return;
                }
                let req = CreateFolderRequest {
                    name: cur_name.trim().to_owned(),
                    color: effective_color,
                    tags: Vec::new(),
                };
                submit_cb.call(req);
                ModalController::get().close();
            }
        };

        let cancel = move |_| ModalController::get().close();

        let swatches = {
            let mut custom_color = custom_color;
            ColorWheel::new()
                .swatches(DEFAULT_SWATCHES.to_vec())
                .initial(effective_color)
                .allow_custom(false)
                .on_change(move |c: Color| custom_color.set(Some(c)))
        };

        let wheel_toggle = use_state(|| false);
        let show_wheel = *wheel_toggle.read();
        let toggle_wheel = {
            let mut wheel_toggle = wheel_toggle;
            move |_| {
                let cur = *wheel_toggle.read();
                wheel_toggle.set(!cur);
            }
        };

        let wheel = if show_wheel {
            let mut custom_color = custom_color;
            Some(
                ColorWheel::new()
                    .initial(effective_color)
                    .allow_custom(true)
                    .diameter(200.0)
                    .on_change(move |c: Color| custom_color.set(Some(c))),
            )
        } else {
            None
        };

        rect()
            .vertical()
            .width(Size::px(480.0))
            .background(Color::from_rgb(30, 30, 34))
            .with_corner_radius(18.0)
            .padding((24.0, 28.0))
            .spacing(18.0)
            .cross_align(Alignment::Center)
            .child(preview_card(effective_color))
            .child(
                label()
                    .color(Color::from_rgb(230, 230, 240))
                    .font_size(15.0)
                    .text("Crear nueva carpeta"),
            )
            .child(Input::new(name).placeholder("Nombre de la carpeta"))
            .child(swatches)
            .child(
                rect()
                    .horizontal()
                    .spacing(6.0)
                    .cross_align(Alignment::Center)
                    .child(
                        label()
                            .color(Color::from_rgb(120, 170, 255))
                            .font_size(13.0)
                            .text(if show_wheel { "Ocultar rueda" } else { "Más colores" }),
                    )
                    .on_press(toggle_wheel),
            )
            .map(wheel, |r, w| r.child(w))
            .child(
                rect()
                    .horizontal()
                    .width(Size::fill())
                    .main_align(Alignment::End)
                    .spacing(20.0)
                    .padding((6.0, 0.0))
                    .child(
                        rect()
                            .padding((8.0, 14.0))
                            .on_press(cancel)
                            .child(
                                label()
                                    .color(Color::from_rgb(120, 170, 255))
                                    .font_size(15.0)
                                    .text("Cancelar"),
                            ),
                    )
                    .child(
                        rect()
                            .padding((8.0, 14.0))
                            .on_press(submit)
                            .child(
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

/// Little folder-shaped preview so the user sees the color they'll
/// end up with before hitting confirm.
fn preview_card(color: Color) -> impl IntoElement {
    rect()
        .width(Size::px(140.0))
        .height(Size::px(170.0))
        .background(color)
        .with_corner_radius(10.0)
        .border(
            Border::new()
                .width(1.0)
                .alignment(BorderAlignment::Inner)
                .fill(Color::from_rgb(70, 70, 80)),
        )
}
