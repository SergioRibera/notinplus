//! Small sheet shown when a document has several notebooks: pick one,
//! create another, or attach an existing canvas.

use freya::icons::lucide;
use freya::prelude::*;

use crate::components::theme::{CARD_BG, PANEL_BG, TEXT_MUTED, TEXT_PRIMARY};
use crate::library::{ItemId, TabTarget};
use crate::space::{self, OpenHint};

use super::modal::{Modal, ModalController};

const SHEET_WIDTH: f32 = 420.0;
const ROW_SIZE: f32 = 44.0;
const LIST_HEIGHT: f32 = 220.0;

/// Chooser for the notebooks of one document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotebookChooser {
    item: ItemId,
    notebooks: Vec<(ItemId, String)>,
}

impl NotebookChooser {
    pub fn open(item: ItemId, notebooks: Vec<(ItemId, String)>) {
        ModalController::get().open(
            Modal::new(Self { item, notebooks })
                .center()
                .width(SHEET_WIDTH)
                .dismiss_on_backdrop(true),
        );
    }
}

impl Component for NotebookChooser {
    fn render(&self) -> impl IntoElement {
        let item = self.item;
        let length = self.notebooks.len();
        let list = VirtualScrollView::new_with_data(
            self.notebooks.clone(),
            move |row, data: &Vec<(ItemId, String)>| {
                let (id, name) = data[row.index].clone();
                notebook_row(id, name).into_element()
            },
        )
        .length(length)
        .width(Size::fill())
        .height(Size::px(list_height(length)))
        .item_size(ROW_SIZE);

        rect()
            .vertical()
            .width(Size::fill())
            .padding(16.0)
            .spacing(12.0)
            .background(PANEL_BG)
            .with_corner_radius(14.0)
            .color(TEXT_PRIMARY)
            .child(
                label()
                    .color(TEXT_PRIMARY)
                    .font_size(16.0)
                    .text("Cuadernos"),
            )
            .child(list)
            .child(
                rect()
                    .horizontal()
                    .width(Size::fill())
                    .spacing(8.0)
                    .content(Content::Flex)
                    .child(action_button(lucide::plus(), "Nuevo cuaderno", move || {
                        ModalController::get().close();
                        crate::notebooks::new_notebook(item);
                    }))
                    .child(action_button(
                        lucide::folder_open(),
                        "Usar un lienzo existente…",
                        move || {
                            ModalController::get().close();
                            crate::notebooks::attach_existing_notebook(item);
                        },
                    )),
            )
    }
}

/// Height that fits `rows` rows without exceeding the sheet budget.
fn list_height(rows: usize) -> f32 {
    let rows = u16::try_from(rows).unwrap_or(u16::MAX);
    LIST_HEIGHT.min(ROW_SIZE * f32::from(rows))
}

fn notebook_row(id: ItemId, name: String) -> Rect {
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(10.0)
        .padding((8.0, 10.0))
        .margin((2.0, 0.0))
        .width(Size::fill())
        .height(Size::px(ROW_SIZE - 4.0))
        .background(CARD_BG)
        .with_corner_radius(6.0)
        .on_press(move |_| {
            ModalController::get().close();
            space::open(TabTarget::Item(id), OpenHint::Beside);
        })
        .child(
            SvgViewer::new(lucide::notebook_text())
                .color(TEXT_MUTED)
                .width(Size::px(16.0))
                .height(Size::px(16.0)),
        )
        .child(label().color(TEXT_PRIMARY).font_size(13.0).text(name))
}

fn action_button(icon: Bytes, text: &'static str, on_press: impl Fn() + 'static) -> Rect {
    rect()
        .horizontal()
        .width(Size::flex(1.0))
        .height(Size::px(40.0))
        .cross_align(Alignment::Center)
        .main_align(Alignment::Center)
        .spacing(6.0)
        .with_corner_radius(8.0)
        .background(CARD_BG)
        .on_press(move |_| on_press())
        .child(
            SvgViewer::new(icon)
                .color(TEXT_PRIMARY)
                .width(Size::px(16.0))
                .height(Size::px(16.0)),
        )
        .child(label().color(TEXT_PRIMARY).font_size(13.0).text(text))
}
