//! Modal sheet for `Crear nueva carpeta`.
//!
//! Collects a folder name plus an optional color override and hands
//! the result back to the caller through a [`Callback`]. The caller
//! (Home) owns the [`Library`] handle and performs the actual
//! `create_folder` call — this component only shapes the input.
//!
//! Color starts auto-derived from the current name (see
//! [`super::color_wheel::auto_color`]) and swaps to whatever the user
//! last tapped. Beyond the seven presets, an eighth "rainbow" slot
//! opens the advanced [`ColorWheel`] picker as an in-modal popup; the
//! slot then displays the chosen custom color in place of the rainbow.

use freya::prelude::*;

use super::color_wheel::{ColorWheel, DEFAULT_SWATCHES, auto_color};
use super::form_input::FormInput;
use super::modal::{Modal, ModalController};
use super::tag_picker::TagPicker;

/// Payload the sheet emits when the user hits `Confirmar`. Tag names
/// are raw strings — the caller resolves them into `TagId`s (creating
/// missing ones) before persisting.
#[derive(Clone, Debug)]
pub struct CreateFolderRequest {
    pub name: String,
    pub color: Color,
    pub tag_names: Vec<String>,
}

/// Builder for the folder-creation sheet. Consumers configure it and
/// call [`FolderCreateSheet::open`] to push it through the shared
/// [`ModalController`].
#[derive(Clone)]
pub struct FolderCreateSheet {
    default_name: String,
    available_tags: Vec<String>,
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
            available_tags: Vec::new(),
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

    /// Existing tag names to offer as suggestions in the tag picker.
    #[must_use]
    pub fn available_tags(mut self, tags: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.available_tags = tags.into_iter().map(Into::into).collect();
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
        let tag_names = use_state(Vec::<String>::new);

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
                    tag_names: tag_names.read().clone(),
                };
                submit_cb.call(req);
                ModalController::get().close();
            }
        };

        let cancel = move |_| ModalController::get().close();

        let mut special_color = use_state(|| Option::<Color>::None);
        let mut picker_open = use_state(|| false);
        let picker_is_open = *picker_open.read();
        let special = *special_color.read();
        let current_custom = *custom_color.read();

        let mut custom_for_preset = custom_color;
        let mut special_for_preset = special_color;
        let on_preset_pick = move |c: Color| {
            special_for_preset.set(None);
            custom_for_preset.set(Some(c));
        };

        let toggle_picker = move |_| {
            let cur = *picker_open.read();
            picker_open.set(!cur);
        };

        let mut custom_for_picker = custom_color;
        let picker_on_change = move |c: Color| {
            special_color.set(Some(c));
            custom_for_picker.set(Some(c));
        };

        let selected_is_special = matches!((special, current_custom), (Some(s), Some(c)) if s == c);
        let dropdown = picker_is_open.then(|| {
            ColorWheel::new()
                .initial(special.unwrap_or(effective_color))
                .swatches(Vec::<Color>::new())
                .allow_custom(true)
                .width(260.0)
                .on_change(picker_on_change)
        });
        let strip = swatch_strip_with_special(
            current_custom,
            special,
            selected_is_special,
            picker_is_open,
            on_preset_pick,
            toggle_picker,
            dropdown,
        );

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
            .child(
                FormInput::new(name)
                    .placeholder("Nombre de la carpeta")
                    .width(Size::fill()),
            )
            .child(strip)
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

fn swatch_strip_with_special<PresetCb, SpecialCb, Picker>(
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
        row = row.child(swatch_dot(color, is_sel, move |_| cb(color)));
    }
    // Rainbow slot lives inside an `Attached` overlay so the picker can
    // float below it instead of pushing the modal content around.
    row = row.child(
        Attached::new(special_slot(
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

fn swatch_dot<F>(color: Color, selected: bool, on_press: F) -> impl IntoElement
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

fn special_slot<F>(
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

/// Little folder-shaped preview so the user sees the color they'll
/// end up with before hitting confirm.
fn preview_card(color: Color) -> impl IntoElement {
    rect()
        .width(Size::px(140.0))
        .height(Size::px(170.0))
        .background(color)
        .with_corner_radius(10.0)
        .border(Border::new().width(1.0).fill(Color::from_rgb(70, 70, 80)))
}
