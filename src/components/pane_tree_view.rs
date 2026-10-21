//! Renderer for a [`PaneLayout`] tree.
//!
//! Each leaf gets a [`TabBar`] atop a body produced by the shell-supplied
//! `tab_body` callback (so the pane can host any component — canvas,
//! PDF viewer, mind map, etc.). Internal split nodes render as
//! [`SplitNode`] — two children flanking a draggable divider. During a
//! drag the divider commits its new ratio to the shell via
//! [`PaneTreeCallbacks::on_split_resize`] on release only, so the
//! persistence worker is not thrashed with per-pointer-move writes.

use freya::prelude::*;

use super::drag_drop::{DropZone, use_drag};
use super::tab_bar::{SCRATCH_ACCENT, TabBar, TabBarEntry, TabDragPayload};
use crate::library::{Pane, PaneId, PaneLayout, SplitOrientation, SplitSide, TabTarget};

/// Which edge of a leaf a dragged tab was dropped on. Determines both
/// the split orientation and which child the newly-created pane takes
/// in the resulting split.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

    /// Which side of the new split the fresh pane carrying the dropped
    /// tab lives on.
    #[must_use]
    pub const fn new_side(self) -> SplitSide {
        match self {
            Self::Left | Self::Top => SplitSide::First,
            Self::Right | Self::Bottom => SplitSide::Second,
        }
    }
}

/// Minimum fraction either child of a split is allowed to shrink to.
/// Mirrors [`PaneLayout::clamp_ratios`] so what the user sees matches
/// what the shell is willing to persist.
pub const MIN_RATIO: f32 = 0.05;

/// Thickness of the visual divider between two split children.
pub const DIVIDER_PX: f32 = 4.0;

/// Interactive hit-area around the divider — larger than the painted
/// strip so touch / mouse pointers do not need pixel-perfect aim.
pub const DIVIDER_HIT_PX: f32 = 10.0;

/// Callback shape fired when the user interacts with the pane tree.
///
/// The shell plumbs these into the library async helpers to persist
/// the change back to disk.
#[derive(Clone)]
pub struct PaneTreeCallbacks {
    pub on_tab_select: Callback<(PaneId, usize), ()>,
    pub on_tab_close: Callback<(PaneId, usize), ()>,
    pub on_tab_new: Callback<PaneId, ()>,
    /// Divider released. First argument addresses the split node (empty
    /// = root split); second is the final ratio in `[MIN_RATIO,
    /// 1.0 - MIN_RATIO]`.
    pub on_split_resize: Callback<(Vec<SplitSide>, f32), ()>,
    /// Tab dropped onto another pane. `(src_pane, src_index, dst_pane)`.
    /// Same-pane drops are filtered out before this fires.
    pub on_tab_move: Callback<(PaneId, usize, PaneId), ()>,
    /// Tab dropped onto another tab of its own pane. `(pane, from, to)`.
    pub on_tab_reorder: Callback<(PaneId, usize, usize), ()>,
    /// Tab dropped on an *edge* of a leaf. The shell splits the target
    /// leaf along [`SplitEdge::orientation`] and moves the tab into
    /// the fresh pane sitting on [`SplitEdge::new_side`].
    pub on_tab_split: Callback<(PaneId, usize, PaneId, SplitEdge), ()>,
    /// User clicked the pane-close button in the tab bar. Fires only
    /// for leaves whose closure would still leave at least one pane in
    /// the workspace — the component suppresses the button on the last
    /// surviving leaf so the shell does not have to re-filter.
    pub on_pane_close: Callback<PaneId, ()>,
}

impl std::fmt::Debug for PaneTreeCallbacks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaneTreeCallbacks").finish_non_exhaustive()
    }
}

/// Component rendering a [`PaneLayout`]. Equality always returns
/// `false`: the callbacks carried in [`PaneTreeCallbacks`] are opaque
/// closures, so pushing a new instance must always re-render.
#[derive(Clone, Debug)]
pub struct PaneTreeView {
    layout: PaneLayout,
    callbacks: PaneTreeCallbacks,
    tab_labels: Callback<PaneTab, String>,
    tab_body: Callback<(PaneId, PaneTab), Element>,
    show_tabs: bool,
}

