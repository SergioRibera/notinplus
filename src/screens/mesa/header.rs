//! The top of each pane: a tab strip, or the document's own header
//! when the pane shows one document.

use std::sync::Arc;

use freya::prelude::*;
use notein_i18n::tr;
use notein_plugin::services::HeaderPress;
use notein_ui_kit::TapExt;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{Icon, IconButton, Text, TextRole, tokens};

use super::Ops;
use crate::components::drag_drop::{DragZone, DropZone};
use crate::library::{ItemId, LibraryData, Pane, PaneId, PaneTab, TabTarget, ViewSlot};
use crate::references::kind_color;

/// A tab being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TabDrag {
    pub(super) pane: PaneId,
    pub(super) index: usize,
}

/// Title, icon and tint of a tab.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TabLook {
    pub(super) title: String,
    pub(super) icon: Bytes,
    pub(super) tint: Color,
    pub(super) scratch: bool,
}

impl TabLook {
    pub(super) fn of(tab: &PaneTab, data: &LibraryData) -> Self {
        use freya::icons::lucide;
        let p = palette();
        match tab.target {
            TabTarget::Item(id) => data.item(id).map_or_else(
                || Self {
                    title: tr!("mesa-tab-gone"),
                    icon: lucide::file_x(),
                    tint: p.t3,
                    scratch: false,
                },
                |item| {
                    let info = crate::plugins::kind_info(&item.kind.plugin_id);
                    Self {
                        title: item.name.clone(),
                        icon: info.as_ref().map_or_else(lucide::file, |k| k.icon.clone()),
                        tint: info.map_or(p.t3, |k| kind_color(&p, k.color)),
                        scratch: false,
                    }
                },
            ),
            TabTarget::Scratch(_) => Self {
                title: tr!("mesa-tab-draft"),
                icon: lucide::pen_line(),
                tint: p.kinds.scratch,
                scratch: true,
            },
            TabTarget::Annotation(_) => Self {
                title: tr!("mesa-tab-annotation"),
                icon: lucide::message_square_text(),
                tint: p.accent,
                scratch: false,
            },
        }
    }
}

fn menu_for(tab: &PaneTab, data: &Arc<LibraryData>, at: CursorPoint) {
    if let Some(item) = tab.item() {
        crate::actions::show(
            crate::actions::for_item(data, item, crate::actions::Origin::Header),
            at,
        );
    }
}

fn anchor(e: &Event<PressEventData>) -> CursorPoint {
    notein_ui_kit::press_point(e.data())
}

/// The header of pane `pane`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PaneHeader {
    pub(super) data: Arc<LibraryData>,
    pub(super) pane: Pane,
    pub(super) ops: Ops,
    pub(super) focused: bool,
    pub(super) can_close: bool,
    /// Phones: the pane the switcher steps to.
    pub(super) next: Option<PaneId>,
}

impl PaneHeader {
    /// Buttons the face of `item` adds to this header.
    fn face_actions(&self, item: ItemId) -> Vec<Element> {
        let Some(kind) = self.data.item(item).map(|i| i.kind.plugin_id.clone()) else {
            return Vec::new();
        };
        let Some(face) = crate::plugins::catalog().face(&kind) else {
            return Vec::new();
        };
        let slot = ViewSlot {
            workspace: self.ops.mesa,
            pane: self.pane.id,
        };
        face.header_actions()
            .into_iter()
            .map(|action| {
                let kind = kind.clone();
                let id = action.id.into_owned();
                IconButton::new(action.icon, action.label)
                    .size(tokens::size::BUTTON_SM)
                    .on_press(move |e: Event<PressEventData>| {
                        let at = anchor(&e);
                        #[allow(clippy::cast_possible_truncation)] // window coordinates
                        let at = (at.x as f32, at.y as f32);
                        crate::plugins::host::shell().press_header(
                            &kind,
                            &HeaderPress {
                                doc: crate::plugins::content::doc_id(item),
                                pane: Some(crate::plugins::faces::pane_key(slot)),
                                action: id.clone(),
                                at,
                            },
                        );
                    })
                    .into_element()
            })
            .collect()
    }
}

