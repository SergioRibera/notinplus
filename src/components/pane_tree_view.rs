//! Read-only renderer for a [`PaneLayout`] tree.
//!
//! Each leaf currently gets a placeholder card (pane id + tab count).
//! Phase 4.3 swaps the stub for a real pane view (tab bar + content).
//! The divider is drawn as a thin bar between splits; drag handling
//! lands in a follow-up so this commit stays read-only.

use freya::prelude::*;

use super::tab_bar::{TabBar, TabBarEntry};
use crate::library::{Pane, PaneId, PaneLayout, SplitOrientation};

/// Minimum fraction either child of a split is allowed to shrink to.
/// Mirrors [`PaneLayout::clamp_ratios`] so what the user sees matches
/// what the shell is willing to persist.
pub const MIN_RATIO: f32 = 0.05;

/// Thickness of the visual divider between two split children.
pub const DIVIDER_PX: f32 = 4.0;

/// Callback shape fired when the user interacts with a leaf's tab bar.
///
/// The shell plumbs these into [`crate::library::Library::set_workspace_layout`]
/// (or the tab-aware helpers that come with Phase 4.3+) to persist the
/// change back to disk.
#[derive(Clone)]
pub struct PaneTreeCallbacks {
    pub on_tab_select: Callback<(PaneId, usize), ()>,
    pub on_tab_close: Callback<(PaneId, usize), ()>,
    pub on_tab_new: Callback<PaneId, ()>,
}

impl std::fmt::Debug for PaneTreeCallbacks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaneTreeCallbacks").finish_non_exhaustive()
    }
}

/// Component rendering a [`PaneLayout`]. Each leaf gets a [`TabBar`]
/// and a stub content body (real content lands with Phase 4.3+).
///
/// Equality always returns `false`: the callbacks carried in
/// [`PaneTreeCallbacks`] are opaque closures, so pushing a new
/// instance must always re-render.
#[derive(Clone, Debug)]
pub struct PaneTreeView {
    layout: PaneLayout,
    callbacks: PaneTreeCallbacks,
    tab_labels: fn(&PaneTab) -> String,
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
    pub const fn new(
        layout: PaneLayout,
        callbacks: PaneTreeCallbacks,
        tab_labels: fn(&PaneTab) -> String,
    ) -> Self {
        Self {
            layout,
            callbacks,
            tab_labels,
        }
    }
}

impl Component for PaneTreeView {
    fn render(&self) -> impl IntoElement {
        render_node(&self.layout, &self.callbacks, self.tab_labels)
    }
}

fn render_node(
    node: &PaneLayout,
    callbacks: &PaneTreeCallbacks,
    tab_labels: fn(&PaneTab) -> String,
) -> Element {
    match node {
        PaneLayout::Leaf(pane) => leaf_view(pane, callbacks, tab_labels).into_element(),
        PaneLayout::Split {
            orientation,
            ratio,
            first,
            second,
        } => split_container(*orientation, *ratio, first, second, callbacks, tab_labels)
            .into_element(),
    }
}

fn split_container(
    orientation: SplitOrientation,
    raw_ratio: f32,
    first: &PaneLayout,
    second: &PaneLayout,
    callbacks: &PaneTreeCallbacks,
    tab_labels: fn(&PaneTab) -> String,
) -> Rect {
    let ratio = raw_ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
    let first_el = render_node(first, callbacks, tab_labels);
    let second_el = render_node(second, callbacks, tab_labels);
    let divider = divider_bar(orientation);
    let (container, first_wrap, second_wrap) = match orientation {
        SplitOrientation::Horizontal => (
            rect().horizontal(),
            rect().width(Size::flex(ratio)).height(Size::fill()),
            rect().width(Size::flex(1.0 - ratio)).height(Size::fill()),
        ),
        SplitOrientation::Vertical => (
            rect().vertical(),
            rect().width(Size::fill()).height(Size::flex(ratio)),
            rect().width(Size::fill()).height(Size::flex(1.0 - ratio)),
        ),
    };
    container
        .width(Size::fill())
        .height(Size::fill())
        .child(first_wrap.child(first_el))
        .child(divider)
        .child(second_wrap.child(second_el))
}

fn divider_bar(orientation: SplitOrientation) -> Rect {
    let bg = Color::from_rgb(45, 45, 52);
    match orientation {
        SplitOrientation::Horizontal => rect()
            .width(Size::px(DIVIDER_PX))
            .height(Size::fill())
            .background(bg),
        SplitOrientation::Vertical => rect()
            .width(Size::fill())
            .height(Size::px(DIVIDER_PX))
            .background(bg),
    }
}

fn leaf_view(
    pane: &Pane,
    callbacks: &PaneTreeCallbacks,
    tab_labels: fn(&PaneTab) -> String,
) -> Rect {
    let pane_id = pane.id;
    let entries: Vec<TabBarEntry> = pane
        .tabs
        .iter()
        .enumerate()
        .map(|(i, tab)| TabBarEntry {
            label: tab_labels(tab).into(),
            icon: None,
            is_active: i == pane.active_tab as usize,
        })
        .collect();
    let select = callbacks.on_tab_select.clone();
    let close = callbacks.on_tab_close.clone();
    let new = callbacks.on_tab_new.clone();
    let bar = TabBar::new(
        entries,
        Callback::new(move |i: usize| select.call((pane_id, i))),
        Callback::new(move |i: usize| close.call((pane_id, i))),
        NoArgCallback::new(move || new.call(pane_id)),
    );
    let active_label = pane
        .tabs
        .get(pane.active_tab as usize)
        .map_or_else(|| format!("Pane {} (vacío)", pane.id.0), tab_labels);
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

    fn stub_label(tab: &PaneTab) -> String {
        format!("Item {}", tab.item.0)
    }

    fn stub_callbacks() -> PaneTreeCallbacks {
        PaneTreeCallbacks {
            on_tab_select: Callback::new(|_| {}),
            on_tab_close: Callback::new(|_| {}),
            on_tab_new: Callback::new(|_| {}),
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
        let view = PaneTreeView::new(layout.clone(), stub_callbacks(), stub_label);
        assert_eq!(view.layout.leaves().len(), 3);
    }
}
