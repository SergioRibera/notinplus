//! "Uniéndose": what happens after an invitation link is opened. The
//! sheet follows the sync snapshot from "looking for the owner" to
//! "in", then opens the shared documents once they have arrived.

use freya::icons::lucide;
use freya::prelude::*;
use notein_sync::{JoinFailure, ShareId};

use crate::components::theme::{ACCENT, CARD_BG, ERROR, PANEL_BG, TEXT_MUTED, TEXT_PRIMARY};
use crate::library::{ItemId, TabTarget};
use crate::space::{self, OpenHint};
use crate::sync;

use super::modal::{Modal, ModalController};

const SHEET_WIDTH: f32 = 400.0;

/// Where the join stands, read from the snapshot on every change.
enum Stage {
    Searching,
    /// In: which of the share's documents are already in the library.
    Joined {
        ready: Vec<ItemId>,
        total: usize,
    },
    Failed(JoinFailure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinSheet {
    share: ShareId,
    title: String,
    owner: String,
}

impl JoinSheet {
    pub fn open(share: ShareId, title: impl Into<String>, owner: impl Into<String>) {
        ModalController::get().open(
            Modal::new(Self {
                share,
                title: title.into(),
                owner: owner.into(),
            })
            .center()
            .width(SHEET_WIDTH)
            .dismiss_on_backdrop(true),
        );
    }

    fn stage(&self) -> Stage {
        let snapshot = sync::snapshot();
        if let Some(view) = snapshot.shares.iter().find(|v| v.id == self.share) {
            let ready = view
                .docs
                .iter()
                .copied()
                .filter(|doc| snapshot.docs.contains_key(doc))
                .collect();
            return Stage::Joined {
                ready,
                total: view.docs.len(),
            };
        }
        match snapshot.last_join_failure {
            Some(reason) => Stage::Failed(reason),
            None => Stage::Searching,
        }
    }
}

impl Component for JoinSheet {
    fn render(&self) -> impl IntoElement {
        let tick = use_state(|| 0u64);
        let _ = *tick.read();
        use_hook({
            let mut tick = tick;
            move || {
                let changes = sync::changes();
                spawn(async move {
                    while changes.recv_async().await.is_ok() {
                        let next = tick.peek().wrapping_add(1);
                        tick.set(next);
                    }
                });
            }
        });

        let stage = self.stage();
        let (icon, headline, detail) = match &stage {
            Stage::Searching => (
                lucide::radar(),
                "Uniéndose…".to_owned(),
                format!(
                    "Buscando a {} en esta red. Dejá ambos dispositivos encendidos y en la misma Wi-Fi.",
                    self.owner
                ),
            ),
            Stage::Joined { ready, total } => (
                lucide::check(),
                "Ya estás dentro".to_owned(),
                if ready.len() == *total {
                    "Todos los documentos llegaron.".to_owned()
                } else {
                    format!(
                        "{} de {total} documentos listos; el resto sigue llegando.",
                        ready.len()
                    )
                },
            ),
            Stage::Failed(JoinFailure::Refused) => (
                lucide::x(),
                "No se pudo entrar".to_owned(),
                "La invitación ya no vale o la clave no coincide. Pedí una nueva.".to_owned(),
            ),
            Stage::Failed(JoinFailure::Unreachable) => (
                lucide::x(),
                "No se pudo entrar".to_owned(),
                format!("{} no apareció en la red.", self.owner),
            ),
        };

        let mut card = rect()
            .vertical()
            .width(Size::fill())
            .padding((20.0, 22.0))
            .spacing(12.0)
            .background(PANEL_BG)
            .with_corner_radius(16.0)
            .cross_align(Alignment::Center)
            .child(
                SvgViewer::new(icon)
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
                    .color(TEXT_PRIMARY)
                    .font_size(14.0)
                    .text(self.title.clone()),
            )
            .child(
                label()
                    .color(TEXT_MUTED)
                    .font_size(12.0)
                    .text_align(TextAlign::Center)
                    .text(detail),
            );

        let mut buttons = rect()
            .horizontal()
            .content(Content::Flex)
            .width(Size::fill())
            .spacing(10.0)
            .main_align(Alignment::End);
        match stage {
            Stage::Searching => {
                buttons = buttons.child(pill("Cerrar", TEXT_MUTED, |_| {
                    ModalController::get().close();
                }));
            }
            Stage::Joined { ready, .. } => {
                buttons = buttons.child(pill("Cerrar", TEXT_MUTED, |_| {
                    ModalController::get().close();
                }));
                if let Some(first) = ready.first().copied() {
                    buttons = buttons.child(pill("Abrir", ACCENT, move |_| {
                        ModalController::get().close();
                        space::open(TabTarget::Item(first), OpenHint::Focused);
                    }));
                }
            }
            Stage::Failed(_) => {
                buttons = buttons.child(pill("Cerrar", ERROR, |_| {
                    ModalController::get().close();
                }));
            }
        }
        card = card.child(buttons);
        card
    }
}

fn pill(
    text: &'static str,
    accent: Color,
    on_press: impl FnMut(Event<PressEventData>) + 'static,
) -> Rect {
    rect()
        .padding((8.0, 14.0))
        .background(CARD_BG)
        .with_corner_radius(8.0)
        .on_press(on_press)
        .child(label().color(accent).font_size(13.0).text(text))
}