impl Component for PaneHeader {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let pane = self.pane.id;
        let ops = self.ops;
        let strip = if self.pane.tabs.len() == 1 {
            self.single_tab()
        } else {
            self.tab_strip()
        };
        let row = rect()
            .width(Size::fill())
            .height(Size::px(tokens::size::TAB_BAR))
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .background(if self.focused { p.s1 } else { p.bg })
            .border(
                Border::new()
                    .fill(p.line)
                    .width(1.0)
                    .alignment(BorderAlignment::Inner),
            )
            .child(strip)
            .child(self.trailing());
        DropZone::new(
            row,
            EventHandler::new(move |drag: TabDrag| {
                if drag.pane != pane {
                    ops.move_tab(drag.pane, drag.index, pane);
                }
            }),
        )
    }
}

impl PaneHeader {
    /// The face's buttons, then other pane, new tab, more and close.
    fn trailing(&self) -> Rect {
        use freya::icons::lucide;
        let pane = self.pane.id;
        let ops = self.ops;
        let active = self.pane.tabs.get(self.pane.active_tab as usize);
        let face_actions = active
            .and_then(PaneTab::item)
            .map(|item| self.face_actions(item))
            .unwrap_or_default();
        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(2.0)
            .maybe_child(
                active
                    .and_then(PaneTab::item)
                    .map(|item| rect().key(item).child(super::here::HereNow { item })),
            )
            .children(face_actions)
            .maybe_child(self.next.map(|next| {
                IconButton::new(lucide::columns_2(), tr!("mesa-pane-other"))
                    .size(tokens::size::BUTTON_SM)
                    .on_press(move |_| ops.focus(next))
            }))
            .child(
                IconButton::new(lucide::plus(), tr!("mesa-tab-new"))
                    .size(tokens::size::BUTTON_SM)
                    .on_press(move |_| ops.new_tab(pane)),
            )
            .maybe_child(active.filter(|t| t.item().is_some()).map(|tab| {
                let tab = tab.clone();
                let data = Arc::clone(&self.data);
                IconButton::new(lucide::ellipsis(), tr!("mesa-more-actions"))
                    .size(tokens::size::BUTTON_SM)
                    .on_press(move |e: Event<PressEventData>| menu_for(&tab, &data, anchor(&e)))
            }))
            .maybe_child(self.can_close.then(|| {
                IconButton::new(lucide::panel_right_close(), tr!("mesa-pane-close"))
                    .size(tokens::size::BUTTON_SM)
                    .on_press(move |_| ops.close_pane(pane))
            }))
    }

    /// One document: its colour, its title and close.
    fn single_tab(&self) -> Element {
        let (pane, ops) = (self.pane.id, self.ops);
        let tab = &self.pane.tabs[0];
        let look = TabLook::of(tab, &self.data);
        let close_tab = tab.clone();
        rect()
            .width(Size::flex(1.0))
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(8.0)
            .padding((0.0, 8.0))
            .child(
                rect()
                    .width(Size::px(8.0))
                    .height(Size::px(8.0))
                    .corner_radius(tokens::radius::FULL)
                    .background(look.tint),
            )
            .child(
                rect()
                    .width(Size::flex(1.0))
                    .child(Text::new(look.title).role(TextRole::Strong).lines(1)),
            )
            .child(
                IconButton::new(freya::icons::lucide::x(), tr!("common-close"))
                    .size(tokens::size::BUTTON_SM)
                    .on_press(move |_| ops.close_tab(pane, 0, &close_tab)),
            )
            .into_element()
    }

