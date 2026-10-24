//! "Continuar" and the mesas: where the user left off, and every other
//! arrangement of open documents.

use freya::prelude::*;
use notein_ui_kit::theme::{Palette, palette};
use notein_ui_kit::{Icon, IconButton, Text, TextRole, tokens, use_press_gesture};

use crate::library::{LibraryData, PaneLayout, SplitOrientation, Workspace, WorkspaceId};
use crate::references::kind_color;

const fn rgba(color: [u8; 4]) -> Color {
    Color::from_argb(color[3], color[0], color[1], color[2])
}

/// The mesa Home offers to continue: the active one, else the newest.
#[must_use]
pub fn continue_target(data: &LibraryData) -> Option<WorkspaceId> {
    data.active_workspace()
        .filter(|id| data.workspace(*id).is_some())
        .or_else(|| {
            data.workspaces()
                .iter()
                .max_by_key(|w| w.updated_at)
                .map(|w| w.id)
        })
}

/// The other mesas: pinned first, then newest.
#[must_use]
pub fn others(data: &LibraryData, except: Option<WorkspaceId>) -> Vec<&Workspace> {
    let mut list: Vec<&Workspace> = data
        .workspaces()
        .iter()
        .filter(|w| Some(w.id) != except)
        .collect();
    list.sort_by_key(|w| (!w.pinned, std::cmp::Reverse(w.updated_at)));
    list
}

fn tab_count(w: &Workspace) -> usize {
    w.layout.leaves().iter().map(|p| p.tabs.len()).sum()
}

fn mesa_meta(w: &Workspace, now: u64) -> String {
    format!(
        "{} · {}",
        crate::format::count(tab_count(w), "pestaña", "pestañas"),
        crate::format::ago(w.updated_at, now)
    )
}

/// Boxes of the panes of `layout`, drawn small.
fn miniature(layout: &PaneLayout, data: &LibraryData, p: &Palette) -> Element {
    match layout {
        PaneLayout::Leaf(pane) => {
            let kind = pane
                .tabs
                .get(pane.active_tab as usize)
                .and_then(crate::library::PaneTab::item)
                .and_then(|id| data.item(id))
                .and_then(|i| crate::plugins::kind_info(&i.kind.plugin_id));
            let tint = kind.map_or(p.t3, |k| kind_color(p, k.color));
            rect()
                .width(Size::fill())
                .height(Size::fill())
                .corner_radius(tokens::radius::XS)
                .background(p.s3)
                .padding(6.0)
                .spacing(4.0)
                .child(
                    rect()
                        .width(Size::percent(40.0))
                        .height(Size::px(4.0))
                        .corner_radius(2.0)
                        .background(tint),
                )
                .children((0..3).map(|i| {
                    rect()
                        .key(i)
                        .width(Size::percent(if i == 2 { 55.0 } else { 85.0 }))
                        .height(Size::px(3.0))
                        .corner_radius(2.0)
                        .background(p.line_strong)
                        .into_element()
                }))
                .into_element()
        }
        PaneLayout::Split {
            orientation,
            ratio,
            first,
            second,
        } => {
            let first_share = ratio.clamp(0.1, 0.9);
            let container = rect()
                .width(Size::fill())
                .height(Size::fill())
                .content(Content::Flex)
                .spacing(3.0);
            let container = match orientation {
                SplitOrientation::Horizontal => container.horizontal(),
                SplitOrientation::Vertical => container.vertical(),
            };
            let size = |share: f32| match orientation {
                SplitOrientation::Horizontal => (Size::flex(share), Size::fill()),
                SplitOrientation::Vertical => (Size::fill(), Size::flex(share)),
            };
            let (w1, h1) = size(first_share);
            let (w2, h2) = size(1.0 - first_share);
            container
                .child(rect().width(w1).height(h1).child(miniature(first, data, p)))
                .child(
                    rect()
                        .width(w2)
                        .height(h2)
                        .child(miniature(second, data, p)),
                )
                .into_element()
        }
    }
}

/// The big "Continuar" card.
#[derive(Clone, Debug, PartialEq)]
pub struct ContinueHero {
    pub data: std::sync::Arc<LibraryData>,
    pub workspace: WorkspaceId,
}

impl Component for ContinueHero {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let gesture = use_press_gesture();
        let data = &self.data;
        let Some(w) = data.workspace(self.workspace) else {
            return rect().into_element();
        };
        let id = w.id;
        let now = crate::format::now();
        let chips = w
            .layout
            .leaves()
            .into_iter()
            .flat_map(|pane| pane.tabs.iter())
            .filter_map(crate::library::PaneTab::item)
            .filter_map(|item| data.item(item))
            .take(4)
            .map(|item| {
                let tint = crate::plugins::kind_info(&item.kind.plugin_id)
                    .map_or(p.t3, |k| kind_color(&p, k.color));
                notein_ui_kit::Tag::new(item.name.clone())
                    .dot(tint)
                    .into_element()
            })
            .collect::<Vec<_>>();
        let data_for_menu = std::sync::Arc::clone(data);
        let surface = rect()
            .width(Size::fill())
            .padding(tokens::space::LG)
            .corner_radius(tokens::radius::XL)
            .background(p.s1)
            .border(
                Border::new()
                    .fill(p.line)
                    .width(1.0)
                    .alignment(BorderAlignment::Inner),
            )
            .spacing(tokens::space::MD)
            .a11y_role(AccessibilityRole::Button)
            .a11y_focusable(true)
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(150.0))
                    .padding(8.0)
                    .corner_radius(tokens::radius::LG)
                    .background(p.s2)
                    .child(miniature(&w.layout, data, &p)),
            )
            .child(Text::new("CONTINUAR").role(TextRole::Label).color(p.t3))
            .child(
                Text::new(data.workspace_title(id))
                    .role(TextRole::Title)
                    .lines(2),
            )
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::wrap_spacing(6.0))
                    .spacing(6.0)
                    .children(chips),
            )
            .child(
                Text::new(format!(
                    "{} · {}",
                    crate::format::ago(w.updated_at, now),
                    crate::format::count(tab_count(w), "documento", "documentos")
                ))
                .role(TextRole::Meta),
            );
        gesture
            .apply(
                surface,
                move || crate::space::open_space(id),
                move |at| crate::actions::show(crate::actions::for_mesa(&data_for_menu, id), at),
            )
            .into_element()
    }
}

