//! Settings › Devices, and the two modals that link devices and
//! act on links: Link, and Scan or paste link.

use std::time::{Duration, Instant};

use freya::prelude::*;
use notin_ui_kit::theme::palette;
use notin_ui_kit::{
    Button, ButtonVariant, Group, Icon, InlineEdit, ListRow, ModalController, Notice, NoticeTone,
    Section, Segmented, Sheet, Text, TextField, TextRole, Trailing, tokens,
};

use notin_i18n::tr;
use notin_plugin::UserMessage;

use crate::sync::{LinkTarget, SyncSnapshot};
use notin_sync::TrustedDevice;

/// A pairing code is renewed after this long.
const CODE_LIFETIME: Duration = Duration::from_secs(5 * 60);
/// A second tap within this long confirms a destructive row action.
const CONFIRM_WINDOW: Duration = Duration::from_secs(3);
/// Side of the QR code.
const QR: f32 = 208.0;

/// SVG of a QR code carrying `text`, black on white for cameras.
#[sismo::function("settings::devices_qr_svg")]
fn qr_svg(text: &str) -> Option<Bytes> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(200, 200)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build();
    Some(Bytes::from(svg.into_bytes()))
}

/// The Devices section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Devices;

/// This device's name, renamed in place.
#[sismo::function("settings::devices_this_device")]
fn this_device(snapshot: &SyncSnapshot) -> InlineEdit {
    use freya::icons::lucide;
    InlineEdit::row(
        tr!("settings-devices-this-device"),
        snapshot.name.clone(),
        |text: String| crate::sync::set_device_name(text),
    )
    .placeholder(tr!("settings-devices-this-device"))
    .edit_label(tr!("common-rename"))
    .max_len(40)
    .icon(lucide::monitor_smartphone())
    .hint(tr!("settings-devices-this-device-hint"))
}

/// A linked device; Remove asks for a second tap.
#[sismo::function("settings::devices_linked_device")]
fn linked_device(
    snapshot: &SyncSnapshot,
    device: &TrustedDevice,
    mut confirm: State<Option<(String, Instant)>>,
) -> Element {
    let p = palette();
    let online = snapshot.online.contains(&device.id);
    let id = device.id;
    let key = format!("{id:?}");
    let armed = confirm
        .read()
        .as_ref()
        .is_some_and(|(k, at)| *k == key && at.elapsed() < CONFIRM_WINDOW);
    let since = crate::format::day(device.since, crate::format::now());
    ListRow::new(device.name.clone())
        .subtitle(if online {
            let path = snapshot.paths.get(&id).map_or_else(
                || tr!("collab-path-lan"),
                |p| crate::screens::collab::path_label(*p),
            );
            tr!("settings-devices-linked-online", "path" => path, "since" => since)
        } else {
            tr!("settings-devices-linked-offline", "since" => since)
        })
        .leading(notin_ui_kit::Leading::Element(
            rect()
                .width(Size::px(10.0))
                .height(Size::px(10.0))
                .corner_radius(tokens::radius::FULL)
                .background(if online { p.ok } else { p.t3 })
                .into_element(),
        ))
        .trailing(Trailing::Element(
            Button::new(if armed {
                tr!("settings-devices-confirm")
            } else {
                tr!("settings-devices-remove")
            })
            .variant(if armed {
                ButtonVariant::DangerFill
            } else {
                ButtonVariant::Ghost
            })
            .small()
            .on_press(move |_| {
                if armed {
                    confirm.set(None);
                    crate::sync::revoke(id);
                } else {
                    confirm.set(Some((key.clone(), Instant::now())));
                }
            })
            .into_element(),
        ))
        .into_element()
}