impl PartialEq for PaneTreeView {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

/// Re-export for the label hook so callers do not need to touch the
/// `library::PaneTab` import path.
pub use crate::library::PaneTab;

impl PaneTreeView {
    /// `tab_labels` renders a human-readable name per tab — the shell
    /// supplies it so this component stays agnostic of the library.
    /// `tab_body` renders the active tab's body [`Element`] (canvas,
    /// PDF viewer, mind map, etc.) and receives the pane id alongside
    /// the tab so shells that mount the same item under different
    /// roles per pane (e.g. doc-linked's PDF + canvas) can discriminate.
    /// Each pane's body is produced lazily per render, so switching
    /// tabs or splitting mounts a fresh component tree in that pane.
    #[must_use]
    pub fn new(
        layout: PaneLayout,
        callbacks: PaneTreeCallbacks,
        tab_labels: impl Into<Callback<PaneTab, String>>,
        tab_body: impl Into<Callback<(PaneId, PaneTab), Element>>,
    ) -> Self {
        Self {
            layout,
            callbacks,
            tab_labels: tab_labels.into(),
            tab_body: tab_body.into(),
            show_tabs: true,
        }
    }

    /// Hide every leaf's tab bar + its drag/edge-drop + pane-close
    /// chrome. Each leaf renders its active tab's body only. Intended
    /// for embedders that reuse the resize/split machinery but handle
    /// tab semantics externally (or have no concept of tabs at all —
    /// so the tree is a plain resizable layout).
    #[must_use]
    pub const fn show_tabs(mut self, show: bool) -> Self {
        self.show_tabs = show;
        self
    }
}

impl Component for PaneTreeView {
    fn render(&self) -> impl IntoElement {
        // One leaf = cannot close any pane (workspace must retain at
        // least one). Any split exposes the close button on every leaf.
        let can_close_pane = self.layout.leaves().len() > 1;
        render_node(
            &self.layout,
            Vec::new(),
            &self.callbacks,
            &self.tab_labels,
            &self.tab_body,
            can_close_pane,
            self.show_tabs,
        )
    }
}

fn render_node(
    node: &PaneLayout,
    path: Vec<SplitSide>,
    callbacks: &PaneTreeCallbacks,
    tab_labels: &Callback<PaneTab, String>,
    tab_body: &Callback<(PaneId, PaneTab), Element>,
    can_close_pane: bool,
    show_tabs: bool,
) -> Element {
    match node {
        PaneLayout::Leaf(pane) => LeafNode {
            pane: pane.clone(),
            callbacks: callbacks.clone(),
            tab_labels: tab_labels.clone(),
            tab_body: tab_body.clone(),
            can_close_pane,
            show_tabs,
        }
        .into_element(),
        PaneLayout::Split {
            orientation,
            ratio,
            first,
            second,
        } => SplitNode {
            path,
            orientation: *orientation,
            ratio: *ratio,
            first: (**first).clone(),
            second: (**second).clone(),
            callbacks: callbacks.clone(),
            tab_labels: tab_labels.clone(),
            tab_body: tab_body.clone(),
            can_close_pane,
            show_tabs,
        }
        .into_element(),
    }
}

/// Internal split node. Owns the area measurement + drag state so a
/// divider release can translate the final pointer position into a
/// ratio commit.
#[derive(Clone)]
struct SplitNode {
    path: Vec<SplitSide>,
    orientation: SplitOrientation,
    ratio: f32,
    first: PaneLayout,
    second: PaneLayout,
    callbacks: PaneTreeCallbacks,
    tab_labels: Callback<PaneTab, String>,
    tab_body: Callback<(PaneId, PaneTab), Element>,
    can_close_pane: bool,
    show_tabs: bool,
}

impl std::fmt::Debug for SplitNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SplitNode")
            .field("path", &self.path)
            .field("orientation", &self.orientation)
            .field("ratio", &self.ratio)
            .finish_non_exhaustive()
    }
}

