//! "Continuar" and the mesas: where the user left off, and every other
//! arrangement of open documents.

use freya::prelude::*;
use notein_ui_kit::theme::{Palette, palette};
use notein_ui_kit::{Button, Icon, IconButton, Text, TextRole, tokens, use_press_gesture};

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

/// Height of the card of the mesa to continue.
pub const MESA_CARD: f32 = 104.0;

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
            .on_pointer_enter(move |e: Event<PointerEventData>| {
                hovering.set(notein_ui_kit::is_mouse(&e));
            })
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

/// The mesas Home lists: the one to continue, then the others that hold
/// something, pinned first and newest next.
struct Listed {
    current: Option<WorkspaceId>,
    rest: Vec<WorkspaceId>,
}

impl From<&LibraryData> for Listed {
    fn from(data: &LibraryData) -> Self {
        let current = continue_target(data);
        let rest = others(data, current)
            .into_iter()
            .filter(|w| !is_blank(w))
            .map(|w| w.id)
            .collect();
        Self { current, rest }
    }
}

/// One mesa as a row of the list beside "Continuar".
#[derive(Clone, Debug, PartialEq)]
struct MesaRow {
    data: std::sync::Arc<LibraryData>,
    workspace: WorkspaceId,
    height: f32,
}

impl Component for MesaRow {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let gesture = use_press_gesture();
        let mut hovering = use_state(|| false);
        let Some(w) = self.data.workspace(self.workspace) else {
            return rect().into_element();
        };
        let id = w.id;
        let data = std::sync::Arc::clone(&self.data);
        let title = self.data.workspace_title(id);
        let row = rect()
            .width(Size::fill())
            .height(Size::px(self.height))
            .padding((0.0, 10.0))
            .corner_radius(tokens::radius::MD)
            .background(if hovering() { p.s2 } else { Color::TRANSPARENT })
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(10.0)
            .a11y_role(AccessibilityRole::Button)
            .a11y_alt(title.clone())
            .a11y_focusable(true)
            .on_pointer_enter(move |e: Event<PointerEventData>| {
                hovering.set(notein_ui_kit::is_mouse(&e));
            })
            .on_pointer_leave(move |_| hovering.set(false))
            .child(
                rect()
                    .width(Size::px(8.0))
                    .height(Size::px(8.0))
                    .corner_radius(tokens::radius::FULL)
                    .background(w.color.map_or(p.t3, rgba)),
            )
            .child(Text::new(title).role(TextRole::Strong).lines(1).fill())
            .maybe_child(w.pinned.then(|| {
                Icon::new(freya::icons::lucide::pin())
                    .size(tokens::size::ICON_XS)
                    .color(p.t3)
            }))
            .child(
                Text::new(mesa_meta(w, crate::format::now()))
                    .role(TextRole::Meta)
                    .lines(1),
            );
        gesture
            .apply(
                row,
                move || crate::space::open_space(id),
                move |at| crate::actions::show(crate::actions::for_mesa(&data, id), at),
            )
            .into_element()
    }
}

/// The row that starts a mesa when there is no other to list.
fn new_mesa_row(height: f32) -> Element {
    let p = palette();
    rect()
        .width(Size::fill())
        .height(Size::px(height))
        .padding((0.0, 10.0))
        .corner_radius(tokens::radius::MD)
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(10.0)
        .a11y_role(AccessibilityRole::Button)
        .a11y_alt("Nueva mesa")
        .on_press(|_| crate::flows::create::new_mesa())
        .child(
            Icon::new(freya::icons::lucide::plus())
                .size(tokens::size::ICON_SM)
                .color(p.t2),
        )
        .child(Text::new("Nueva mesa").role(TextRole::Meta))
        .into_element()
}

/// The mesas: a card for the one to continue and, beside it (below on
/// phones), a short list of the rest that "Ver todas" unfolds in place.
#[derive(Clone, PartialEq)]
pub struct MesaStrip {
    pub data: std::sync::Arc<LibraryData>,
    /// Width the block has to fill.
    pub width: f32,
    pub compact: bool,
    /// Whether every mesa is listed, not only the first few.
    pub expanded: State<bool>,
}

