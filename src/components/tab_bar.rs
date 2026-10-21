//! Pane tab bar — horizontal strip of tabs with a trailing `+`
//! shortcut.
//!
//! Shell-agnostic: callers map their own tab shape (currently
//! [`crate::library::PaneTab`]) to [`TabBarEntry`] and route the
//! `on_select` / `on_close` / `on_new` callbacks back to whatever
//! owns the pane's layout state.
//!
//! Each cell is wrapped in a [`DragZone`] so tabs can be dragged
//! between panes, and is itself a [`DropZone`]: a tab of the same
//! pane released over it asks for a reorder (`on_tab_reorder`), one
//! from another pane for a move (`on_tab_drop`). The whole bar is a
//! [`DropZone`] too, so a drop past the last tab still moves the tab
//! in.

use std::borrow::Cow;

use freya::prelude::*;

use super::drag_drop::{DragZone, DropZone};
use crate::library::PaneId;

/// A single tab row entry rendered by [`TabBar`].
#[derive(Clone)]
pub struct TabBarEntry {
    pub label: Cow<'static, str>,
    pub icon: Option<Bytes>,
    pub is_active: bool,
    /// Background tint marking a special tab (a scratch canvas).
    pub accent: Option<Color>,
}

impl std::fmt::Debug for TabBarEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabBarEntry")
            .field("label", &self.label)
            .field("icon_present", &self.icon.is_some())
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
    on_tab_reorder: Option<Callback<(usize, usize), ()>>,
    on_pane_close: Option<NoArgCallback<()>>,
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
            on_tab_reorder: None,
            on_pane_close: None,
        }
    }

    /// Install a reorder handler, fired as `(from, to)` when a tab of
    /// this bar is released over another of its tabs. Without it,
    /// such drops do nothing.
    #[must_use]
    pub fn on_tab_reorder(mut self, cb: impl Into<Callback<(usize, usize), ()>>) -> Self {
        self.on_tab_reorder = Some(cb.into());
        self
    }

    /// Install a drop handler so tabs dragged from another pane can
    /// land on this bar. Without this the bar still renders but drops
    /// are swallowed.
    #[must_use]
    pub fn on_tab_drop(mut self, cb: impl Into<Callback<TabDragPayload, ()>>) -> Self {
        self.on_tab_drop = Some(cb.into());
        self
    }

    /// Install a pane-close handler. When present the bar renders a
    /// trailing `×` button after the `+` so the pane can be closed in
    /// place. Callers skip this on the last surviving leaf — the
    /// library rejects closing it anyway.
    #[must_use]
    pub fn on_pane_close(mut self, cb: impl Into<NoArgCallback<()>>) -> Self {
        self.on_pane_close = Some(cb.into());
        self
    }
}

