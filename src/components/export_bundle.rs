//! Centered modal that lets the user review what a bundle export will
//! carry before the save-file picker opens.
//!
//! Receives a [`crate::bundle::ExportPreview`] resolved from the raw
//! selection (an item id, folder id or workspace id). The sheet
//! renders a per-item checklist, an `include attachments` toggle, and
//! the list of plugin ids the receiver will need. On `Exportar` it
//! calls back with an [`ExportBundleRequest`]; the caller then opens
//! the native save picker and streams `write_bundle`.
//!
//! The sheet never touches `Library` or the file picker itself — it
//! only shapes the user's choices. That keeps it reusable from the
//! item card menu, the folder card menu and the workspace toolbar.

use std::collections::HashSet;

use freya::prelude::*;

use crate::bundle::{ExportOptions, ExportPreview};
use crate::library::{Item, ItemId, ItemKind};

use super::modal::{Modal, ModalController};

const SHEET_WIDTH: f32 = 560.0;
const ROW_SIZE: f32 = 44.0;
const LIST_HEIGHT: f32 = 220.0;

/// Payload fired on `Exportar`. `items` is the subset of
/// [`ExportPreview::items`] the user left checked.
#[derive(Clone, Debug)]
pub struct ExportBundleRequest {
    pub items: Vec<ItemId>,
    pub options: ExportOptions,
}

/// Builder for the sheet. Follows the shape of
/// [`super::canvas_create::CanvasCreateSheet`] /
/// [`super::folder_create::FolderCreateSheet`] — configure, then
/// [`ExportBundleSheet::open`] through the shared
/// [`ModalController`].
#[derive(Clone)]
pub struct ExportBundleSheet {
    preview: ExportPreview,
    on_confirm: Callback<ExportBundleRequest, ()>,
    on_cancel: Option<NoArgCallback<()>>,
}

impl PartialEq for ExportBundleSheet {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl std::fmt::Debug for ExportBundleSheet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportBundleSheet")
            .field("items", &self.preview.items.len())
            .field("plugins", &self.preview.requires_plugins.len())
            .finish_non_exhaustive()
    }
}

impl ExportBundleSheet {
    #[must_use]
    pub fn new(
        preview: ExportPreview,
        on_confirm: impl Into<Callback<ExportBundleRequest, ()>>,
    ) -> Self {
        Self {
            preview,
            on_confirm: on_confirm.into(),
            on_cancel: None,
        }
    }

    #[must_use]
    pub fn on_cancel(mut self, cb: impl Into<NoArgCallback<()>>) -> Self {
        self.on_cancel = Some(cb.into());
        self
    }

    pub fn open(self) {
        let on_cancel = self.on_cancel.clone();
        ModalController::get().open(
            Modal::new(self)
                .center()
                .width(SHEET_WIDTH)
                .dismiss_on_backdrop(true)
                .on_close(move || {
                    if let Some(cb) = on_cancel.clone() {
                        cb.call();
                    }
                }),
        );
    }
}

impl Component for ExportBundleSheet {
    fn render(&self) -> impl IntoElement {
        let items = self.preview.items.clone();
        let requires = self.preview.requires_plugins.clone();
        let any_pdf = items.iter().any(|i| item_carries_pdf(&i.kind));
        let all_ids: Vec<ItemId> = items.iter().map(|i| i.id).collect();

        let selected: State<HashSet<ItemId>> = {
            let initial = all_ids.iter().copied().collect::<HashSet<_>>();
            use_state(move || initial)
        };
        let include_attachments = use_state(|| true);

        let selected_count = selected.read().len();
        let total = items.len();
        let submit_disabled = selected_count == 0;

        let items_for_submit = items.clone();
        let submit_cb = self.on_confirm.clone();
        let submit = {
            let selected = selected;
            let include = include_attachments;
            move |_| {
                if submit_disabled {
                    return;
                }
                let sel = selected.read();
                let picked: Vec<ItemId> = items_for_submit
                    .iter()
                    .map(|i| i.id)
                    .filter(|id| sel.contains(id))
                    .collect();
                submit_cb.call(ExportBundleRequest {
                    items: picked,
                    options: ExportOptions {
                        include_attachments: *include.read(),
                    },
                });
                ModalController::get().close();
            }
        };
        let cancel = move |_| ModalController::get().close();

        rect()
            .vertical()
            .width(Size::px(SHEET_WIDTH))
            .background(Color::from_rgb(30, 30, 34))
            .with_corner_radius(18.0)
            .padding((22.0, 24.0))
            .spacing(16.0)
            .child(
                label()
                    .color(Color::from_rgb(230, 230, 240))
                    .font_size(16.0)
                    .text("Exportar bundle"),
            )
            .child(
                label()
                    .color(Color::from_rgb(170, 170, 180))
                    .font_size(13.0)
                    .text(format!(
                        "Se incluirán {selected_count} de {total} documentos."
                    )),
            )
            .child(item_list(items, selected))
            .child(attachments_row(include_attachments, any_pdf))
            .child(plugins_row(&requires))
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
                                .text("Exportar"),
                        ),
                    ),
            )
    }
}

