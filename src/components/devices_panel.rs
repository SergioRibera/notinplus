//! Settings › Dispositivos: this device's name, pairing, the devices
//! seen on the network and the trusted ones.

use freya::icons::lucide;
use freya::prelude::*;
use notein_sync::{DeviceId, PairFailure, PairingCode, PeerInfo, TrustedDevice};

use crate::components::form_input::FormInput;
use crate::components::theme::{ACCENT, BORDER, CARD_BG, ERROR, TEXT_MUTED, TEXT_PRIMARY};
use crate::sync::{self, SyncSnapshot};

use super::pair_sheet::PairSheet;

/// The whole section body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DevicesPanel;

impl Component for DevicesPanel {
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
        let snapshot = sync::snapshot();
        let name = use_state(|| snapshot.name.clone());
        let relay = use_state(|| snapshot.relay.clone().unwrap_or_default());
        let link = use_state(String::new);
        let code = use_state(String::new);
        let chosen = use_state(|| Option::<DeviceId>::None);

        let mut column = rect()
            .vertical()
            .width(Size::fill())
            .spacing(12.0)
            .child(this_device(&snapshot, name));
        if let Some(error) = &snapshot.error {
            column = column.child(notice(error.clone(), ERROR));
        }
        column
            .child(pairing_card(&snapshot, link, code, chosen))
            .child(trusted_card(&snapshot))
            .child(relay_card(&snapshot, relay))
    }
}

fn card_title(text: &'static str) -> Label {
    label().color(TEXT_PRIMARY).font_size(15.0).text(text)
}

fn card() -> Rect {
    rect()
        .vertical()
        .width(Size::fill())
        .padding((16.0, 18.0))
        .spacing(10.0)
        .background(CARD_BG)
        .with_corner_radius(12.0)
}

fn notice(text: String, color: Color) -> Rect {
    rect()
        .width(Size::fill())
        .padding((10.0, 14.0))
        .with_corner_radius(10.0)
        .border(Border::new().width(1.0).fill(color))
        .child(label().color(color).font_size(12.0).text(text))
}

fn pill(text: &'static str, on_press: impl FnMut(Event<PressEventData>) + 'static) -> Rect {
    rect()
        .padding((8.0, 14.0))
        .background(Color::from_rgb(46, 46, 56))
        .with_corner_radius(8.0)
        .on_press(on_press)
        .child(label().color(ACCENT).font_size(13.0).text(text))
}

fn this_device(snapshot: &SyncSnapshot, name: State<String>) -> Rect {
    let id = snapshot
        .device
        .map_or_else(|| "sin identidad".to_owned(), |d| d.short());
    card()
        .child(card_title("Este dispositivo"))
        .child(label().color(TEXT_MUTED).font_size(12.0).text(format!(
            "Identidad {id} · los demás lo verán con el nombre de abajo."
        )))
        .child(
            rect()
                .horizontal()
                .content(Content::Flex)
                .width(Size::fill())
                .spacing(10.0)
                .cross_align(Alignment::Center)
                .child(
                    FormInput::new(name)
                        .placeholder("Nombre del dispositivo")
                        .width(Size::flex(1.0)),
                )
                .child(pill("Guardar", move |_| {
                    let value = name.read().trim().to_owned();
                    if !value.is_empty() {
                        sync::set_device_name(value);
                    }
                })),
        )
}

