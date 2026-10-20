//! "Vincular dispositivo": shows this device's QR, link and six-digit
//! code so another device can pair with it.

use freya::icons::lucide;
use freya::prelude::*;
use notein_sync::PairingOffer;

use crate::components::theme::{ACCENT, CARD_BG, PANEL_BG, TEXT_MUTED, TEXT_PRIMARY};

use super::modal::{Modal, ModalController};

const SHEET_WIDTH: f32 = 420.0;
const QR_SIZE: f32 = 220.0;

/// Modal that renders the current pairing offer. It follows the sync
/// snapshot, so it shows the code once the service minted it and
/// closes itself when a device pairs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PairSheet;

impl PairSheet {
    pub fn open() {
        crate::sync::begin_pairing();
        ModalController::get().open(
            Modal::new(Self)
                .center()
                .width(SHEET_WIDTH)
                .dismiss_on_backdrop(true)
                .on_close(|| crate::sync::cancel_pairing()),
        );
    }
}

/// SVG bytes of a QR code carrying `text`, styled for the dark theme.
pub(crate) fn qr_bytes(text: &str) -> Option<Bytes> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(200, 200)
        .dark_color(qrcode::render::svg::Color("#f0f0f8"))
        .light_color(qrcode::render::svg::Color("#15151a"))
        .build();
    Some(Bytes::from(svg.into_bytes()))
}

impl Component for PairSheet {
    fn render(&self) -> impl IntoElement {
        let tick = use_state(|| 0u64);
        let _ = *tick.read();
        use_hook({
            let mut tick = tick;
            move || {
                let changes = crate::sync::changes();
                spawn(async move {
                    while changes.recv_async().await.is_ok() {
                        let next = tick.peek().wrapping_add(1);
                        tick.set(next);
                    }
                });
            }
        });
        let snapshot = crate::sync::snapshot();
        let trusted_before = use_hook(|| snapshot.trusted.len());
        if snapshot.trusted.len() > trusted_before {
            ModalController::get().close();
        }

        let mut card = rect()
            .vertical()
            .width(Size::fill())
            .padding((20.0, 22.0))
            .spacing(14.0)
            .background(PANEL_BG)
            .with_corner_radius(16.0)
            .cross_align(Alignment::Center)
            .child(
                label()
                    .color(TEXT_PRIMARY)
                    .font_size(17.0)
                    .text("Vincular dispositivo"),
            )
            .child(
                label()
                    .color(TEXT_MUTED)
                    .font_size(13.0)
                    .text_align(TextAlign::Center)
                    .text("En el otro dispositivo abrí Ajustes › Dispositivos y escaneá el código, pegá el enlace o elegí este dispositivo y escribí los seis dígitos."),
            );

        match snapshot.offer {
            Some(offer) => {
                if let Some(svg) = qr_bytes(&offer.to_link()) {
                    card = card.child(
                        rect()
                            .width(Size::px(QR_SIZE))
                            .height(Size::px(QR_SIZE))
                            .with_corner_radius(12.0)
                            .background(Color::from_rgb(21, 21, 26))
                            .center()
                            .child(
                                SvgViewer::new(svg)
                                    .width(Size::px(QR_SIZE - 16.0))
                                    .height(Size::px(QR_SIZE - 16.0)),
                            ),
                    );
                }
                card = card
                    .child(
                        label()
                            .color(TEXT_PRIMARY)
                            .font_size(34.0)
                            .text(format!("{}", offer.code)),
                    )
                    .child(
                        rect()
                            .width(Size::fill())
                            .padding((8.0, 12.0))
                            .background(CARD_BG)
                            .with_corner_radius(8.0)
                            .child(
                                label()
                                    .color(TEXT_MUTED)
                                    .font_size(10.0)
                                    .max_lines(2)
                                    .text_overflow(TextOverflow::Ellipsis)
                                    .text(offer.to_link()),
                            ),
                    );
            }
            None => {
                let text = snapshot
                    .error
                    .clone()
                    .unwrap_or_else(|| "Preparando el código…".to_owned());
                card = card.child(label().color(TEXT_MUTED).font_size(13.0).text(text));
            }
        }

        card.child(
            rect()
                .horizontal()
                .spacing(8.0)
                .cross_align(Alignment::Center)
                .padding((8.0, 14.0))
                .with_corner_radius(8.0)
                .background(CARD_BG)
                .on_press(|_| ModalController::get().close())
                .child(
                    SvgViewer::new(lucide::x())
                        .color(ACCENT)
                        .width(Size::px(14.0))
                        .height(Size::px(14.0)),
                )
                .child(label().color(ACCENT).font_size(13.0).text("Cerrar")),
        )
    }
}