/// The relay server, changed in place.
#[sismo::function("settings::devices_server_row")]
fn server_row(snapshot: &SyncSnapshot) -> InlineEdit {
    use freya::icons::lucide;
    let relay = snapshot.relay.clone();
    // The key after the host is for the device, not for reading.
    let host = relay
        .as_deref()
        .map(|addr| addr.rsplit_once('/').map_or(addr, |(host, _)| host));
    let status = match (host, snapshot.relay_failure) {
        (Some(address), _) if snapshot.relay_online => {
            tr!("settings-devices-server-online", "address" => address)
        }
        (Some(address), Some(notin_sync::RelayFailure::Unresolved)) => {
            tr!("settings-devices-server-unresolved", "address" => address)
        }
        (Some(address), Some(notin_sync::RelayFailure::NoAnswer)) => {
            tr!("settings-devices-server-no-answer", "address" => address)
        }
        (Some(address), None) => tr!("settings-devices-server-offline", "address" => address),
        (None, _) => tr!("settings-devices-server-none"),
    };
    let before = relay.clone();
    InlineEdit::row(
        tr!("settings-devices-server"),
        relay.unwrap_or_default(),
        move |text: String| {
            let relay = (!text.is_empty()).then_some(text);
            if relay != before {
                crate::sync::set_relay(relay);
            }
        },
    )
    .shown_as(tr!("settings-devices-server"))
    .placeholder(tr!("settings-devices-server-placeholder"))
    .edit_label(tr!("settings-devices-change"))
    .allow_empty()
    .icon(lucide::server())
    .hint(status)
}

impl Component for Devices {
    #[sismo::function("settings::devices")]
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let snapshot = crate::sync::use_sync().read().clone();
        let confirm = use_state(|| None::<(String, Instant)>);
        let linked: Vec<Element> = snapshot
            .trusted
            .iter()
            .map(|device| linked_device(&snapshot, device, confirm))
            .collect();
        let none_linked = linked.is_empty();
        rect()
            .width(Size::fill())
            .spacing(24.0)
            .child(Group::default().child(this_device(&snapshot)))
            .child(
                Section::new(tr!("settings-devices-linked"))
                    .maybe_child(none_linked.then(|| {
                        Text::new(tr!("settings-devices-linked-empty")).role(TextRole::Hint)
                    }))
                    .maybe_child((!none_linked).then(|| Group::default().children(linked)))
                    .child(
                        Group::default()
                            .child(
                                ListRow::new(tr!("settings-devices-link-another"))
                                    .icon(lucide::link())
                                    .chevron()
                                    .on_press(|_| open_pairing()),
                            )
                            .child(
                                ListRow::new(tr!("settings-devices-scan-or-paste"))
                                    .icon(lucide::scan_qr_code())
                                    .chevron()
                                    .on_press(|_| open_link_sheet()),
                            ),
                    ),
            )
            .child(
                Section::new(tr!("settings-devices-sync"))
                    .child(Group::default().child(server_row(&snapshot))),
            )
            .maybe_child(
                snapshot
                    .error
                    .clone()
                    .map(|err| Notice::new(NoticeTone::Warning, err.user_message())),
            )
            .into_element()
    }
}

/// Open Link: show this device's code, or type another's.
#[sismo::function("settings::devices_open_pairing")]
pub fn open_pairing() {
    crate::sync::begin_pairing();
    Sheet::titled(tr!("settings-devices-pairing-title"))
        .width(tokens::size::SHEET_WIDTH_SM)
        .on_close(|()| crate::sync::cancel_pairing())
        .child(Pairing)
        .push();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pairing;

impl Component for Pairing {
    #[sismo::function("settings::pairing")]
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let sync = crate::sync::use_sync();
        let snapshot = sync.read().clone();
        let mut tab = use_state(|| 0usize);
        let trusted_before = use_hook(|| crate::sync::snapshot().trusted.len());
        let paired = snapshot.trusted.len() > trusted_before;
        let mut minted = use_state(Instant::now);
        let mut now = use_state(Instant::now);
        use_hook(move || {
            spawn(traced!("task::settings::pairing_clock", async move {
                loop {
                    async_io::Timer::after(Duration::from_secs(1)).await;
                    now.set(Instant::now());
                    if minted.peek().elapsed() >= CODE_LIFETIME {
                        crate::sync::begin_pairing();
                        minted.set(Instant::now());
                    }
                }
            }));
        });
        let left =
            CODE_LIFETIME.saturating_sub(now.read().saturating_duration_since(*minted.read()));

        let body = if paired {
            let name = snapshot
                .trusted
                .last()
                .map_or_else(String::new, |d| d.name.clone());
            rect()
                .width(Size::fill())
                .cross_align(Alignment::Center)
                .spacing(12.0)
                .padding(12.0)
                .child(Icon::new(lucide::circle_check()).size(44.0).color(p.ok))
                .child(
                    Text::new(tr!("settings-devices-paired", "name" => name)).role(TextRole::Title),
                )
                .child(
                    Text::new(tr!("settings-devices-paired-hint"))
                        .role(TextRole::Hint)
                        .align(TextAlign::Center),
                )
                .into_element()
        } else if *tab.read() == 0 {
            show_code(&snapshot, left)
        } else {
            EnterCode.into_element()
        };
        rect()
            .width(Size::fill())
            .spacing(16.0)
            .maybe_child((!paired).then(|| {
                Segmented::new(
                    [
                        tr!("settings-devices-pairing-show-code"),
                        tr!("settings-devices-pairing-have-code"),
                    ],
                    *tab.read(),
                )
                .full_width()
                .on_select(move |i: usize| tab.set(i))
            }))
            .child(body)
    }
}