impl Component for TabBar {
    fn render(&self) -> impl IntoElement {
        let bg = Color::from_rgb(18, 18, 22);
        let mut row = rect()
            .horizontal()
            .width(Size::fill())
            .content(Content::Flex)
            .height(Size::px(36.0))
            .background(bg)
            .padding((0.0, 4.0))
            .cross_align(Alignment::Center)
            .spacing(2.0);

        let on_cell_drop = {
            let pane_id = self.pane_id;
            let reorder = self.on_tab_reorder.clone();
            let move_in = self.on_tab_drop.clone();
            Callback::new(move |(payload, index): (TabDragPayload, usize)| {
                if payload.source_pane == pane_id {
                    if let Some(cb) = &reorder {
                        cb.call((payload.source_index, index));
                    }
                } else if let Some(cb) = &move_in {
                    cb.call(payload);
                }
            })
        };
        for (i, entry) in self.entries.iter().enumerate() {
            let on_drop = {
                let on_cell_drop = on_cell_drop.clone();
                Callback::new(move |payload: TabDragPayload| on_cell_drop.call((payload, i)))
            };
            let cell = TabCell {
                index: i,
                entry: entry.clone(),
                on_select: self.on_select.clone(),
                on_close: self.on_close.clone(),
                on_drop,
            };
            row = row.child(DragZone::new(
                TabDragPayload {
                    source_pane: self.pane_id,
                    source_index: i,
                },
                cell,
            ));
        }
        row = row
            .child(new_tab_button(self.on_new.clone()))
            .child(rect().width(Size::flex(1.)));
        if let Some(close_pane) = self.on_pane_close.clone() {
            row = row.child(close_pane_button(close_pane));
        }

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

/// One tab of the strip. A component of its own so the drop highlight
/// can hold state without hooks running inside the bar's loop.
#[derive(Clone)]
struct TabCell {
    index: usize,
    entry: TabBarEntry,
    on_select: Callback<usize, ()>,
    on_close: Callback<usize, ()>,
    /// A dragged tab released over this cell.
    on_drop: Callback<TabDragPayload, ()>,
}

impl std::fmt::Debug for TabCell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TabCell")
            .field("index", &self.index)
            .field("entry", &self.entry)
            .finish_non_exhaustive()
    }
}

impl PartialEq for TabCell {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl Component for TabCell {
    fn render(&self) -> impl IntoElement {
        let hovered = use_state(|| false);
        let entry = &self.entry;
        let index = self.index;
        let bg = match (entry.accent, entry.is_active, *hovered.read()) {
            (_, _, true) => Color::from_rgb(64, 64, 78),
            (Some(accent), true, false) => accent,
            (Some(accent), false, false) => {
                Color::from_argb(90, accent.r(), accent.g(), accent.b())
            }
            (None, true, false) => Color::from_rgb(32, 32, 38),
            (None, false, false) => Color::TRANSPARENT,
        };
        let label_color = if entry.is_active {
            Color::from_rgb(240, 240, 245)
        } else {
            Color::from_rgb(180, 180, 190)
        };
        let label_text = entry.label.to_string();
        let on_select = self.on_select.clone();
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
        if let Some(icon) = entry.icon.clone() {
            cell = cell.child(
                SvgViewer::new(icon)
                    .color(label_color)
                    .width(Size::px(14.0))
                    .height(Size::px(14.0)),
            );
        }
        cell = cell.child(label().color(label_color).font_size(13.0).text(label_text));
        cell = cell.child(close_glyph(index, self.on_close.clone()));

        let on_drop = self.on_drop.clone();
        let on_drag_over = {
            let mut hovered = hovered;
            EventHandler::new(move |over: bool| hovered.set(over))
        };
        DropZone::new(
            cell,
            EventHandler::new(move |payload: TabDragPayload| on_drop.call(payload)),
        )
        .on_drag_over(on_drag_over)
    }
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
            SvgViewer::new(freya::icons::lucide::x())
                .color(Color::from_rgb(170, 170, 180))
                .width(Size::px(12.0))
                .height(Size::px(12.0)),
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
            SvgViewer::new(freya::icons::lucide::plus())
                .color(Color::from_rgb(190, 190, 200))
                .width(Size::px(14.0))
                .height(Size::px(14.0)),
        )
}

fn close_pane_button(on_close: NoArgCallback<()>) -> Rect {
    rect()
        .width(Size::px(28.0))
        .height(Size::px(28.0))
        .center()
        .with_corner_radius(8.0)
        .on_press(move |e: Event<PressEventData>| {
            e.stop_propagation();
            on_close.call();
        })
        .child(
            SvgViewer::new(freya::icons::lucide::x())
                .color(Color::from_rgb(190, 190, 200))
                .width(Size::px(14.0))
                .height(Size::px(14.0)),
        )
}

/// Tint of a scratch-canvas tab, so the private, unsaved surface is
/// told apart at a glance.
pub const SCRATCH_ACCENT: Color = Color::from_rgb(112, 66, 190);