impl std::fmt::Debug for MesaStrip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MesaStrip")
            .field("width", &self.width)
            .field("compact", &self.compact)
            .finish_non_exhaustive()
    }
}

impl MesaStrip {
    /// Rows listed while folded.
    const SHOWN: usize = 3;
    /// The section label and the space under it.
    const HEADER: f32 = 36.0;
    /// The "Ver todas" button.
    const MORE: f32 = 32.0;
    const GAP: f32 = 12.0;

    const fn row_height(compact: bool) -> f32 {
        if compact {
            tokens::size::HIT
        } else {
            tokens::size::HIT_DESKTOP
        }
    }

    /// Rows the list shows: the other mesas, or the one that starts a
    /// mesa when the one to continue is alone.
    fn rows(listed: &Listed, expanded: bool) -> usize {
        match (listed.current, listed.rest.len()) {
            (None, _) => 0,
            (Some(_), 0) => 1,
            (Some(_), n) if expanded => n,
            (Some(_), n) => n.min(Self::SHOWN),
        }
    }

    fn list_height(listed: &Listed, compact: bool, expanded: bool) -> f32 {
        #[allow(clippy::cast_precision_loss)] // a handful of rows
        let rows = Self::rows(listed, expanded) as f32;
        let more = if listed.rest.len() > Self::SHOWN {
            Self::MORE
        } else {
            0.0
        };
        rows.mul_add(Self::row_height(compact), more)
    }

    /// Height of the whole block for `data`, as laid out by `render`.
    #[must_use]
    pub fn height(data: &LibraryData, compact: bool, expanded: bool) -> f32 {
        let listed = Listed::from(data);
        let list = Self::list_height(&listed, compact, expanded);
        let body = if !compact {
            MESA_CARD.max(list)
        } else if list > 0.0 {
            MESA_CARD + Self::GAP + list
        } else {
            MESA_CARD
        };
        Self::HEADER + body
    }

    fn card_width(&self) -> f32 {
        if self.compact {
            self.width
        } else {
            (self.width * 0.42).clamp(260.0, 400.0)
        }
    }

    fn list(&self, listed: &Listed) -> Element {
        let mut expanded = self.expanded;
        let open = expanded();
        let row = Self::row_height(self.compact);
        let shown = if open {
            listed.rest.len()
        } else {
            listed.rest.len().min(Self::SHOWN)
        };
        let total = listed.rest.len() + usize::from(listed.current.is_some());
        let rows = listed.rest.iter().take(shown).map(|&workspace| {
            MesaRow {
                data: std::sync::Arc::clone(&self.data),
                workspace,
                height: row,
            }
            .into_element()
        });
        rect()
            .width(if self.compact {
                Size::fill()
            } else {
                Size::flex(1.0)
            })
            .max_width(Size::px(640.0))
            .children(rows)
            .maybe_child(
                (listed.current.is_some() && listed.rest.is_empty()).then(|| new_mesa_row(row)),
            )
            .maybe_child((listed.rest.len() > Self::SHOWN).then(|| {
                Button::new(if open {
                    "Ver menos".to_owned()
                } else {
                    format!("Ver todas ({total})")
                })
                .ghost()
                .small()
                .on_press(move |_| expanded.toggle())
            }))
            .into_element()
    }
}

impl Component for MesaStrip {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let listed = Listed::from(&*self.data);
        let card = self.card_width();
        let first = listed.current.map_or_else(
            || new_mesa_card(card),
            |workspace| {
                MesaCard {
                    data: std::sync::Arc::clone(&self.data),
                    workspace,
                    width: card,
                    current: true,
                }
                .into_element()
            },
        );
        let body = rect().width(Size::fill()).spacing(Self::GAP);
        let body = if self.compact {
            body
        } else {
            body.horizontal().content(Content::Flex)
        };
        rect()
            .width(Size::fill())
            .height(Size::px(Self::height(
                &self.data,
                self.compact,
                *self.expanded.read(),
            )))
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
            .child(body.child(first).child(self.list(&listed)))
    }
}
