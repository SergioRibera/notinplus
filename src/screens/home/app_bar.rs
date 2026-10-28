//! The top of Home: name, the one search field, sync state, the trash
//! and settings.

use freya::prelude::*;
use notein_ui_kit::TapExt;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{IconButton, Tag, Text, TextRole, tokens, width_class};

use crate::sync::SyncSnapshot;

/// What the sync pill says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyncState {
    UpToDate,
    Syncing,
    Offline,
}

impl SyncState {
    /// `None` when no other device is linked: nothing to report.
    fn of(snapshot: &SyncSnapshot) -> Option<Self> {
        if snapshot.trusted.is_empty() && snapshot.shares.is_empty() {
            return None;
        }
        if snapshot.error.is_some() || (snapshot.online.is_empty() && !snapshot.relay_online) {
            return Some(Self::Offline);
        }
        let pending = snapshot
            .docs
            .values()
            .any(|s| *s == notein_sync::DocStatus::Pending);
        Some(if pending {
            Self::Syncing
        } else {
            Self::UpToDate
        })
    }

    const fn label(self) -> &'static str {
        match self {
            Self::UpToDate => "Al día",
            Self::Syncing => "Sincronizando",
            Self::Offline => "Sin conexión",
        }
    }
}

/// Home's app bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppBar {
    /// Things in the trash, counted on its button.
    pub trash: usize,
}

impl SyncState {
    /// A dot on phones, a tag with the label on wider screens.
    fn indicator(self, compact: bool) -> Element {
        let p = palette();
        let dot = match self {
            Self::UpToDate => p.ok,
            Self::Syncing => p.warn,
            Self::Offline => p.t3,
        };
        if compact {
            rect()
                .width(Size::px(10.0))
                .height(Size::px(10.0))
                .corner_radius(tokens::radius::FULL)
                .background(dot)
                .a11y_alt(self.label())
                .into_element()
        } else {
            Tag::new(self.label()).dot(dot).into_element()
        }
    }
}

/// The field look-alike that opens search.
fn search_field(compact: bool) -> Rect {
    let p = palette();
    rect()
        .width(if compact {
            Size::flex(1.0)
        } else {
            Size::px(420.0)
        })
        .height(Size::px(tokens::size::BUTTON))
        .padding((0.0, 12.0))
        .corner_radius(tokens::radius::MD)
        .background(p.s2)
        .border(
            Border::new()
                .fill(p.line)
                .width(1.0)
                .alignment(BorderAlignment::Inner),
        )
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::Flex)
        .spacing(8.0)
        .a11y_role(AccessibilityRole::SearchInput)
        .a11y_focusable(true)
        .on_tap(|_| crate::screens::search::open(String::new()))
        .child(
            notein_ui_kit::Icon::new(freya::icons::lucide::search())
                .size(tokens::size::ICON_SM)
                .color(p.t3),
        )
        .child(
            Text::new(if compact {
                "Buscar"
            } else {
                "Buscar documentos, versículos, notas…"
            })
            .role(TextRole::Meta)
            .color(p.t3)
            .lines(1)
            .fill(),
        )
        .maybe_child((!compact).then(|| notein_ui_kit::Kbd::new("Ctrl K")))
}

/// The app mark, with the name on wider screens.
fn brand(compact: bool) -> Rect {
    let p = palette();
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(8.0)
        .child(
            rect()
                .width(Size::px(28.0))
                .height(Size::px(28.0))
                .corner_radius(tokens::radius::SM)
                .background(p.accent)
                .center()
                .child(
                    notein_ui_kit::Icon::new(freya::icons::lucide::pen_line())
                        .size(16.0)
                        .color(p.on_accent),
                ),
        )
        .maybe_child((!compact).then(|| Text::new("Notein").role(TextRole::Title)))
}

impl Component for AppBar {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let compact = width_class().is_compact();
        let state = SyncState::of(&crate::sync::use_sync().read());
        rect()
            .width(Size::fill())
            .height(Size::px(tokens::size::BAR + 8.0))
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(tokens::space::MD)
            .child(brand(compact))
            .child(
                rect()
                    .width(Size::flex(1.0))
                    .horizontal()
                    .main_align(if compact {
                        Alignment::Start
                    } else {
                        Alignment::Center
                    })
                    .content(Content::Flex)
                    .child(search_field(compact)),
            )
            .maybe_child(crate::update::ui::chip(compact))
            .maybe_child(state.map(|s| s.indicator(compact)))
            .child(
                IconButton::new(lucide::trash_2(), "Papelera")
                    .badge(u32::try_from(self.trash).unwrap_or(u32::MAX))
                    .on_press(|_| crate::screens::trash::open()),
            )
            .child(
                IconButton::new(lucide::waypoints(), "Mapa de vínculos")
                    .on_press(|_| crate::screens::map::open()),
            )
            .child(
                IconButton::new(lucide::settings(), "Ajustes")
                    .on_press(|_| crate::screens::settings::open()),
            )
    }
}