impl PartialEq for SplitNode {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl Component for SplitNode {
    fn render(&self) -> impl IntoElement {
        let mut area = use_state(Area::default);
        // While `Some`, overrides the persisted ratio during a drag.
        // Cleared on release so the next render reads the fresh prop.
        let mut drag_ratio = use_state(|| None::<f32>);
        let persisted = self.ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
        let shown = drag_ratio.read().unwrap_or(persisted);

        let orientation = self.orientation;
        let path_on_commit = self.path.clone();
        let on_split_resize = self.callbacks.on_split_resize.clone();

        let compute_ratio = move |cursor: CursorPoint, area: Area| -> f32 {
            let a = area.to_f64();
            match orientation {
                SplitOrientation::Horizontal => {
                    let w = a.width();
                    if w <= 0.0 {
                        return 0.5;
                    }
                    ((cursor.x - a.min_x()) / w)
                        .clamp(f64::from(MIN_RATIO), 1.0 - f64::from(MIN_RATIO))
                        as f32
                }
                SplitOrientation::Vertical => {
                    let h = a.height();
                    if h <= 0.0 {
                        return 0.5;
                    }
                    ((cursor.y - a.min_y()) / h)
                        .clamp(f64::from(MIN_RATIO), 1.0 - f64::from(MIN_RATIO))
                        as f32
                }
            }
        };

        let on_divider_down = move |e: Event<PointerEventData>| {
            if !e.data().is_primary() {
                return;
            }
            let new_ratio = compute_ratio(e.global_location(), *area.read());
            drag_ratio.set(Some(new_ratio));
            e.stop_propagation();
            e.prevent_default();
        };

        let on_global_pointer_move = move |e: Event<PointerEventData>| {
            if drag_ratio.read().is_some() {
                let new_ratio = compute_ratio(e.global_location(), *area.read());
                drag_ratio.set(Some(new_ratio));
            }
        };

        let on_global_pointer_up = move |_: Event<PointerEventData>| {
            let final_ratio = *drag_ratio.read();
            if let Some(final_ratio) = final_ratio {
                on_split_resize.call((path_on_commit.clone(), final_ratio));
                drag_ratio.set(None);
            }
        };

        let first_el = render_node(
            &self.first,
            {
                let mut p = self.path.clone();
                p.push(SplitSide::First);
                p
            },
            &self.callbacks,
            &self.tab_labels,
            &self.tab_body,
            self.can_close_pane,
            self.show_tabs,
        );
        let second_el = render_node(
            &self.second,
            {
                let mut p = self.path.clone();
                p.push(SplitSide::Second);
                p
            },
            &self.callbacks,
            &self.tab_labels,
            &self.tab_body,
            self.can_close_pane,
            self.show_tabs,
        );

        let (container, first_wrap, second_wrap) = match orientation {
            SplitOrientation::Horizontal => (
                rect().horizontal(),
                rect().width(Size::flex(shown)).height(Size::fill()),
                rect().width(Size::flex(1.0 - shown)).height(Size::fill()),
            ),
            SplitOrientation::Vertical => (
                rect().vertical(),
                rect().width(Size::fill()).height(Size::flex(shown)),
                rect().width(Size::fill()).height(Size::flex(1.0 - shown)),
            ),
        };

        container
            .width(Size::fill())
            .height(Size::fill())
            .content(Content::Flex)
            .on_sized(move |e: Event<SizedEventData>| area.set_if_modified(e.area))
            .on_global_pointer_move(on_global_pointer_move)
            .on_global_pointer_up(on_global_pointer_up)
            .child(first_wrap.child(first_el))
            .child(divider_bar(orientation, on_divider_down))
            .child(second_wrap.child(second_el))
    }
}

fn divider_bar(
    orientation: SplitOrientation,
    on_pointer_down: impl FnMut(Event<PointerEventData>) + 'static,
) -> Rect {
    let bg = Color::from_rgb(45, 45, 52);
    // The hit area is a larger invisible track so touch / mouse
    // pointers do not need pixel-perfect aim; the visible strip sits
    // centered inside it.
    let visible = match orientation {
        SplitOrientation::Horizontal => rect()
            .width(Size::px(DIVIDER_PX))
            .height(Size::fill())
            .background(bg),
        SplitOrientation::Vertical => rect()
            .width(Size::fill())
            .height(Size::px(DIVIDER_PX))
            .background(bg),
    };
    let (hit_w, hit_h) = match orientation {
        SplitOrientation::Horizontal => (Size::px(DIVIDER_HIT_PX), Size::fill()),
        SplitOrientation::Vertical => (Size::fill(), Size::px(DIVIDER_HIT_PX)),
    };
    rect()
        .width(hit_w)
        .height(hit_h)
        .center()
        .on_pointer_down(on_pointer_down)
        .child(visible)
}

/// Pixel size of each edge drop zone — thick enough for touch but
/// still leaves most of the body as a plain tab-bar drop target.
const EDGE_DROP_PX: f32 = 56.0;

/// Internal leaf renderer. Owns the live drag peek so edge drop
/// overlays only appear while a tab is being dragged.
#[derive(Clone)]
struct LeafNode {
    pane: Pane,
    callbacks: PaneTreeCallbacks,
    tab_labels: Callback<PaneTab, String>,
    tab_body: Callback<(PaneId, PaneTab), Element>,
    can_close_pane: bool,
    show_tabs: bool,
}

impl std::fmt::Debug for LeafNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeafNode")
            .field("pane", &self.pane.id)
            .finish_non_exhaustive()
    }
}

