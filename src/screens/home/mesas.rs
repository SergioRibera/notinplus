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

/// Where a mesa was made, when it was not on this device.
fn origin(w: &Workspace) -> Option<String> {
    let device = w.device?;
    let sync = crate::sync::snapshot();
    if sync.device == Some(device) {
        return None;
    }
    Some(sync.trusted.iter().find(|d| d.id == device).map_or_else(
        || "En otro dispositivo".to_owned(),
        |d| format!("En {}", d.name),
    ))
}

/// An unnamed mesa with nothing open: a placeholder every device makes
/// for itself, not something to come back to.
fn is_blank(w: &Workspace) -> bool {
    !w.named && tab_count(w) == 0
}

/// Height of a mesa card.
pub const MESA_CARD: f32 = 104.0;
/// Height of the mesas block: its header and one row of cards.
pub const MESAS_BLOCK: f32 = MESA_CARD + 36.0;

/// One mesa: the panes drawn small, its name, what it holds and where
/// it comes from. The one to continue wears the accent.
#[derive(Clone, Debug, PartialEq)]
struct MesaCard {
    data: std::sync::Arc<LibraryData>,
    workspace: WorkspaceId,
    width: f32,
    current: bool,
}

impl MesaCard {
    /// What the card says: whether it is the one to continue, its name,
    /// its tabs and age, and where it was made.
    fn details(&self, w: &Workspace, p: &Palette) -> Rect {
        use freya::icons::lucide;
        let dot = w.color.map_or(p.accent, rgba);
        let from = origin(w);
        let now = crate::format::now();
        let id = w.id;
        rect()
            .width(Size::flex(1.0))
            .spacing(3.0)
            .child(
                rect()
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(6.0)
                    .child(
                        rect()
                            .width(Size::px(8.0))
                            .height(Size::px(8.0))
                            .corner_radius(tokens::radius::FULL)
                            .background(dot),
                    )
                    .child(
                        Text::new(if self.current { "CONTINUAR" } else { "MESA" })
                            .role(TextRole::Label)
                            .color(if self.current { p.accent_text } else { p.t3 }),
                    )
                    .maybe_child(w.pinned.then(|| {
                        Icon::new(lucide::pin())
                            .size(tokens::size::ICON_XS)
                            .color(p.t3)
                    })),
            )
            .child(
                Text::new(self.data.workspace_title(id))
                    .role(TextRole::Strong)
                    .lines(1),
            )
            .child(Text::new(mesa_meta(w, now)).role(TextRole::Meta).lines(1))
            .maybe_child(from.map(|from| {
                rect()
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(4.0)
                    .child(
                        Icon::new(lucide::monitor_smartphone())
                            .size(tokens::size::ICON_XS)
                            .color(p.t3),
                    )
                    .child(Text::new(from).role(TextRole::Meta).lines(1))
            }))
    }
}

impl Component for MesaCard {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let gesture = use_press_gesture();
        let mut hovering = use_state(|| false);
        let Some(w) = self.data.workspace(self.workspace) else {
            return rect().into_element();
        };
        let id = w.id;
        let data = std::sync::Arc::clone(&self.data);
        let border = if self.current {
            p.accent
        } else if hovering() {
            p.line_strong
        } else {
            p.line
        };
        let text = self.details(w, &p);
        let body = rect()
            .width(Size::px(self.width))
            .height(Size::px(MESA_CARD))
            .padding(10.0)
            .corner_radius(tokens::radius::LG)
            .background(if self.current {
                p.accent_soft.with_a(90)
            } else {
                p.s1
            })
            .border(
                Border::new()
                    .fill(border)
                    .width(if self.current { 1.5 } else { 1.0 })
                    .alignment(BorderAlignment::Inner),
            )
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(12.0)
            .a11y_role(AccessibilityRole::Button)
            .a11y_alt(self.data.workspace_title(id))
            .a11y_focusable(true)
            .on_pointer_enter(move |_| hovering.set(true))
            .on_pointer_leave(move |_| hovering.set(false))
            .child(
                rect()
                    .width(Size::px(if self.width < 320.0 { 80.0 } else { 112.0 }))
                    .height(Size::fill())
                    .padding(5.0)
                    .corner_radius(tokens::radius::MD)
                    .background(p.s2)
                    .child(miniature(&w.layout, &self.data, &p)),
            )
            .child(text);
        gesture
            .apply(
                body,
                move || crate::space::open_space(id),
                move |at| crate::actions::show(crate::actions::for_mesa(&data, id), at),
            )
            .into_element()
    }
}

