//! Pane tab bar — horizontal strip of tabs with a trailing `+`
//! shortcut.
//!
//! Shell-agnostic: callers map their own tab shape (currently
//! [`crate::library::PaneTab`]) to [`TabBarEntry`] and route the
//! `on_select` / `on_close` / `on_new` callbacks back to whatever
//! owns the pane's layout state.
//!
//! Each cell is wrapped in a [`DragZone`] so tabs can be dragged
//! between panes. The whole bar doubles as a [`DropZone`] accepting
//! incoming [`TabDragPayload`]s — on drop, `on_tab_drop` fires with
//! the payload so the shell can mutate the layout.

use std::borrow::Cow;

use freya::prelude::*;

use super::drag_drop::{DragZone, DropZone};
use crate::library::PaneId;

/// A single tab row entry rendered by [`TabBar`].
#[derive(Clone)]
pub struct TabBarEntry {
    pub label: Cow<'static, str>,
    pub icon: Option<Cow<'static, str>>,
    pub is_active: bool,
}

impl std::fmt::Debug for TabBarEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabBarEntry")
            .field("label", &self.label)
            .field("icon", &self.icon)
            .field("is_active", &self.is_active)
            .finish()
    }
}

/// Payload carried while a tab is being dragged. Produced by the
/// source [`TabBar`] and consumed by whichever [`TabBar`] it is
/// dropped onto.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TabDragPayload {
    pub source_pane: PaneId,
    pub source_index: usize,
}

/// Tab strip anchored above a pane's content.
///
/// Equality intentionally returns `false` so pushing a mutated entry
/// list triggers a re-render — the body carries non-comparable
/// closures.
#[derive(Clone)]
pub struct TabBar {
    pane_id: PaneId,
    entries: Vec<TabBarEntry>,
    on_select: Callback<usize, ()>,
    on_close: Callback<usize, ()>,
    on_new: NoArgCallback<()>,
    on_tab_drop: Option<Callback<TabDragPayload, ()>>,
}

impl std::fmt::Debug for TabBar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabBar")
            .field("pane_id", &self.pane_id)
            .field("entries", &self.entries.len())
            .finish_non_exhaustive()
    }
}

impl PartialEq for TabBar {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl TabBar {
    #[must_use]
    pub fn new(
        pane_id: PaneId,
        entries: Vec<TabBarEntry>,
        on_select: impl Into<Callback<usize, ()>>,
        on_close: impl Into<Callback<usize, ()>>,
        on_new: impl Into<NoArgCallback<()>>,
    ) -> Self {
        Self {
            pane_id,
            entries,
            on_select: on_select.into(),
            on_close: on_close.into(),
            on_new: on_new.into(),
            on_tab_drop: None,
        }
    }

    /// Install a drop handler so tabs dragged from another pane can
    /// land on this bar. Without this the bar still renders but drops
    /// are swallowed.
    #[must_use]
    pub fn on_tab_drop(mut self, cb: impl Into<Callback<TabDragPayload, ()>>) -> Self {
        self.on_tab_drop = Some(cb.into());
        self
    }
}

impl Component for TabBar {
    fn render(&self) -> impl IntoElement {
        let bg = Color::from_rgb(18, 18, 22);
        let mut row = rect()
            .horizontal()
            .width(Size::fill())
            .height(Size::px(36.0))
            .background(bg)
            .padding((0.0, 4.0))
            .cross_align(Alignment::Center)
            .spacing(2.0);

        for (i, entry) in self.entries.iter().enumerate() {
            let cell = tab_cell(
                i,
                entry,
                self.on_select.clone(),
                self.on_close.clone(),
            );
            row = row.child(DragZone::new(
                TabDragPayload {
                    source_pane: self.pane_id,
                    source_index: i,
                },
                cell,
            ));
        }
        row = row.child(new_tab_button(self.on_new.clone()));

        let pane_id = self.pane_id;
        let on_tab_drop = self.on_tab_drop.clone();
        DropZone::new(
            row,
            EventHandler::new(move |payload: TabDragPayload| {
                if payload.source_pane == pane_id {
                    return;
                }
                if let Some(cb) = &on_tab_drop {
                    cb.call(payload);
                }
            }),
        )
    }
}

fn tab_cell(
    index: usize,
    entry: &TabBarEntry,
    on_select: Callback<usize, ()>,
    on_close: Callback<usize, ()>,
) -> Rect {
    let bg = if entry.is_active {
        Color::from_rgb(32, 32, 38)
    } else {
        Color::TRANSPARENT
    };
    let label_color = if entry.is_active {
        Color::from_rgb(240, 240, 245)
    } else {
        Color::from_rgb(180, 180, 190)
    };
    let label_text = entry.label.to_string();
    let close_cb = on_close.clone();
    let mut cell = rect()
        .horizontal()
        .height(Size::px(28.0))
        .cross_align(Alignment::Center)
        .padding((0.0, 10.0))
        .with_corner_radius(8.0)
        .background(bg)
        .spacing(8.0)
        .on_press(move |_| {
            on_select.call(index);
        });
    if let Some(icon) = &entry.icon {
        cell = cell.child(
            label()
                .color(label_color)
                .font_size(14.0)
                .text(icon.to_string()),
        );
    }
    cell = cell.child(
        label()
            .color(label_color)
            .font_size(13.0)
            .text(label_text),
    );
    cell = cell.child(close_glyph(index, close_cb));
    cell
}

fn close_glyph(index: usize, on_close: Callback<usize, ()>) -> Rect {
    rect()
        .width(Size::px(18.0))
        .height(Size::px(18.0))
        .center()
        .with_corner_radius(9.0)
        .on_press(move |e: Event<PressEventData>| {
            e.stop_propagation();
            on_close.call(index);
        })
        .child(
            label()
                .color(Color::from_rgb(170, 170, 180))
                .font_size(12.0)
                .text("×"),
        )
}

fn new_tab_button(on_new: NoArgCallback<()>) -> Rect {
    rect()
        .width(Size::px(28.0))
        .height(Size::px(28.0))
        .center()
        .with_corner_radius(8.0)
        .on_press(move |_| on_new.call())
        .child(
            label()
                .color(Color::from_rgb(190, 190, 200))
                .font_size(16.0)
                .text("+"),
        )
}