impl PartialEq for LeafNode {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl Component for LeafNode {
    fn render(&self) -> impl IntoElement {
        // Hook calls run on every render regardless of `show_tabs` so
        // freya's hook ordering stays constant whether the embedder has
        // tabs enabled or not. The resulting state is only observed
        // when tabs are on.
        let drag = use_drag::<TabDragPayload>();
        let hover_top = use_state(|| false);
        let hover_bottom = use_state(|| false);
        let hover_left = use_state(|| false);
        let hover_right = use_state(|| false);

        let pane_id = self.pane.id;
        let active_tab = self.pane.tabs.get(self.pane.active_tab as usize).cloned();
        let active_is_scratch = active_tab
            .as_ref()
            .is_some_and(|tab| matches!(tab.target, TabTarget::Scratch(_)));
        let body_inner = match active_tab {
            Some(tab) => self.tab_body.call((self.pane.id, tab)),
            None => leaf_body_empty(self.pane.id).into_element(),
        };

        if !self.show_tabs {
            // Tabs hidden: no tab bar, no drag/edge-drop overlay. A
            // scratch canvas still shows its tint as a thin strip so
            // the user knows nothing here is being saved.
            return rect()
                .vertical()
                .width(Size::percent(100.))
                .height(Size::percent(100.))
                .content(Content::Flex)
                .background(Color::from_rgb(22, 22, 28))
                .maybe_child(active_is_scratch.then(|| {
                    rect()
                        .width(Size::percent(100.))
                        .height(Size::px(3.0))
                        .background(SCRATCH_ACCENT)
                }))
                .child(
                    rect()
                        .width(Size::percent(100.))
                        .height(Size::flex(1.0))
                        .content(Content::Flex)
                        .child(body_inner),
                );
        }

        let drag_active = drag.read().is_some();
        let entries: Vec<TabBarEntry> = self
            .pane
            .tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| TabBarEntry {
                label: self.tab_labels.call(tab.clone()).into(),
                icon: None,
                is_active: i == self.pane.active_tab as usize,
                accent: matches!(tab.target, TabTarget::Scratch(_)).then_some(SCRATCH_ACCENT),
            })
            .collect();
        let select = self.callbacks.on_tab_select.clone();
        let close = self.callbacks.on_tab_close.clone();
        let new = self.callbacks.on_tab_new.clone();
        let move_cb = self.callbacks.on_tab_move.clone();
        let reorder_cb = self.callbacks.on_tab_reorder.clone();
        let close_pane = self.callbacks.on_pane_close.clone();
        let mut bar = TabBar::new(
            pane_id,
            entries,
            Callback::new(move |i: usize| select.call((pane_id, i))),
            Callback::new(move |i: usize| close.call((pane_id, i))),
            NoArgCallback::new(move || new.call(pane_id)),
        )
        .on_tab_drop(Callback::new(move |payload: TabDragPayload| {
            move_cb.call((payload.source_pane, payload.source_index, pane_id));
        }))
        .on_tab_reorder(Callback::new(move |(from, to): (usize, usize)| {
            reorder_cb.call((pane_id, from, to));
        }));
        if self.can_close_pane {
            bar = bar.on_pane_close(NoArgCallback::new(move || close_pane.call(pane_id)));
        }