/// One mesa row (tablet and desktop) or tile (phone).
#[derive(Clone, Debug, PartialEq)]
struct MesaRow {
    data: std::sync::Arc<LibraryData>,
    workspace: WorkspaceId,
    compact: bool,
}

impl Component for MesaRow {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let gesture = use_press_gesture();
        let Some(w) = self.data.workspace(self.workspace) else {
            return rect().into_element();
        };
        let id = w.id;
        let dot = w.color.map_or(p.accent, rgba);
        let title = self.data.workspace_title(id);
        let meta = mesa_meta(w, crate::format::now());
        let pinned = w.pinned;
        let data = std::sync::Arc::clone(&self.data);
        let body = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(10.0)
            .padding((10.0, 12.0))
            .corner_radius(tokens::radius::LG)
            .background(p.s1)
            .a11y_role(AccessibilityRole::Button)
            .a11y_focusable(true)
            .width(if self.compact {
                Size::px(220.0)
            } else {
                Size::fill()
            })
            .child(
                rect()
                    .width(Size::px(10.0))
                    .height(Size::px(10.0))
                    .corner_radius(tokens::radius::FULL)
                    .background(dot),
            )
            .child(
                rect()
                    .width(Size::flex(1.0))
                    .spacing(2.0)
                    .child(Text::new(title).role(TextRole::Strong).lines(1))
                    .child(Text::new(meta).role(TextRole::Meta).lines(1)),
            )
            .maybe_child(pinned.then(|| {
                Icon::new(lucide::pin())
                    .size(tokens::size::ICON_XS)
                    .color(p.t3)
            }));
        gesture
            .apply(
                body,
                move || crate::space::open_space(id),
                move |at| crate::actions::show(crate::actions::for_mesa(&data, id), at),
            )
            .into_element()
    }
}

/// The list of mesas beside the hero, or the phone carousel.
#[derive(Clone, Debug, PartialEq)]
pub struct MesaList {
    pub data: std::sync::Arc<LibraryData>,
    pub except: Option<WorkspaceId>,
    pub compact: bool,
}

impl Component for MesaList {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let rows: Vec<Element> = others(&self.data, self.except)
            .into_iter()
            .map(|w| {
                MesaRow {
                    data: std::sync::Arc::clone(&self.data),
                    workspace: w.id,
                    compact: self.compact,
                }
                .into_element()
            })
            .collect();
        if self.compact {
            let add = rect()
                .width(Size::px(56.0))
                .height(Size::px(56.0))
                .corner_radius(tokens::radius::LG)
                .background(p.s2)
                .border(
                    Border::new()
                        .fill(p.line_strong)
                        .width(1.0)
                        .alignment(BorderAlignment::Inner),
                )
                .center()
                .a11y_role(AccessibilityRole::Button)
                .a11y_alt("Nueva mesa")
                .on_press(|_| crate::flows::create::new_mesa())
                .child(Icon::new(lucide::plus()).size(20.0).color(p.t1));
            return rect()
                .width(Size::fill())
                .spacing(8.0)
                .child(Text::new("MESAS").role(TextRole::Label).color(p.t3))
                .child(
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .content(Content::Flex)
                        .spacing(8.0)
                        .child(add)
                        .child(
                            ScrollView::new()
                                .direction(Direction::Horizontal)
                                .width(Size::flex(1.0))
                                .height(Size::px(64.0))
                                .show_scrollbar(false)
                                .child(
                                    rect()
                                        .horizontal()
                                        .cross_align(Alignment::Center)
                                        .spacing(8.0)
                                        .children(rows),
                                ),
                        ),
                )
                .into_element();
        }
        rect()
            .width(Size::fill())
            .spacing(6.0)
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .content(Content::Flex)
                    .child(Text::new("MESAS").role(TextRole::Label).color(p.t3).fill())
                    .child(
                        IconButton::new(lucide::plus(), "Nueva mesa")
                            .size(tokens::size::BUTTON_SM)
                            .on_press(|_| crate::flows::create::new_mesa()),
                    ),
            )
            .maybe_child(rows.is_empty().then(|| {
                Text::new("Abrí documentos juntos y quedan acá como una mesa.").role(TextRole::Hint)
            }))
            .children(rows)
            .into_element()
    }
}
