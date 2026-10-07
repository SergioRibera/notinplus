//! Renderer for a [`PaneLayout`] tree.
//!
//! Each leaf gets a [`TabBar`] atop a stub body (real content lands
//! with Phase 4.3+). Internal split nodes render as [`SplitNode`] —
//! two children flanking a draggable divider. During a drag the
//! divider commits its new ratio to the shell via
//! [`PaneTreeCallbacks::on_split_resize`] on release only, so the
//! persistence worker is not thrashed with per-pointer-move writes.

use freya::prelude::*;

use super::tab_bar::{TabBar, TabBarEntry, TabDragPayload};
use crate::library::{Pane, PaneId, PaneLayout, SplitOrientation, SplitSide};

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
    #[must_use]
    pub fn new(
        layout: PaneLayout,
        callbacks: PaneTreeCallbacks,
        tab_labels: impl Into<Callback<PaneTab, String>>,
    ) -> Self {
        Self {
            layout,
            callbacks,
            tab_labels: tab_labels.into(),
        }
    }
}

impl Component for PaneTreeView {
    fn render(&self) -> impl IntoElement {
        render_node(
            &self.layout,
            Vec::new(),
            &self.callbacks,
            &self.tab_labels,
        )
    }
}

fn render_node(
    node: &PaneLayout,
    path: Vec<SplitSide>,
    callbacks: &PaneTreeCallbacks,
    tab_labels: &Callback<PaneTab, String>,
) -> Element {
    match node {
        PaneLayout::Leaf(pane) => leaf_view(pane, callbacks, tab_labels).into_element(),
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
                    ((cursor.x - a.min_x()) / w).clamp(f64::from(MIN_RATIO), 1.0 - f64::from(MIN_RATIO)) as f32
                }
                SplitOrientation::Vertical => {
                    let h = a.height();
                    if h <= 0.0 {
                        return 0.5;
                    }
                    ((cursor.y - a.min_y()) / h).clamp(f64::from(MIN_RATIO), 1.0 - f64::from(MIN_RATIO)) as f32
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

fn leaf_view(
    pane: &Pane,
    callbacks: &PaneTreeCallbacks,
    tab_labels: &Callback<PaneTab, String>,
) -> Rect {
    let pane_id = pane.id;
    let entries: Vec<TabBarEntry> = pane
        .tabs
        .iter()
        .enumerate()
        .map(|(i, tab)| TabBarEntry {
            label: tab_labels.call(*tab).into(),
            icon: None,
            is_active: i == pane.active_tab as usize,
        })
        .collect();
    let select = callbacks.on_tab_select.clone();
    let close = callbacks.on_tab_close.clone();
    let new = callbacks.on_tab_new.clone();
    let move_cb = callbacks.on_tab_move.clone();
    let bar = TabBar::new(
        pane_id,
        entries,
        Callback::new(move |i: usize| select.call((pane_id, i))),
        Callback::new(move |i: usize| close.call((pane_id, i))),
        NoArgCallback::new(move || new.call(pane_id)),
    )
    .on_tab_drop(Callback::new(move |payload: TabDragPayload| {
        move_cb.call((payload.source_pane, payload.source_index, pane_id));
    }));
    let active_label = pane
        .tabs
        .get(pane.active_tab as usize)
        .map_or_else(|| format!("Pane {} (vacío)", pane.id.0), |t| tab_labels.call(*t));
    rect()
        .vertical()
        .width(Size::fill())
        .height(Size::fill())
        .background(Color::from_rgb(22, 22, 28))
        .child(bar)
        .child(leaf_body(&active_label))
}

fn leaf_body(text: &str) -> Rect {
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .padding((16.0, 16.0))
        .child(
            label()
                .color(Color::from_rgb(170, 170, 180))
                .font_size(14.0)
                .text(text.to_owned()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{ItemId, Pane, PaneId};

    fn stub_label_cb() -> Callback<PaneTab, String> {
        Callback::new(|tab: PaneTab| format!("Item {}", tab.item.0))
    }

    fn stub_callbacks() -> PaneTreeCallbacks {
        PaneTreeCallbacks {
            on_tab_select: Callback::new(|_| {}),
            on_tab_close: Callback::new(|_| {}),
            on_tab_new: Callback::new(|_| {}),
            on_split_resize: Callback::new(|_| {}),
            on_tab_move: Callback::new(|_| {}),
        }
    }

    #[test]
    fn leaf_count_matches_layout() {
        let layout = PaneLayout::Split {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.5,
            first: Box::new(PaneLayout::Leaf(Pane {
                id: PaneId(1),
                tabs: vec![PaneTab { item: ItemId(10) }],
                active_tab: 0,
            })),
            second: Box::new(PaneLayout::Split {
                orientation: SplitOrientation::Vertical,
                ratio: 0.5,
                first: Box::new(PaneLayout::leaf(PaneId(2))),
                second: Box::new(PaneLayout::Leaf(Pane {
                    id: PaneId(3),
                    tabs: Vec::new(),
                    active_tab: 0,
                })),
            }),
        };
        let view = PaneTreeView::new(layout.clone(), stub_callbacks(), stub_label_cb());
        assert_eq!(view.layout.leaves().len(), 3);
    }
}
