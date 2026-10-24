//! The bar on top of a mesa: back, panel, name, search, who is here
//! and sharing.

use std::sync::Arc;

use freya::prelude::*;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{Avatar, Button, IconButton, Kbd, Text, TextRole, tokens, width_class};

use super::{Focus, MesaUi};
use crate::library::{ItemId, LibraryData, PaneTab, WorkspaceId};

/// Most avatars shown before "+N".
const AVATARS: usize = 3;

fn anchor(e: &Event<PressEventData>) -> CursorPoint {
    match e.data() {
        PressEventData::Mouse(m) => m.global_location,
        _ => CursorPoint::new(0.0, 0.0),
    }
}

/// People sharing any document of the mesa, online first.
fn people(docs: &[ItemId]) -> Vec<(String, bool)> {
    let snapshot = crate::sync::snapshot();
    let mut out: Vec<(String, bool)> = Vec::new();
    for share in snapshot
        .shares
        .iter()
        .filter(|s| s.docs.iter().any(|d| docs.contains(d)))
    {
        if !share.is_owner && !out.iter().any(|(n, _)| *n == share.owner.name) {
            out.push((share.owner.name.clone(), share.owner_online));
        }
        for (member, online) in &share.members {
            if Some(member.device) != snapshot.device && !out.iter().any(|(n, _)| *n == member.name)
            {
                out.push((member.name.clone(), *online));
            }
        }
    }
    out.sort_by_key(|(_, online)| !online);
    out
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct MesaBar {
    pub(super) data: Arc<LibraryData>,
    pub(super) mesa: WorkspaceId,
    pub(super) ui: MesaUi,
    pub(super) focus: Option<Focus>,
}

impl Component for MesaBar {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let _sync = crate::sync::use_sync().read().shares.len();
        let compact = width_class().is_compact();
        let mesa = self.mesa;
        let ui = self.ui;
        let title = self.data.workspace_title(mesa);
        let docs: Vec<ItemId> = self
            .data
            .workspace(mesa)
            .map(|w| {
                w.layout
                    .leaves()
                    .into_iter()
                    .flat_map(|p| p.tabs.iter())
                    .filter_map(PaneTab::item)
                    .collect()
            })
            .unwrap_or_default();
        let present = people(&docs);
        let more = present.len().saturating_sub(AVATARS);
        let dot = self
            .data
            .workspace(mesa)
            .and_then(|w| w.color)
            .map_or(p.accent, |c| Color::from_argb(c[3], c[0], c[1], c[2]));
        let data_menu = Arc::clone(&self.data);
        let data_palette = Arc::clone(&self.data);
        let focus_palette = self.focus.clone();
        let name = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(8.0)
            .padding((6.0, 10.0))
            .corner_radius(tokens::radius::MD)
            .a11y_role(AccessibilityRole::Button)
            .a11y_alt("Opciones de la mesa")
            .on_press(move |e: Event<PressEventData>| {
                crate::actions::show(crate::actions::for_mesa(&data_menu, mesa), anchor(&e));
            })
            .child(
                rect()
                    .width(Size::px(9.0))
                    .height(Size::px(9.0))
                    .corner_radius(tokens::radius::FULL)
                    .background(dot),
            )
            .child(
                rect()
                    .max_width(Size::px(if compact { 150.0 } else { 320.0 }))
                    .child(Text::new(title).role(TextRole::Strong).lines(1)),
            )
            .child(
                notein_ui_kit::Icon::new(lucide::chevron_down())
                    .size(14.0)
                    .color(p.t3),
            );
        let search = rect()
            .width(Size::px(260.0))
            .height(Size::px(tokens::size::BUTTON_SM))
            .padding((0.0, 10.0))
            .corner_radius(tokens::radius::MD)
            .background(p.s2)
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(8.0)
            .a11y_role(AccessibilityRole::SearchInput)
            .a11y_focusable(true)
            .on_press(move |_| {
                super::palette::open(
                    Arc::clone(&data_palette),
                    super::Ops { mesa },
                    ui,
                    focus_palette.clone(),
                );
            })
            .child(
                notein_ui_kit::Icon::new(lucide::search())
                    .size(14.0)
                    .color(p.t3),
            )
            .child(
                Text::new("Buscar en la mesa")
                    .role(TextRole::Meta)
                    .color(p.t3)
                    .fill(),
            )
            .child(Kbd::new("Ctrl K"));
        let avatars =
            (!present.is_empty()).then(|| {
                rect()
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(-6.0)
                    .a11y_alt(format!("{} en la mesa", present.len()))
                    .on_press(move |_| crate::flows::share_mesa(mesa))
                    .children(present.iter().take(AVATARS).enumerate().map(
                        |(i, (name, online))| {
                            let initials: String = name
                                .split_whitespace()
                                .filter_map(|w| w.chars().next())
                                .take(2)
                                .collect::<String>()
                                .to_uppercase();
                            rect()
                                .key(i)
                                .child(
                                    Avatar::new(
                                        initials,
                                        notein_ui_kit::theme::ACCENT_SWATCHES[(i + 1) % 8],
                                    )
                                    .size(28.0)
                                    .presence(*online),
                                )
                                .into_element()
                        },
                    ))
                    .maybe_child((more > 0).then(|| {
                        rect()
                            .width(Size::px(28.0))
                            .height(Size::px(28.0))
                            .corner_radius(tokens::radius::FULL)
                            .background(p.s3)
                            .center()
                            .child(Text::new(format!("+{more}")).role(TextRole::Meta))
                    }))
            });
        let panel_open = *self.ui.panel.read();
        rect()
            .width(Size::fill())
            .height(Size::px(tokens::size::BAR))
            .padding((0.0, tokens::space::SM))
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(4.0)
            .child(
                IconButton::new(lucide::arrow_left(), "Volver al inicio").on_press(|_| {
                    crate::screens::home::open_folder(crate::screens::home::Browse::get().folder())
                }),
            )
            .child(
                IconButton::new(lucide::panel_left(), "Panel del documento")
                    .active(panel_open)
                    .on_press(move |_| ui.toggle_panel()),
            )
            .child(
                rect()
                    .width(Size::flex(1.0))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .child(name),
            )
            .maybe_child((!compact).then_some(search))
            .maybe_child(compact.then(|| {
                IconButton::new(lucide::search(), "Buscar en la mesa").on_press({
                    let data = Arc::clone(&self.data);
                    let focus = self.focus.clone();
                    move |_| {
                        super::palette::open(
                            Arc::clone(&data),
                            super::Ops { mesa },
                            ui,
                            focus.clone(),
                        )
                    }
                })
            }))
            .maybe_child(avatars)
            .child(if compact {
                IconButton::new(lucide::share_2(), "Compartir")
                    .on_press(move |_| crate::flows::share_mesa(mesa))
                    .into_element()
            } else {
                Button::new("Compartir")
                    .tonal()
                    .small()
                    .icon(lucide::share_2())
                    .on_press(move |_| crate::flows::share_mesa(mesa))
                    .into_element()
            })
    }
}
