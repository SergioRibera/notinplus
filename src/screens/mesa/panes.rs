//! The panes of a mesa: splits with a draggable divider, and leaves
//! that show their active document under its header.

use std::sync::Arc;

use freya::prelude::*;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{Button, EmptyState, tokens};

use super::Ops;
use super::header::{PaneHeader, TabDrag};
use super::notices::TabNotice;
use crate::components::drag_drop::{DropZone, use_drag};
use crate::library::{
    LibraryData, Pane, PaneId, PaneLayout, PaneTab, SplitOrientation, SplitSide, TabTarget,
    ViewSlot, Workspace,
};

/// Smallest share either side of a split keeps.
const MIN_RATIO: f32 = 0.15;
/// Width of the divider's hit area.
const DIVIDER_HIT: f32 = 12.0;
/// Depth of the edge bands that split a pane when a tab is dropped.
const EDGE_DROP: f32 = 64.0;

/// Every pane of a mesa.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Panes {
    pub(super) data: Arc<LibraryData>,
    pub(super) mesa: Workspace,
    pub(super) ops: Ops,
}

impl Component for Panes {
    fn render(&self) -> impl IntoElement {
        let focused = super::Focus::of(&self.mesa).map(|f| f.pane);
        let leaves = self.mesa.layout.leaves();
        let can_close = leaves.len() > 1;
        // Phones show one pane at a time; its header steps to the next.
        let next = focused.and_then(|f| {
            let at = leaves.iter().position(|p| p.id == f)?;
            (leaves.len() > 1).then(|| leaves[(at + 1) % leaves.len()].id)
        });
        let ctx = Ctx {
            data: Arc::clone(&self.data),
            ops: self.ops,
            focused,
            can_close,
            next: None,
        };
        if notein_ui_kit::width_class().is_compact()
            && let Some(pane) = focused.and_then(|f| leaves.iter().find(|p| p.id == f))
        {
            return Leaf {
                pane: (*pane).clone(),
                ctx: Ctx { next, ..ctx },
            }
            .into_element();
        }
        node(&self.mesa.layout, Vec::new(), &ctx)
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Ctx {
    data: Arc<LibraryData>,
    ops: Ops,
    focused: Option<PaneId>,
    can_close: bool,
    /// On phones, the pane the header's switcher goes to.
    next: Option<PaneId>,
}

fn node(layout: &PaneLayout, path: Vec<SplitSide>, ctx: &Ctx) -> Element {
    match layout {
        PaneLayout::Leaf(pane) => rect()
            .key(pane.id.0)
            .width(Size::fill())
            .height(Size::fill())
            .child(Leaf {
                pane: pane.clone(),
                ctx: ctx.clone(),
            })
            .into_element(),
        PaneLayout::Split {
            orientation,
            ratio,
            first,
            second,
        } => Split {
            path,
            orientation: *orientation,
            ratio: *ratio,
            first: (**first).clone(),
            second: (**second).clone(),
            ctx: ctx.clone(),
        }
        .into_element(),
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Split {
    path: Vec<SplitSide>,
    orientation: SplitOrientation,
    ratio: f32,
    first: PaneLayout,
    second: PaneLayout,
    ctx: Ctx,
}

impl Split {
    fn ratio_at(orientation: SplitOrientation, cursor: CursorPoint, area: Area) -> f32 {
        let a = area.to_f64();
        let (pos, start, len) = match orientation {
            SplitOrientation::Horizontal => (cursor.x, a.min_x(), a.width()),
            SplitOrientation::Vertical => (cursor.y, a.min_y(), a.height()),
        };
        if len <= 0.0 {
            return 0.5;
        }
        #[allow(clippy::cast_possible_truncation)] // a ratio in 0..1
        let ratio = ((pos - start) / len) as f32;
        ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO)
    }
}

impl Component for Split {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut area = use_state(Area::default);
        let mut dragging = use_state(|| None::<f32>);
        let mut hovered = use_state(|| false);
        let shown = dragging
            .read()
            .unwrap_or_else(|| self.ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO));
        let orientation = self.orientation;
        let ops = self.ctx.ops;
        let path = self.path.clone();
        let child_path = |side: SplitSide| {
            let mut path = self.path.clone();
            path.push(side);
            path
        };
        let (container, first, second, divider) = match orientation {
            SplitOrientation::Horizontal => (
                rect().horizontal(),
                rect().width(Size::flex(shown)).height(Size::fill()),
                rect().width(Size::flex(1.0 - shown)).height(Size::fill()),
                rect().width(Size::px(DIVIDER_HIT)).height(Size::fill()),
            ),
            SplitOrientation::Vertical => (
                rect().vertical(),
                rect().width(Size::fill()).height(Size::flex(shown)),
                rect().width(Size::fill()).height(Size::flex(1.0 - shown)),
                rect().width(Size::fill()).height(Size::px(DIVIDER_HIT)),
            ),
        };
        let active = dragging.read().is_some() || *hovered.read();
        let grip =
            match orientation {
                SplitOrientation::Horizontal => rect()
                    .width(Size::px(if active { 3.0 } else { 1.0 }))
                    .height(Size::fill()),
                SplitOrientation::Vertical => rect()
                    .width(Size::fill())
                    .height(Size::px(if active { 3.0 } else { 1.0 })),
            }
            .background(if active { p.accent } else { p.line });
        container
            .width(Size::fill())
            .height(Size::fill())
            .content(Content::Flex)
            .on_sized(move |e: Event<SizedEventData>| area.set_if_modified(e.area))
            .on_global_pointer_move(move |e: Event<PointerEventData>| {
                if dragging.read().is_some() {
                    dragging.set(Some(Self::ratio_at(
                        orientation,
                        e.global_location(),
                        *area.read(),
                    )));
                }
            })
            .on_global_pointer_up(move |_: Event<PointerEventData>| {
                let done = *dragging.read();
                if let Some(ratio) = done {
                    dragging.set(None);
                    ops.resize(path.clone(), ratio);
                }
            })
            .child(first.child(node(&self.first, child_path(SplitSide::First), &self.ctx)))
            .child(
                divider
                    .center()
                    .a11y_role(AccessibilityRole::Splitter)
                    .on_pointer_enter(move |e: Event<PointerEventData>| {
                        hovered.set(notein_ui_kit::is_mouse(&e));
                    })
                    .on_pointer_leave(move |_| hovered.set(false))
                    .on_pointer_down(move |e: Event<PointerEventData>| {
                        if e.data().is_primary() {
                            e.stop_propagation();
                            dragging.set(Some(Self::ratio_at(
                                orientation,
                                e.global_location(),
                                *area.read(),
                            )));
                        }
                    })
                    .child(grip),
            )
            .child(second.child(node(&self.second, child_path(SplitSide::Second), &self.ctx)))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Leaf {
    pane: Pane,
    ctx: Ctx,
}

/// The document of `tab`, mounted fresh per tab.
fn body(tab: &PaneTab, data: &LibraryData, slot: ViewSlot) -> Element {
    use crate::app::{CanvasMount, CanvasPane};
    let inner = match tab.target {
        TabTarget::Item(id) => match data.item(id) {
            Some(item) if item.trashed.is_none() => {
                crate::plugins::faces::render(id, &item.kind, Some(slot))
            }
            _ => rect().expanded().into_element(),
        },
        TabTarget::Scratch(id) => CanvasPane {
            mount: CanvasMount::Ephemeral(id),
        }
        .into_element(),
        TabTarget::Annotation(id) => crate::annotation::ExpandedAnnotation { id }.into_element(),
    };
    rect()
        .key(tab.target)
        .width(Size::fill())
        .height(Size::fill())
        .content(Content::Flex)
        .child(inner)
        .into_element()
}

impl Component for Leaf {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let p = palette();
        let drag = use_drag::<TabDrag>();
        let pane = self.pane.id;
        let ops = self.ctx.ops;
        let focused = self.ctx.focused == Some(pane);
        let index = self.pane.active_tab as usize;
        let active = self.pane.tabs.get(index).cloned();
        let slot = ViewSlot {
            workspace: ops.mesa,
            pane,
        };
        let content = active.as_ref().map_or_else(
            || {
                EmptyState::new(lucide::layout_panel_left(), "Panel vacío")
                    .body("Abrí un documento acá o un borrador para escribir rápido.")
                    .action(
                        rect()
                            .horizontal()
                            .spacing(8.0)
                            .child(
                                Button::new("Abrir documento")
                                    .primary()
                                    .on_press(move |_| ops.new_tab(pane)),
                            )
                            .child(
                                Button::new("Borrador")
                                    .tonal()
                                    .on_press(move |_| ops.new_scratch(pane)),
                            ),
                    )
                    .into_element()
            },
            |tab| body(tab, &self.ctx.data, slot),
        );
        let dragging = drag.read().is_some();
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .padding(2.0)
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::fill())
                    .content(Content::Flex)
                    .corner_radius(tokens::radius::LG)
                    .overflow(Overflow::Clip)
                    .background(p.s1)
                    .border(
                        Border::new()
                            .fill(if focused && self.ctx.can_close {
                                p.accent
                            } else {
                                p.line
                            })
                            .width(if focused && self.ctx.can_close {
                                1.5
                            } else {
                                1.0
                            })
                            .alignment(BorderAlignment::Inner),
                    )
                    .on_pointer_down(move |_| {
                        if !focused {
                            ops.focus(pane);
                        }
                    })
                    .child(PaneHeader {
                        data: Arc::clone(&self.ctx.data),
                        pane: self.pane.clone(),
                        ops,
                        focused,
                        can_close: self.ctx.can_close,
                        next: self.ctx.next,
                    })
                    .maybe_child(active.map(|tab| TabNotice {
                        data: Arc::clone(&self.ctx.data),
                        pane,
                        index,
                        tab,
                        ops,
                    }))
                    .child(
                        rect()
                            .width(Size::fill())
                            .height(Size::flex(1.0))
                            .content(Content::Flex)
                            .child(content)
                            .maybe_child(dragging.then(|| EdgeDrops { pane, ops }.into_element())),
                    ),
            )
    }
}

