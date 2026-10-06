//! Read-only renderer for a [`PaneLayout`] tree.
//!
//! Each leaf currently gets a placeholder card (pane id + tab count).
//! Phase 4.3 swaps the stub for a real pane view (tab bar + content).
//! The divider is drawn as a thin bar between splits; drag handling
//! lands in a follow-up so this commit stays read-only.

use freya::prelude::*;

use crate::library::{PaneLayout, SplitOrientation};

/// Minimum fraction either child of a split is allowed to shrink to.
/// Mirrors [`PaneLayout::clamp_ratios`] so what the user sees matches
/// what the shell is willing to persist.
pub const MIN_RATIO: f32 = 0.05;

/// Thickness of the visual divider between two split children.
pub const DIVIDER_PX: f32 = 4.0;

/// Component rendering a [`PaneLayout`] with stub leaves.
#[derive(Clone, PartialEq, Debug)]
pub struct PaneTreeView {
    layout: PaneLayout,
}

impl PaneTreeView {
    #[must_use]
    pub const fn new(layout: PaneLayout) -> Self {
        Self { layout }
    }
}

impl Component for PaneTreeView {
    fn render(&self) -> impl IntoElement {
        render_node(&self.layout)
    }
}

fn render_node(node: &PaneLayout) -> Element {
    match node {
        PaneLayout::Leaf(pane) => leaf_card(&format!(
            "Pane {} · {} tab{}",
            pane.id.0,
            pane.tabs.len(),
            if pane.tabs.len() == 1 { "" } else { "s" },
        ))
        .into_element(),
        PaneLayout::Split {
            orientation,
            ratio,
            first,
            second,
        } => split_container(*orientation, *ratio, first, second).into_element(),
    }
}

fn split_container(
    orientation: SplitOrientation,
    raw_ratio: f32,
    first: &PaneLayout,
    second: &PaneLayout,
) -> Rect {
    let ratio = raw_ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
    let first_el = render_node(first);
    let second_el = render_node(second);
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

fn leaf_card(text: &str) -> Rect {
    rect()
        .width(Size::fill())
        .height(Size::fill())
        .background(Color::from_rgb(22, 22, 28))
        .padding((16.0, 16.0))
        .child(
            label()
                .color(Color::from_rgb(190, 190, 200))
                .font_size(13.0)
                .text(text.to_owned()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Pane, PaneId};

    #[test]
    fn tree_view_eq_is_structural() {
        let a = PaneTreeView::new(PaneLayout::leaf(PaneId(1)));
        let b = PaneTreeView::new(PaneLayout::leaf(PaneId(1)));
        assert_eq!(a, b);
        let c = PaneTreeView::new(PaneLayout::leaf(PaneId(2)));
        assert_ne!(a, c);
    }

    #[test]
    fn leaf_count_matches_layout() {
        let layout = PaneLayout::Split {
            orientation: SplitOrientation::Horizontal,
            ratio: 0.5,
            first: Box::new(PaneLayout::leaf(PaneId(1))),
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
        let view = PaneTreeView::new(layout.clone());
        assert_eq!(view.layout.leaves().len(), 3);
    }
}
