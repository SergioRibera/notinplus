//! The bar on top of a mesa: back, panel, name, search, who is here
//! and sharing.

use std::sync::Arc;

use freya::prelude::*;
use notein_i18n::tr;
use notein_ui_kit::TapExt;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{Avatar, Button, IconButton, Kbd, Text, TextRole, tokens, width_class};

use super::{Focus, MesaUi};
use crate::library::{ItemId, LibraryData, PaneTab, WorkspaceId};

/// Most avatars shown before "+N".
const AVATARS: usize = 3;

fn anchor(e: &Event<PressEventData>) -> CursorPoint {
    notein_ui_kit::press_point(e.data())
}

/// People sharing any document of the mesa, online first.
#[sismo::function("mesa::bar_people")]
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

impl MesaBar {
    #[sismo::function("mesa::bar_open_palette")]
    fn open_palette(&self) -> impl FnMut(Event<PressEventData>) + 'static {
        let (data, focus) = (Arc::clone(&self.data), self.focus.clone());
        let (mesa, ui) = (self.mesa, self.ui);
        move |_| super::palette::open(Arc::clone(&data), super::Ops { mesa }, ui, focus.as_ref())
    }

    /// The mesa's colour and name; pressing it shows the mesa actions.
    #[sismo::function("mesa::bar_name")]
    fn name(&self, compact: bool) -> Rect {
        let p = palette();
        let mesa = self.mesa;
        let data = Arc::clone(&self.data);
        let dot = self
            .data
            .workspace(mesa)
            .and_then(|w| w.color)
            .map_or(p.accent, |c| Color::from_argb(c[3], c[0], c[1], c[2]));
        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(8.0)
            .padding((6.0, 10.0))
            .corner_radius(tokens::radius::MD)
            .a11y_role(AccessibilityRole::Button)
            .a11y_alt(tr!("mesa-bar-options"))
            .on_tap(move |e: Event<PressEventData>| {
                crate::actions::show(crate::actions::for_mesa(&data, mesa), anchor(&e));
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
                    .child(
                        Text::new(self.data.workspace_title(mesa))
                            .role(TextRole::Strong)
                            .lines(1),
                    ),
            )
            .child(
                notein_ui_kit::Icon::new(freya::icons::lucide::chevron_down())
                    .size(14.0)
                    .color(p.t3),
            )
    }

    /// The search field look-alike that opens the palette.
    #[sismo::function("mesa::bar_search")]
    fn search(&self) -> Rect {
        let p = palette();
        rect()
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
            .on_tap(self.open_palette())
            .child(
                notein_ui_kit::Icon::new(freya::icons::lucide::search())
                    .size(14.0)
                    .color(p.t3),
            )
            .child(
                Text::new(tr!("mesa-bar-search"))
                    .role(TextRole::Meta)
                    .color(p.t3)
                    .fill(),
            )
            .child(Kbd::new("Ctrl K"))
    }

    /// Who else is on the mesa's documents; pressing opens sharing.
    #[sismo::function("mesa::bar_avatars")]
    fn avatars(&self) -> Option<Rect> {
        let p = palette();
        let mesa = self.mesa;
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
        (!present.is_empty()).then(|| {
            rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(-6.0)
                .a11y_alt(tr!("mesa-bar-people", "count" => present.len()))
                .on_tap(move |_| crate::flows::share_mesa(mesa))
                .children(
                    present
                        .iter()
                        .take(AVATARS)
                        .enumerate()
                        .map(|(i, (name, online))| {
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
                        }),
                )
                .maybe_child((more > 0).then(|| {
                    rect()
                        .width(Size::px(28.0))
                        .height(Size::px(28.0))
                        .corner_radius(tokens::radius::FULL)
                        .background(p.s3)
                        .center()
                        .child(Text::new(format!("+{more}")).role(TextRole::Meta))
                }))
        })
    }
}

impl Component for MesaBar {
    #[sismo::function("mesa::mesa_bar")]
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        // Re-render when shares change, for the avatars.
        let _sync = crate::sync::use_sync().read().shares.len();
        let compact = width_class().is_compact();
        let (mesa, ui) = (self.mesa, self.ui);
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
                IconButton::new(lucide::arrow_left(), tr!("mesa-bar-back-home")).on_press(|_| {
                    crate::screens::home::open_folder(crate::screens::home::Browse::get().folder());
                }),
            )
            .child(
                IconButton::new(lucide::panel_left(), tr!("mesa-document-panel"))
                    .active(panel_open)
                    .on_press(move |_| ui.toggle_panel()),
            )
            .child(
                rect()
                    .width(Size::flex(1.0))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .child(self.name(compact)),
            )
            .child(if compact {
                IconButton::new(lucide::search(), tr!("mesa-bar-search"))
                    .on_press(self.open_palette())
                    .into_element()
            } else {
                self.search().into_element()
            })
            .maybe_child(self.avatars())
            .child(if compact {
                IconButton::new(lucide::share_2(), tr!("common-share"))
                    .on_press(move |_| crate::flows::share_mesa(mesa))
                    .into_element()
            } else {
                Button::new(tr!("common-share"))
                    .tonal()
                    .small()
                    .icon(lucide::share_2())
                    .on_press(move |_| crate::flows::share_mesa(mesa))
                    .into_element()
            })
    }
}
