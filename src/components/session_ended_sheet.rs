//! "La sesión terminó": shown to a guest whose shared session was
//! closed or who was thrown out. The documents stay open behind the
//! sheet; acknowledging closes them and returns home.

use freya::icons::lucide;
use freya::prelude::*;
use notein_sync::Removal;

use crate::components::theme::{ACCENT, CARD_BG, PANEL_BG, TEXT_MUTED, TEXT_PRIMARY};
use crate::session_end::{self, SessionEnd};

use super::modal::{Modal, ModalController};

const SHEET_WIDTH: f32 = 400.0;

#[derive(Clone, Debug, PartialEq)]
pub struct SessionEndedSheet {
    end: SessionEnd,
}

impl SessionEndedSheet {
    pub fn open(end: SessionEnd) {
        ModalController::get().open(
            Modal::new(Self { end })
                .center()
                .width(SHEET_WIDTH)
                .dismiss_on_backdrop(false),
        );
    }
}

impl Component for SessionEndedSheet {
    fn render(&self) -> impl IntoElement {
        let end = self.end.clone();
        let headline = match end.reason {
            Removal::Expelled => "Te quitaron de la sesión",
            Removal::Ended | Removal::Left => "La sesión terminó",
        };
        let count = end.docs.len();
        let detail = format!(
            "«{}» ya no se comparte con vos. {} se cierra{} al volver al inicio.",
            end.title,
            if count == 1 {
                "El documento".to_owned()
            } else {
                format!("Los {count} documentos")
            },
            if count == 1 { "" } else { "n" },
        );

        rect()
            .vertical()
            .width(Size::fill())
            .padding((20.0, 22.0))
            .spacing(12.0)
            .background(PANEL_BG)
            .with_corner_radius(16.0)
            .cross_align(Alignment::Center)
            .child(
                SvgViewer::new(lucide::door_open())
                    .color(ACCENT)
                    .width(Size::px(28.0))
                    .height(Size::px(28.0)),
            )
            .child(
                label()
                    .color(TEXT_PRIMARY)
                    .font_size(18.0)
                    .font_weight(FontWeight::BOLD)
                    .text(headline),
            )
            .child(
                label()
                    .color(TEXT_MUTED)
                    .font_size(12.0)
                    .text_align(TextAlign::Center)
                    .text(detail),
            )
            .child(
                rect()
                    .horizontal()
                    .content(Content::Flex)
                    .width(Size::fill())
                    .main_align(Alignment::End)
                    .child(
                        rect()
                            .padding((8.0, 14.0))
                            .background(CARD_BG)
                            .with_corner_radius(8.0)
                            .on_press(move |_: Event<PressEventData>| {
                                ModalController::get().close();
                                session_end::resolve(end.clone());
                            })
                            .child(
                                label()
                                    .color(ACCENT)
                                    .font_size(13.0)
                                    .text("Volver al inicio"),
                            ),
                    ),
            )
    }
}