fn item_list(items: Vec<Item>, selected: State<HashSet<ItemId>>) -> impl IntoElement {
    if items.is_empty() {
        return rect()
            .vertical()
            .width(Size::fill())
            .height(Size::px(LIST_HEIGHT))
            .center()
            .child(
                label()
                    .color(Color::from_rgb(140, 140, 150))
                    .font_size(13.0)
                    .text("La selección quedó vacía."),
            )
            .into_element();
    }

    let length = items.len();
    VirtualScrollView::new_with_data(items, move |item, data: &Vec<Item>| {
        let entry = data[item.index].clone();
        item_row(entry, selected).into_element()
    })
    .length(length)
    .width(Size::fill())
    .height(Size::px(LIST_HEIGHT))
    .item_size(ROW_SIZE)
    .into_element()
}

fn item_row(item: Item, selected: State<HashSet<ItemId>>) -> impl IntoElement {
    let id = item.id;
    let checked = selected.read().contains(&id);
    let kind_label = kind_label(&item.kind);
    let mut selected_for_press = selected;
    let on_press = move |_: Event<PressEventData>| {
        let mut set = selected_for_press.write();
        if !set.remove(&id) {
            set.insert(id);
        }
    };

    rect()
        .horizontal()
        .height(Size::px(ROW_SIZE))
        .cross_align(Alignment::Center)
        .spacing(12.0)
        .padding((4.0, 8.0))
        .on_press(on_press)
        .child(check_indicator(checked))
        .child(
            rect().vertical().width(Size::fill()).spacing(2.0).child(
                label()
                    .color(Color::from_rgb(230, 230, 240))
                    .font_size(14.0)
                    .text(item.name.clone()),
            ).child(
                label()
                    .color(Color::from_rgb(130, 130, 140))
                    .font_size(11.0)
                    .text(kind_label),
            ),
        )
}

fn check_indicator(on: bool) -> impl IntoElement {
    let (border_color, fill_color) = if on {
        (Color::from_rgb(80, 140, 240), Color::from_rgb(80, 140, 240))
    } else {
        (Color::from_rgb(90, 90, 100), Color::TRANSPARENT)
    };
    rect()
        .width(Size::px(18.0))
        .height(Size::px(18.0))
        .with_corner_radius(4.0)
        .border(Border::new().width(1.5).fill(border_color))
        .background(fill_color)
        .center()
        .maybe_child(on.then(|| {
            SvgViewer::new(freya::icons::lucide::check())
                .color(Color::WHITE)
                .width(Size::px(12.0))
                .height(Size::px(12.0))
        }))
}

fn attachments_row(include: State<bool>, any_pdf: bool) -> impl IntoElement {
    let on = *include.read() && any_pdf;
    let disabled = !any_pdf;
    let mut toggle = include;
    let on_press = move |_: Event<PressEventData>| {
        if disabled {
            return;
        }
        let cur = *toggle.read();
        toggle.set(!cur);
    };
    rect()
        .horizontal()
        .width(Size::fill())
        .cross_align(Alignment::Center)
        .spacing(12.0)
        .padding((2.0, 2.0))
        .on_press(on_press)
        .child(check_indicator(on))
        .child(
            rect().vertical().width(Size::fill()).spacing(2.0).child(
                label()
                    .color(if disabled {
                        Color::from_rgb(120, 120, 130)
                    } else {
                        Color::from_rgb(220, 220, 230)
                    })
                    .font_size(14.0)
                    .text("Incluir attachments PDF"),
            ).child(
                label()
                    .color(Color::from_rgb(130, 130, 140))
                    .font_size(11.0)
                    .text(if disabled {
                        "Esta selección no contiene PDFs."
                    } else {
                        "Si lo dejás apagado, el bundle sólo lleva refs."
                    }),
            ),
        )
}

fn plugins_row(requires: &[String]) -> impl IntoElement {
    let body = if requires.is_empty() {
        "Plugins requeridos: ninguno.".to_owned()
    } else {
        format!("Plugins requeridos: {}", requires.join(", "))
    };
    rect()
        .vertical()
        .width(Size::fill())
        .padding((2.0, 2.0))
        .child(
            label()
                .color(Color::from_rgb(170, 170, 180))
                .font_size(12.0)
                .text(body),
        )
}

const fn kind_label(kind: &ItemKind) -> &'static str {
    match kind {
        ItemKind::Canvas => "Lienzo",
        ItemKind::PdfCanvas => "PDF + lienzo",
        ItemKind::SourcePdf => "PDF",
        ItemKind::DocLinked { .. } => "Documento ligado",
        ItemKind::PluginCanvas { .. } => "Plugin",
    }
}

const fn item_carries_pdf(kind: &ItemKind) -> bool {
    matches!(
        kind,
        ItemKind::PdfCanvas | ItemKind::SourcePdf | ItemKind::DocLinked { .. }
    )
}