#[sismo::function("settings::devices_show_code")]
fn show_code(snapshot: &SyncSnapshot, left: Duration) -> Element {
    let p = palette();
    let Some(offer) = snapshot.offer.as_ref() else {
        return rect()
            .width(Size::fill())
            .padding(24.0)
            .cross_align(Alignment::Center)
            .child(
                Text::new(snapshot.error.as_ref().map_or_else(
                    || tr!("settings-devices-pairing-preparing"),
                    UserMessage::user_message,
                ))
                .role(TextRole::Meta),
            )
            .into_element();
    };
    let link = offer.to_link();
    let digits = offer.code.to_string();
    let minutes = left.as_secs() / 60;
    let seconds = left.as_secs() % 60;
    rect()
        .width(Size::fill())
        .cross_align(Alignment::Center)
        .spacing(12.0)
        .child(
            Text::new(tr!("settings-devices-pairing-show-hint"))
                .role(TextRole::Hint)
                .align(TextAlign::Center),
        )
        // Without a server the code and the link only work nearby.
        .maybe_child(offer.relay.is_none().then(|| {
            Text::new(tr!("settings-devices-pairing-lan-only"))
                .role(TextRole::Meta)
                .align(TextAlign::Center)
        }))
        .maybe_child(qr_svg(&link).map(|svg| {
            rect()
                .width(Size::px(QR))
                .height(Size::px(QR))
                .corner_radius(tokens::radius::LG)
                .background(Color::WHITE)
                .center()
                .child(
                    SvgViewer::new(svg)
                        .width(Size::px(QR - 16.0))
                        .height(Size::px(QR - 16.0)),
                )
        }))
        .child(
            rect()
                .horizontal()
                .spacing(6.0)
                .children(digits.chars().enumerate().map(|(i, d)| {
                    rect()
                        .key(i)
                        .width(Size::px(38.0))
                        .height(Size::px(48.0))
                        .corner_radius(tokens::radius::MD)
                        .background(p.s2)
                        .center()
                        .child(
                            label()
                                .text(d.to_string())
                                .font_size(26.0)
                                .font_weight(FontWeight::BOLD)
                                .color(p.t1),
                        )
                        .into_element()
                })),
        )
        .child(
            Text::new(tr!(
                "settings-devices-pairing-renews",
                "time" => format!("{minutes}:{seconds:02}"),
            ))
            .role(TextRole::Meta),
        )
        .child(
            Button::new(tr!("settings-devices-pairing-copy-link"))
                .ghost()
                .small()
                .icon(freya::icons::lucide::copy())
                .on_press(move |_| {
                    if freya::clipboard::Clipboard::set(link.clone()).is_ok() {
                        notin_ui_kit::Toasts::get().show(notin_ui_kit::Toast::new(tr!(
                            "settings-devices-link-copied"
                        )));
                    }
                }),
        )
        .into_element()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EnterCode;

/// Six boxes showing the digits typed, the next one outlined.
#[sismo::function("settings::devices_code_boxes")]
fn code_boxes(digits: &str) -> Rect {
    let p = palette();
    rect()
        .width(Size::fill())
        .horizontal()
        .main_align(Alignment::Center)
        .spacing(6.0)
        .children((0..6).map(|i| {
            let digit = digits.chars().nth(i).map(String::from).unwrap_or_default();
            let active = i == digits.len();
            rect()
                .key(i)
                .width(Size::px(40.0))
                .height(Size::px(50.0))
                .corner_radius(tokens::radius::MD)
                .background(p.s2)
                .border(
                    Border::new()
                        .fill(if active { p.accent } else { p.line })
                        .width(if active { 2.0 } else { 1.0 })
                        .alignment(BorderAlignment::Inner),
                )
                .center()
                .child(
                    label()
                        .text(digit)
                        .font_size(24.0)
                        .font_weight(FontWeight::BOLD)
                        .color(p.t1),
                )
                .into_element()
        }))
}

impl Component for EnterCode {
    #[sismo::function("settings::enter_code")]
    fn render(&self) -> impl IntoElement {
        let snapshot = crate::sync::use_sync().read().clone();
        let code = use_state(String::new);
        let mut device = use_state(|| None::<notin_sync::DeviceId>);
        let mut sent = use_state(|| false);
        let digits: String = code
            .read()
            .chars()
            .filter(char::is_ascii_digit)
            .take(6)
            .collect();
        let chosen = (*device.read())
            .or_else(|| (snapshot.discovered.len() == 1).then(|| snapshot.discovered[0].device));
        let mut submit = move |digits: String| {
            if let (Some(target), Some(parsed)) = (chosen, notin_sync::PairingCode::parse(&digits))
            {
                sent.set(true);
                crate::sync::pair_discovered(target, parsed);
            }
        };
        if digits.len() == 6 && !sent() && chosen.is_some() {
            submit(digits.clone());
        }
        let failure = snapshot.last_pair_failure.map(|(_, why)| match why {
            notin_sync::PairFailure::Rejected => {
                tr!("settings-devices-pair-rejected")
            }
            notin_sync::PairFailure::Unreachable => {
                tr!("settings-devices-pair-unreachable")
            }
        });
        let peers = snapshot.discovered.iter().map(|peer| {
            let id = peer.device;
            ListRow::new(peer.name.clone())
                .icon(freya::icons::lucide::monitor_smartphone())
                .leading(notin_ui_kit::Leading::Check(chosen == Some(id)))
                .selected(chosen == Some(id))
                .on_press(move |_| device.set(Some(id)))
                .into_element()
        });
        let peers: Vec<Element> = peers.collect();
        let no_peers = peers.is_empty();
        let mut code_field = code;
        rect()
            .width(Size::fill())
            .spacing(12.0)
            .child(Text::new(tr!("settings-devices-pairing-enter-hint")).role(TextRole::Hint))
            .maybe_child(
                no_peers.then(|| {
                    Notice::new(NoticeTone::Info, tr!("settings-devices-pairing-searching"))
                }),
            )
            .maybe_child((!no_peers).then(|| Group::default().children(peers)))
            .child(code_boxes(&digits))
            .child(
                TextField::new(code)
                    .label(tr!("settings-devices-pairing-code"))
                    .placeholder("123456")
                    .max_len(12)
                    .auto_focus(true)
                    .on_change(move |text: String| {
                        let clean: String =
                            text.chars().filter(char::is_ascii_digit).take(6).collect();
                        if clean != text {
                            code_field.set(clean);
                        }
                        sent.set(false);
                    }),
            )
            .maybe_child(
                sent().then(|| {
                    Text::new(tr!("settings-devices-pairing-linking")).role(TextRole::Meta)
                }),
            )
            .maybe_child(failure.map(|why| Notice::new(NoticeTone::Danger, why)))
    }
}

/// Open Scan or paste link.
#[sismo::function("settings::devices_open_link_sheet")]
pub fn open_link_sheet() {
    Sheet::titled(tr!("settings-devices-scan-or-paste"))
        .child(LinkSheet)
        .push();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LinkSheet;

/// A link the sheet understood.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Recognized {
    Sync(LinkTarget),
    Document(String),
}

#[sismo::function("settings::devices_recognize")]
fn recognize(text: &str) -> Option<Recognized> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    crate::sync::recognize_link(text)
        .map(Recognized::Sync)
        .or_else(|| {
            crate::deep_link::scan_links(text)
                .next()
                .map(|_| Recognized::Document(text.to_owned()))
        })
}