fn pairing_card(
    snapshot: &SyncSnapshot,
    mut link: State<String>,
    mut code: State<String>,
    chosen: State<Option<DeviceId>>,
) -> Rect {
    let mut card = card()
        .child(card_title("Vincular dispositivo"))
        .child(
            label()
                .color(TEXT_MUTED)
                .font_size(12.0)
                .text("Dos dispositivos vinculados se consideran el mismo usuario y mantienen la biblioteca igual."),
        )
        .child(
            rect()
                .horizontal()
                .spacing(10.0)
                .child(pill("Mostrar mi código", |_| PairSheet::open())),
        );

    if let Some(target) = &snapshot.pairing_with {
        card = card.child(notice(format!("Vinculando con {target}…"), ACCENT));
    }
    if let Some((_, reason)) = snapshot.last_pair_failure {
        let text = match reason {
            PairFailure::Rejected => "El código no coincide o venció. Pedí uno nuevo.",
            PairFailure::Unreachable => "No se pudo alcanzar al otro dispositivo.",
        };
        card = card.child(notice(text.to_owned(), ERROR));
    }

    card = card.child(divider()).child(
        label()
            .color(TEXT_PRIMARY)
            .font_size(13.0)
            .text("Pegar enlace de vinculación o invitación"),
    );
    card = card.child(
        rect()
            .horizontal()
            .content(Content::Flex)
            .width(Size::fill())
            .spacing(10.0)
            .cross_align(Alignment::Center)
            .child(
                FormInput::new(link)
                    .placeholder("notein://pair?… o notein://join?…")
                    .width(Size::flex(1.0)),
            )
            .child(pill("Vincular", move |_| {
                let text = link.read().clone();
                if sync::open_link(&text).is_some() {
                    link.set(String::new());
                }
            })),
    );

    card = card.child(divider()).child(
        label()
            .color(TEXT_PRIMARY)
            .font_size(13.0)
            .text("Dispositivos en esta red"),
    );
    let trusted: Vec<DeviceId> = snapshot.trusted.iter().map(|d| d.id).collect();
    let candidates: Vec<&PeerInfo> = snapshot
        .discovered
        .iter()
        .filter(|p| !trusted.contains(&p.device))
        .collect();
    if candidates.is_empty() {
        card = card.child(label().color(TEXT_MUTED).font_size(12.0).text(
            "Todavía no se ve ningún otro dispositivo. Abrí la app en el otro, en la misma red.",
        ));
    }
    for peer in candidates {
        let device = peer.device;
        let selected = *chosen.read() == Some(device);
        let mut row = rect()
            .vertical()
            .width(Size::fill())
            .spacing(8.0)
            .padding((10.0, 12.0))
            .with_corner_radius(10.0)
            .background(Color::from_rgb(30, 30, 38))
            .child(
                rect()
                    .horizontal()
                    .content(Content::Flex)
                    .width(Size::fill())
                    .spacing(10.0)
                    .cross_align(Alignment::Center)
                    .child(
                        SvgViewer::new(lucide::smartphone())
                            .color(TEXT_PRIMARY)
                            .width(Size::px(16.0))
                            .height(Size::px(16.0)),
                    )
                    .child(
                        label()
                            .color(TEXT_PRIMARY)
                            .font_size(13.0)
                            .text(peer.name.clone()),
                    )
                    .child(
                        label()
                            .color(TEXT_MUTED)
                            .font_size(11.0)
                            .text(peer.addr.to_string()),
                    )
                    .child(rect().width(Size::flex(1.0)))
                    .child(pill(
                        if selected {
                            "Cancelar"
                        } else {
                            "Tengo su código"
                        },
                        move |_| {
                            let mut chosen = chosen;
                            chosen.set(if selected { None } else { Some(device) });
                        },
                    )),
            );
        if selected {
            row = row.child(
                rect()
                    .horizontal()
                    .width(Size::fill())
                    .spacing(10.0)
                    .cross_align(Alignment::Center)
                    .child(
                        FormInput::new(code)
                            .placeholder("Seis dígitos")
                            .width(Size::px(140.0)),
                    )
                    .child(pill("Vincular", move |_| {
                        let text = code.read().clone();
                        if let Some(parsed) = PairingCode::parse(&text) {
                            sync::pair_discovered(device, parsed);
                            code.set(String::new());
                        }
                    })),
            );
        }
        card = card.child(row);
    }
    card
}

fn trusted_card(snapshot: &SyncSnapshot) -> Rect {
    let mut card = card().child(card_title("Dispositivos de confianza"));
    if snapshot.trusted.is_empty() {
        return card.child(
            label()
                .color(TEXT_MUTED)
                .font_size(12.0)
                .text("Ninguno todavía."),
        );
    }
    for device in &snapshot.trusted {
        let online = snapshot.online.contains(&device.id);
        card = card.child(trusted_row(device, online));
    }
    card
}

fn trusted_row(device: &TrustedDevice, online: bool) -> Rect {
    let id = device.id;
    let dot = if online {
        Color::from_rgb(90, 200, 120)
    } else {
        Color::from_rgb(90, 90, 100)
    };
    rect()
        .horizontal()
        .content(Content::Flex)
        .width(Size::fill())
        .spacing(10.0)
        .cross_align(Alignment::Center)
        .child(
            rect()
                .width(Size::px(10.0))
                .height(Size::px(10.0))
                .with_corner_radius(5.0)
                .background(dot),
        )
        .child(
            label()
                .color(TEXT_PRIMARY)
                .font_size(13.0)
                .text(device.name.clone()),
        )
        .child(label().color(TEXT_MUTED).font_size(11.0).text(if online {
            "en línea"
        } else {
            "sin conexión"
        }))
        .child(rect().width(Size::flex(1.0)))
        .child(pill("Quitar", move |_| sync::revoke(id)))
}

fn relay_card(snapshot: &SyncSnapshot, relay: State<String>) -> Rect {
    let status = match (&snapshot.relay, snapshot.relay_online) {
        (None, _) => "Sin servidor: solo funciona en la misma red.".to_owned(),
        (Some(_), true) => "Conectado.".to_owned(),
        (Some(_), false) => "Configurado, sin conexión por ahora.".to_owned(),
    };
    card()
        .child(card_title("Servidor"))
        .child(hint_text(
            "Un relevo siempre encendido guarda los cambios cifrados de los compartidos permanentes y los entrega cuando cada persona vuelve. Pegá su dirección como `host:puerto/clave`.",
        ))
        .child(
            rect()
                .horizontal()
                .content(Content::Flex)
                .width(Size::fill())
                .spacing(10.0)
                .cross_align(Alignment::Center)
                .child(
                    FormInput::new(relay)
                        .placeholder("relay.ejemplo.org:7720/…")
                        .width(Size::flex(1.0)),
                )
                .child(pill("Guardar", move |_| {
                    let value = relay.read().trim().to_owned();
                    sync::set_relay((!value.is_empty()).then_some(value));
                })),
        )
        .child(hint_text(status))
}

fn hint_text(text: impl Into<String>) -> Label {
    label().color(TEXT_MUTED).font_size(12.0).text(text.into())
}

fn divider() -> Rect {
    rect()
        .width(Size::fill())
        .height(Size::px(1.0))
        .background(BORDER)
}