    /// Several documents: draggable tabs that scroll sideways.
    fn tab_strip(&self) -> Element {
        let (pane, ops) = (self.pane.id, self.ops);
        let active = self.pane.active_tab as usize;
        let cells: Vec<Element> = self
            .pane
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                DragZone::new(
                    TabDrag { pane, index },
                    TabCell {
                        look_title: TabLook::of(tab, &self.data).title,
                        tab: tab.clone(),
                        data: Arc::clone(&self.data),
                        pane,
                        index,
                        active: index == active,
                        ops,
                    },
                )
                .into_element()
            })
            .collect();
        ScrollView::new()
            .direction(Direction::Horizontal)
            .width(Size::flex(1.0))
            .height(Size::px(tokens::size::TAB_BAR))
            .show_scrollbar(false)
            .child(
                notein_ui_kit::ScrollContent::new(
                    rect()
                        .height(Size::fill())
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(2.0)
                        .padding((0.0, 4.0))
                        .children(cells),
                )
                .width(Size::auto()),
            )
            .into_element()
    }
}

#[derive(Clone, Debug, PartialEq)]
struct TabCell {
    look_title: String,
    tab: PaneTab,
    data: Arc<LibraryData>,
    pane: PaneId,
    index: usize,
    active: bool,
    ops: Ops,
}

impl Component for TabCell {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let mut over = use_state(|| false);
        let look = TabLook::of(&self.tab, &self.data);
        let (pane, index, ops) = (self.pane, self.index, self.ops);
        let close = self.tab.clone();
        let menu_tab = self.tab.clone();
        let data = Arc::clone(&self.data);
        let fill = match (self.active, look.scratch, *over.read()) {
            (_, _, true) => p.s3,
            (true, true, false) => p.kinds.scratch.with_a(60),
            (true, false, false) => p.s2,
            (false, true, false) => p.kinds.scratch.with_a(24),
            (false, false, false) => Color::TRANSPARENT,
        };
        let cell = rect()
            .height(Size::px(32.0))
            .max_width(Size::px(220.0))
            .padding((0.0, 6.0, 0.0, 10.0))
            .corner_radius(tokens::radius::SM)
            .background(fill)
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(6.0)
            .a11y_role(AccessibilityRole::Tab)
            .a11y_alt(self.look_title.clone())
            .on_tap(move |_| ops.select_tab(pane, index))
            .on_secondary_down(move |e: Event<PressEventData>| {
                menu_for(&menu_tab, &data, anchor(&e));
            })
            .child(Icon::new(look.icon).size(14.0).color(look.tint))
            .child(
                Text::new(look.title)
                    .role(if self.active {
                        TextRole::Strong
                    } else {
                        TextRole::Body
                    })
                    .color(if self.active { p.t1 } else { p.t2 })
                    .lines(1),
            )
            .child(
                rect()
                    .width(Size::px(20.0))
                    .height(Size::px(20.0))
                    .corner_radius(tokens::radius::XS)
                    .center()
                    .a11y_role(AccessibilityRole::Button)
                    .a11y_alt(tr!("mesa-tab-close"))
                    .on_tap(move |e: Event<PressEventData>| {
                        e.stop_propagation();
                        ops.close_tab(pane, index, &close);
                    })
                    .child(Icon::new(lucide::x()).size(12.0).color(p.t3)),
            )
            .maybe_child(self.active.then(|| {
                rect()
                    .position(Position::new_absolute().left(8.0).top(29.0))
                    .width(Size::px(24.0))
                    .height(Size::px(2.0))
                    .corner_radius(1.0)
                    .background(p.accent)
            }));
        DropZone::new(
            cell,
            EventHandler::new(move |drag: TabDrag| {
                if drag.pane == pane {
                    if drag.index != index {
                        ops.reorder(pane, drag.index, index);
                    }
                } else {
                    ops.move_tab(drag.pane, drag.index, pane);
                }
            }),
        )
        .on_drag_over(EventHandler::new(move |is_over: bool| over.set(is_over)))
    }
}