/// What a recognised link does, with the button that does it.
#[sismo::function("settings::devices_link_card")]
fn link_card(target: &Recognized, raw: String) -> Rect {
    use freya::icons::lucide;
    let p = palette();
    let (icon, title, subtitle, action) = match target {
        Recognized::Sync(LinkTarget::Join { title, owner, .. }) => (
            lucide::users(),
            tr!("settings-devices-link-join-title", "title" => title.as_str()),
            tr!("settings-devices-link-join-from", "owner" => owner.as_str()),
            tr!("settings-devices-link-join"),
        ),
        Recognized::Sync(LinkTarget::AlreadyIn { title, .. }) => (
            lucide::users(),
            title.clone(),
            tr!("settings-devices-link-already-in"),
            tr!("common-open"),
        ),
        Recognized::Sync(LinkTarget::Pair { name }) => (
            lucide::link(),
            name.clone(),
            tr!("settings-devices-link-pair-hint"),
            tr!("settings-devices-link-pair"),
        ),
        Recognized::Document(_) => (
            lucide::file_text(),
            tr!("settings-devices-link-document"),
            tr!("settings-devices-link-document-hint"),
            tr!("common-open"),
        ),
    };
    rect()
        .width(Size::fill())
        .padding(14.0)
        .corner_radius(tokens::radius::LG)
        .background(p.accent_soft)
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::Flex)
        .spacing(12.0)
        .child(Icon::new(icon).size(22.0).color(p.accent_text))
        .child(
            rect()
                .width(Size::flex(1.0))
                .child(Text::new(title).role(TextRole::Strong).lines(1))
                .child(Text::new(subtitle).role(TextRole::Meta)),
        )
        .child(Button::new(action).primary().on_press(move |_| {
            ModalController::get().close();
            match recognize(&raw) {
                Some(Recognized::Sync(_)) => {
                    if let Some(LinkTarget::Join { .. }) = crate::sync::open_link(&raw) {
                        crate::screens::collab::joining();
                    }
                }
                Some(Recognized::Document(url)) => crate::deep_link::open(&url),
                None => {}
            }
        }))
}

