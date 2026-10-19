//! Per-document action list every face shows behind its link button:
//! notebook, open beside, link with, show links.

use freya::icons::lucide;
use freya::prelude::*;

use crate::components::theme::{CARD_BG, PANEL_BG, TEXT_MUTED, TEXT_PRIMARY};
use crate::library::ItemId;

use super::modal::{Modal, ModalController};

const SHEET_WIDTH: f32 = 300.0;

/// Builder for the menu. `item` is `None` on a scratch canvas, which
/// disables everything that needs a persisted document.
#[derive(Clone)]
pub struct DocActionsMenu {
    item: Option<ItemId>,
    on_show_links: Option<NoArgCallback<()>>,
}

impl PartialEq for DocActionsMenu {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl std::fmt::Debug for DocActionsMenu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocActionsMenu")
            .field("item", &self.item)
            .finish_non_exhaustive()
    }
}

impl DocActionsMenu {
    #[must_use]
    pub const fn new(item: Option<ItemId>) -> Self {
        Self {
            item,
            on_show_links: None,
        }
    }

    /// Called when the user asks to see the document's links; the
    /// caller opens its sidebar on the links tab.
    #[must_use]
    pub fn on_show_links(mut self, cb: impl Into<NoArgCallback<()>>) -> Self {
        self.on_show_links = Some(cb.into());
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

impl Component for DocActionsMenu {
    fn render(&self) -> impl IntoElement {
        let item = self.item;
        let show_links = self.on_show_links.clone();
        let mut column = rect()
            .vertical()
            .width(Size::fill())
            .padding(10.0)
            .spacing(4.0)
            .background(PANEL_BG)
            .with_corner_radius(14.0)
            .color(TEXT_PRIMARY);

        column = column.child(row(
            lucide::notebook_text(),
            "Cuaderno",
            item.is_some(),
            move || {
                if let Some(id) = item {
                    crate::notebooks::open_notebook(id);
                }
            },
        ));
        column = column.child(row(lucide::columns_2(), "Abrir al lado…", true, || {
            crate::notebooks::open_beside_picker();
        }));
        column = column.child(row(
            lucide::link(),
            "Vincular con…",
            item.is_some(),
            move || {
                if let Some(id) = item {
                    crate::notebooks::link_related(id);
                }
            },
        ));
        if let Some(cb) = show_links {
            column = column.child(row(
                lucide::link_2(),
                "Ver vínculos",
                item.is_some(),
                move || cb.call(),
            ));
        }
        column
    }
}

fn row(icon: Bytes, text: &'static str, enabled: bool, on_press: impl Fn() + 'static) -> Rect {
    let glyph = if enabled { TEXT_PRIMARY } else { TEXT_MUTED };
    rect()
        .horizontal()
        .width(Size::fill())
        .height(Size::px(40.0))
        .cross_align(Alignment::Center)
        .padding((0.0, 10.0))
        .spacing(10.0)
        .with_corner_radius(8.0)
        .background(CARD_BG)
        .on_press(move |_| {
            if !enabled {
                return;
            }
            ModalController::get().close();
            on_press();
        })
        .child(
            SvgViewer::new(icon)
                .color(glyph)
                .width(Size::px(16.0))
                .height(Size::px(16.0)),
        )
        .child(label().color(glyph).font_size(13.0).text(text))
}