        let mut body = rect()
            .width(Size::percent(100.))
            .height(Size::flex(1.0))
            .content(Content::Flex)
            .child(body_inner);

        if drag_active {
            let split_cb = self.callbacks.on_tab_split.clone();
            body = body.child(edge_drop_overlay(
                pane_id,
                split_cb,
                hover_top,
                hover_bottom,
                hover_left,
                hover_right,
            ));
        }

        rect()
            .vertical()
            .width(Size::percent(100.))
            .height(Size::percent(100.))
            .content(Content::Flex)
            .background(Color::from_rgb(22, 22, 28))
            .child(bar)
            .child(body)
    }
}

/// Shown when the active pane has no tabs — explains the tab bar's
/// `+` button is the way out.
fn leaf_body_empty(id: PaneId) -> Rect {
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .center()
        .child(
            label()
                .color(Color::from_rgb(150, 150, 160))
                .font_size(13.0)
                .text(format!(
                    "Pane {} vacío — usá + para abrir un item.",
                    &id.to_string()[..8]
                )),
        )
}

/// Single-rect overlay that lays its four edge drop zones out via flex
/// bands rather than four absolute siblings — mirrors the pattern
/// `freya-components::docking` uses. One absolute Overlay-layer rect
/// fills the pane body, split vertically into Top / Middle / Bottom
/// bands; the middle row carries Left / spacer / Right.
///
/// Hover state for each edge is lifted to the caller so freya's hook
/// rules (same hook count every render) hold — the overlay only mounts
/// while a drag is active.
fn edge_drop_overlay(
    dst_pane: PaneId,
    on_split: Callback<(PaneId, usize, PaneId, SplitEdge), ()>,
    hover_top: State<bool>,
    hover_bottom: State<bool>,
    hover_left: State<bool>,
    hover_right: State<bool>,
) -> Rect {
    let middle = rect()
        .width(Size::fill())
        .height(Size::flex(1.0))
        .horizontal()
        .content(Content::Flex)
        .child(edge_drop_band(
            dst_pane,
            SplitEdge::Left,
            Size::px(EDGE_DROP_PX),
            Size::fill(),
            on_split.clone(),
            hover_left,
        ))
        // Transparent pass-through spacer so clicks on the pane body
        // between the edges still reach the content underneath.
        .child(
            rect()
                .width(Size::flex(1.0))
                .height(Size::fill())
                .interactive(false),
        )
        .child(edge_drop_band(
            dst_pane,
            SplitEdge::Right,
            Size::px(EDGE_DROP_PX),
            Size::fill(),
            on_split.clone(),
            hover_right,
        ));

    rect()
        .position(Position::new_absolute().top(0.0).left(0.0))
        .layer(Layer::Overlay)
        .width(Size::percent(100.0))
        .height(Size::percent(100.0))
        .vertical()
        .content(Content::Flex)
        .child(edge_drop_band(
            dst_pane,
            SplitEdge::Top,
            Size::fill(),
            Size::px(EDGE_DROP_PX),
            on_split.clone(),
            hover_top,
        ))
        .child(middle)
        .child(edge_drop_band(
            dst_pane,
            SplitEdge::Bottom,
            Size::fill(),
            Size::px(EDGE_DROP_PX),
            on_split,
            hover_bottom,
        ))
}