/// The card that starts a mesa, filling the row while there are few.
fn new_mesa_card(width: f32) -> Element {
    let p = palette();
    rect()
        .width(Size::px(width))
        .height(Size::px(MESA_CARD))
        .corner_radius(tokens::radius::LG)
        .border(
            Border::new()
                .fill(p.line_strong)
                .width(1.0)
                .alignment(BorderAlignment::Inner),
        )
        .center()
        .spacing(6.0)
        .a11y_role(AccessibilityRole::Button)
        .a11y_alt("Nueva mesa")
        .on_press(|_| crate::flows::create::new_mesa())
        .child(
            Icon::new(freya::icons::lucide::plus())
                .size(20.0)
                .color(p.t2),
        )
        .child(Text::new("Nueva mesa").role(TextRole::Meta))
        .into_element()
}

/// The mesas as one row of equal cards: the one to continue first,
/// then the rest by pin and date. Three fit across on wide screens, the
/// rest scroll sideways; phones scroll a narrower card.
#[derive(Clone, Debug, PartialEq)]
pub struct MesaStrip {
    pub data: std::sync::Arc<LibraryData>,
    /// Width the row has to fill.
    pub width: f32,
    pub compact: bool,
}

impl MesaStrip {
    /// Cards that fit across without scrolling.
    const ACROSS: usize = 3;
    const GAP: f32 = 12.0;

    fn card_width(&self) -> f32 {
        if self.compact {
            (self.width * 0.82).min(300.0)
        } else {
            #[allow(clippy::cast_precision_loss)] // three cards
            let across = Self::ACROSS as f32;
            (Self::GAP.mul_add(-(across - 1.0), self.width) / across).floor()
        }
    }
}

impl Component for MesaStrip {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let current = continue_target(&self.data);
        let card = self.card_width();
        let mut mesas: Vec<(WorkspaceId, bool)> =
            current.map(|id| (id, true)).into_iter().collect();
        mesas.extend(
            others(&self.data, current)
                .into_iter()
                .filter(|w| !is_blank(w))
                .map(|w| (w.id, false)),
        );
        let shown = mesas.len();
        let mut cards: Vec<Element> = mesas
            .into_iter()
            .map(|(workspace, current)| {
                MesaCard {
                    data: std::sync::Arc::clone(&self.data),
                    workspace,
                    width: card,
                    current,
                }
                .into_element()
            })
            .collect();
        if shown < Self::ACROSS {
            cards.push(new_mesa_card(card));
        }
        rect()
            .width(Size::fill())
            .height(Size::px(MESAS_BLOCK))
            .spacing(8.0)
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(28.0))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .content(Content::Flex)
                    .child(Text::new("MESAS").role(TextRole::Label).color(p.t3).fill())
                    .child(
                        IconButton::new(freya::icons::lucide::plus(), "Nueva mesa")
                            .size(tokens::size::BUTTON_SM)
                            .on_press(|_| crate::flows::create::new_mesa()),
                    ),
            )
            .child(
                ScrollView::new()
                    .direction(Direction::Horizontal)
                    .width(Size::fill())
                    .height(Size::px(MESA_CARD))
                    .show_scrollbar(false)
                    .child(rect().horizontal().spacing(Self::GAP).children(cards)),
            )
    }
}