/// Bands on the four edges of a pane that split it when a tab lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EdgeDrops {
    pane: PaneId,
    ops: Ops,
}

impl Component for EdgeDrops {
    fn render(&self) -> impl IntoElement {
        let band = |edge: SplitEdge, width: Size, height: Size| EdgeBand {
            pane: self.pane,
            ops: self.ops,
            edge,
            width,
            height,
        };
        rect()
            .position(Position::new_absolute().top(0.0).left(0.0))
            .layer(Layer::Overlay)
            .width(Size::fill())
            .height(Size::fill())
            .content(Content::Flex)
            .child(band(SplitEdge::Top, Size::fill(), Size::px(EDGE_DROP)))
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.0))
                    .horizontal()
                    .content(Content::Flex)
                    .child(band(SplitEdge::Left, Size::px(EDGE_DROP), Size::fill()))
                    .child(
                        rect()
                            .width(Size::flex(1.0))
                            .height(Size::fill())
                            .interactive(false),
                    )
                    .child(band(SplitEdge::Right, Size::px(EDGE_DROP), Size::fill())),
            )
            .child(band(SplitEdge::Bottom, Size::fill(), Size::px(EDGE_DROP)))
    }
}

#[derive(Clone, Debug, PartialEq)]
struct EdgeBand {
    pane: PaneId,
    ops: Ops,
    edge: SplitEdge,
    width: Size,
    height: Size,
}