/// One band of [`edge_drop_overlay`]: a sized DropZone with its own hover
/// highlight. Returned as `Rect` so it slots into either flex direction.
fn edge_drop_band(
    dst_pane: PaneId,
    edge: SplitEdge,
    width: Size,
    height: Size,
    on_split: Callback<(PaneId, usize, PaneId, SplitEdge), ()>,
    hovered: State<bool>,
) -> Rect {
    let highlight = if *hovered.read() {
        Color::from_argb(170, 110, 160, 240)
    } else {
        Color::from_argb(110, 80, 120, 190)
    };
    let on_drag_over = {
        let mut hovered = hovered;
        EventHandler::new(move |over: bool| hovered.set(over))
    };
    let zone_body = rect()
        .width(Size::fill())
        .height(Size::fill())
        .background(highlight);

    let drop = DropZone::new(
        zone_body,
        EventHandler::new(move |payload: TabDragPayload| {
            on_split.call((payload.source_pane, payload.source_index, dst_pane, edge));
        }),
    )
    .on_drag_over(on_drag_over)
    .width(Size::fill())
    .height(Size::fill());

    rect().width(width).height(height).child(drop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{ItemId, Pane, PaneId};

    fn stub_label_cb() -> Callback<PaneTab, String> {
        Callback::new(|tab: PaneTab| format!("Item {:?}", tab.item()))
    }

    fn stub_callbacks() -> PaneTreeCallbacks {
        PaneTreeCallbacks {
            on_tab_select: Callback::new(|_| {}),
            on_tab_close: Callback::new(|_| {}),
            on_tab_new: Callback::new(|_| {}),
            on_split_resize: Callback::new(|_| {}),
            on_tab_move: Callback::new(|_| {}),
            on_tab_reorder: Callback::new(|_| {}),
            on_tab_split: Callback::new(|_| {}),
            on_pane_close: Callback::new(|_| {}),
        }
    }

    fn stub_body_cb() -> Callback<(PaneId, PaneTab), Element> {
        Callback::new(|_: (PaneId, PaneTab)| rect().into_element())
    }

    #[test]
    fn split_edge_encodes_orientation_and_side() {
        assert_eq!(SplitEdge::Right.orientation(), SplitOrientation::Horizontal);
        assert_eq!(SplitEdge::Right.new_side(), SplitSide::Second);
        assert_eq!(SplitEdge::Left.new_side(), SplitSide::First);
        assert_eq!(SplitEdge::Bottom.orientation(), SplitOrientation::Vertical);
        assert_eq!(SplitEdge::Top.new_side(), SplitSide::First);
    }

    #[test]
    fn leaf_count_matches_layout() {
        let pane_a = PaneId::new_v4();
        let pane_b = PaneId::new_v4();
        let pane_c = PaneId::new_v4();
        let layout = PaneLayout::Split {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.5,
            first: Box::new(PaneLayout::Leaf(Pane {
                id: pane_a,
                tabs: vec![PaneTab::new(ItemId::new_v4())],
                active_tab: 0,
            })),
            second: Box::new(PaneLayout::Split {
                orientation: SplitOrientation::Vertical,
                ratio: 0.5,
                first: Box::new(PaneLayout::leaf(pane_b)),
                second: Box::new(PaneLayout::Leaf(Pane {
                    id: pane_c,
                    tabs: Vec::new(),
                    active_tab: 0,
                })),
            }),
        };
        let view = PaneTreeView::new(
            layout.clone(),
            stub_callbacks(),
            stub_label_cb(),
            stub_body_cb(),
        );
        assert_eq!(view.layout.leaves().len(), 3);
    }
}
