//! The top of each pane: a tab strip, or the document's own header
//! when the pane shows one document.

use std::sync::Arc;

use freya::prelude::*;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{Icon, IconButton, Text, TextRole, tokens};

use super::Ops;
use crate::components::drag_drop::{DragZone, DropZone};
use crate::library::{LibraryData, Pane, PaneId, PaneTab, TabTarget};
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
            TabTarget::Item(id) => match data.item(id) {
                Some(item) => {
                    let info = crate::plugins::kind_info(&item.kind.plugin_id);
                    Self {
                        title: item.name.clone(),
                        icon: info.as_ref().map_or_else(lucide::file, |k| k.icon.clone()),
                        tint: info.map_or(p.t3, |k| kind_color(&p, k.color)),
                        scratch: false,
                    }
                }
                None => Self {
                    title: "Ya no está".to_owned(),
                    icon: lucide::file_x(),
                    tint: p.t3,
                    scratch: false,
                },
            },
            TabTarget::Scratch(_) => Self {
                title: "Borrador".to_owned(),
                icon: lucide::pen_line(),
                tint: p.kinds.scratch,
                scratch: true,
            },
            TabTarget::Annotation(id) => Self {
                title: crate::annotation::tab_title(id),
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
    match e.data() {
        PressEventData::Mouse(m) => m.global_location,
        _ => CursorPoint::new(0.0, 0.0),
    }
}

/// The header of pane `pane`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PaneHeader {
    pub(super) data: Arc<LibraryData>,
    pub(super) pane: Pane,
    pub(super) ops: Ops,
    pub(super) focused: bool,
    pub(super) can_close: bool,
}

impl Component for PaneHeader {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let pane = self.pane.id;
        let ops = self.ops;
        let single = self.pane.tabs.len() == 1;
        let active = self.pane.active_tab as usize;

        let trailing = rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(2.0)
            .child(
                IconButton::new(lucide::plus(), "Nueva pestaña")
                    .size(tokens::size::BUTTON_SM)
                    .on_press(move |_| ops.new_tab(pane)),
            )
            .maybe_child(
                self.pane
                    .tabs
                    .get(active)
                    .filter(|t| t.item().is_some())
                    .map(|tab| {
                        let tab = tab.clone();
                        let data = Arc::clone(&self.data);
                        IconButton::new(lucide::ellipsis(), "Más acciones")
                            .size(tokens::size::BUTTON_SM)
                            .on_press(move |e: Event<PressEventData>| {
                                menu_for(&tab, &data, anchor(&e))
                            })
                    }),
            )
            .maybe_child(self.can_close.then(|| {
                IconButton::new(lucide::panel_right_close(), "Cerrar el panel")
                    .size(tokens::size::BUTTON_SM)
                    .on_press(move |_| ops.close_pane(pane))
            }));

        let strip: Element = if single {
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
                    IconButton::new(lucide::x(), "Cerrar")
                        .size(tokens::size::BUTTON_SM)
                        .on_press(move |_| ops.close_tab(pane, 0, &close_tab)),
                )
                .into_element()
        } else {
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
                    rect()
                        .height(Size::fill())
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(2.0)
                        .padding((0.0, 4.0))
                        .children(cells),
                )
                .into_element()
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
            .child(trailing);
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
            .on_press(move |_| ops.select_tab(pane, index))
            .on_secondary_down(move |e: Event<PressEventData>| {
                menu_for(&menu_tab, &data, anchor(&e))
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
                    .a11y_alt("Cerrar pestaña")
                    .on_press(move |e: Event<PressEventData>| {
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