/// Scan a QR code into `pasted`; a camera problem lands in `error`.
#[sismo::function("settings::devices_scan_button")]
fn scan_button(mut error: State<Option<String>>, mut pasted: State<String>) -> Button {
    Button::new(tr!("settings-devices-scan-qr"))
        .tonal()
        .full_width()
        .icon(freya::icons::lucide::scan_qr_code())
        .on_press(move |_| {
            let scan = crate::scan::scan_qr(&tr!("settings-devices-scan-prompt"));
            spawn(traced!("task::settings::scan_qr", async move {
                match scan.await {
                    Ok(Some(text)) => {
                        error.set(None);
                        pasted.set(text);
                    }
                    Ok(None) => {}
                    Err(err) => {
                        error.set(Some(
                            tr!("settings-devices-scan-error", "reason" => err.user_message()),
                        ));
                    }
                }
            }));
        })
}

impl Component for LinkSheet {
    #[sismo::function("settings::link_sheet")]
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let pasted = use_state(String::new);
        let scan_error = use_state(|| None::<String>);
        let clipboard = use_hook(|| {
            freya::clipboard::Clipboard::get()
                .ok()
                .filter(|text| recognize(text).is_some())
        });
        let text = pasted.read().clone();
        let found = recognize(&text);
        let typed_unknown = !text.trim().is_empty() && found.is_none();

        let card = found.map(|target| link_card(&target, text.clone()));

        let mut paste_target = pasted;
        rect()
            .width(Size::fill())
            .spacing(14.0)
            .child(scan_button(scan_error, paste_target))
            .maybe_child(
                scan_error
                    .read()
                    .clone()
                    .map(|why| Notice::new(NoticeTone::Warning, why)),
            )
            .maybe_child(clipboard.filter(|_| text.trim().is_empty()).map(|link| {
                ListRow::new(tr!("settings-devices-clipboard-link"))
                    .subtitle(link.clone())
                    .icon(lucide::clipboard())
                    .trailing(Trailing::Element(
                        Button::new(tr!("settings-devices-clipboard-use"))
                            .tonal()
                            .small()
                            .on_press(move |_| paste_target.set(link.clone()))
                            .into_element(),
                    ))
            }))
            .child(
                TextField::new(pasted)
                    .label(tr!("settings-devices-paste-label"))
                    .placeholder("notein://…")
                    .icon(lucide::link()),
            )
            .maybe_child(card)
            .maybe_child(typed_unknown.then(|| {
                Notice::new(NoticeTone::Danger, tr!("settings-devices-link-unknown"))
                    .detail(tr!("settings-devices-link-unknown-hint"))
            }))
    }
}