impl Component for EdgeBand {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut over = use_state(|| false);
        let (pane, ops, edge) = (self.pane, self.ops, self.edge);
        rect()
            .width(self.width.clone())
            .height(self.height.clone())
            .child(
                DropZone::new(
                    rect()
                        .width(Size::fill())
                        .height(Size::fill())
                        .background(if *over.read() {
                            p.accent.with_a(110)
                        } else {
                            p.accent.with_a(36)
                        }),
                    EventHandler::new(move |drag: TabDrag| {
                        ops.split_with_tab(drag.pane, drag.index, pane, edge);
                    }),
                )
                .on_drag_over(EventHandler::new(move |is_over: bool| over.set(is_over)))
                .width(Size::fill())
                .height(Size::fill()),
            )
    }
}

/// Which edge of a pane a dragged tab was dropped on: the side of the
/// split and where the new pane goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitEdge {
    Left,
    Right,
    Top,
    Bottom,
}

impl SplitEdge {
    #[must_use]
    pub const fn orientation(self) -> SplitOrientation {
        match self {
            Self::Left | Self::Right => SplitOrientation::Horizontal,
            Self::Top | Self::Bottom => SplitOrientation::Vertical,
        }
    }

    /// The side of the new split the pane with the dropped tab takes.
    #[must_use]
    pub const fn new_side(self) -> SplitSide {
        match self {
            Self::Left | Self::Top => SplitSide::First,
            Self::Right | Self::Bottom => SplitSide::Second,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SplitEdge;
    use crate::library::{SplitOrientation, SplitSide};

    #[test]
    fn split_edge_encodes_orientation_and_side() {
        assert_eq!(SplitEdge::Right.orientation(), SplitOrientation::Horizontal);
        assert_eq!(SplitEdge::Right.new_side(), SplitSide::Second);
        assert_eq!(SplitEdge::Left.new_side(), SplitSide::First);
        assert_eq!(SplitEdge::Bottom.orientation(), SplitOrientation::Vertical);
        assert_eq!(SplitEdge::Top.new_side(), SplitSide::First);
    }
}
