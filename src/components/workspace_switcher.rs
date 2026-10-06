//! Workspace switcher dropdown.
//!
//! Opens as a `Modal::manual` anchored top-left of the shell, lists
//! every workspace in the user's library, and surfaces an action row
//! that mints a brand-new one. Switching + creating are mediated by
//! callbacks so this component stays library-handle-agnostic.

use std::borrow::Cow;

use freya::prelude::*;

use super::modal::{Modal, ModalController};
use crate::library::WorkspaceId;

/// One row in the switcher's list.
#[derive(Clone)]
pub struct WorkspaceSwitcherEntry {
    pub id: WorkspaceId,
    pub label: Cow<'static, str>,
    pub swatch: Option<Color>,
    pub is_active: bool,
}

impl std::fmt::Debug for WorkspaceSwitcherEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceSwitcherEntry")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("is_active", &self.is_active)
            .finish_non_exhaustive()
    }
}

/// Dropdown card rendered under the top-left workspace pill.
#[derive(Clone)]
pub struct WorkspaceSwitcher {
    entries: Vec<WorkspaceSwitcherEntry>,
    on_pick: Callback<WorkspaceId, ()>,
    on_create: NoArgCallback<()>,
    anchor_left: f32,
    anchor_top: f32,
    width: f32,
}

impl std::fmt::Debug for WorkspaceSwitcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceSwitcher")
            .field("entries", &self.entries.len())
            .field("anchor_left", &self.anchor_left)
            .field("anchor_top", &self.anchor_top)
            .field("width", &self.width)
            .finish_non_exhaustive()
    }
}

impl PartialEq for WorkspaceSwitcher {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl WorkspaceSwitcher {
    #[must_use]
    pub fn new(
        entries: Vec<WorkspaceSwitcherEntry>,
        on_pick: impl Into<Callback<WorkspaceId, ()>>,
        on_create: impl Into<NoArgCallback<()>>,
    ) -> Self {
        Self {
            entries,
            on_pick: on_pick.into(),
            on_create: on_create.into(),
            anchor_left: 12.0,
            anchor_top: 56.0,
            width: 260.0,
        }
    }

    #[must_use]
    pub const fn anchor(mut self, left: f32, top: f32) -> Self {
        self.anchor_left = left;
        self.anchor_top = top;
        self
    }

    #[must_use]
    pub const fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Push the switcher through the shared [`ModalController`] with
    /// a transparent backdrop so tapping outside dismisses it without
    /// darkening the home grid.
    pub fn open(self) {
        ModalController::get().open(
            Modal::new(self)
                .manual()
                .backdrop_alpha(0)
                .dismiss_on_backdrop(true),
        );
    }
}

impl Component for WorkspaceSwitcher {
    fn render(&self) -> impl IntoElement {
        let mut card = rect()
            .vertical()
            .width(Size::px(self.width))
            .background(Color::from_rgb(28, 28, 32))
            .with_corner_radius(14.0)
            .padding((6.0, 6.0))
            .position(
                Position::new_absolute()
                    .left(self.anchor_left)
                    .top(self.anchor_top),
            );

        for entry in &self.entries {
            card = card.child(entry_row(entry, self.on_pick.clone()));
        }

        card = card.child(
            rect()
                .width(Size::fill())
                .height(Size::px(1.0))
                .background(Color::from_rgb(55, 55, 62)),
        );
        card = card.child(create_row(self.on_create.clone()));
        card
    }
}

fn entry_row(entry: &WorkspaceSwitcherEntry, on_pick: Callback<WorkspaceId, ()>) -> Rect {
    let active_bg = if entry.is_active {
        Color::from_rgb(42, 44, 54)
    } else {
        Color::TRANSPARENT
    };
    let id = entry.id;
    let swatch_color = entry.swatch.unwrap_or(Color::from_rgb(80, 80, 90));
    let label_text = entry.label.to_string();
    rect()
        .horizontal()
        .width(Size::fill())
        .cross_align(Alignment::Center)
        .padding((10.0, 12.0))
        .spacing(12.0)
        .with_corner_radius(10.0)
        .background(active_bg)
        .on_press(move |_| {
            let mut controller = ModalController::get();
            controller.close();
            on_pick.call(id);
        })
        .child(
            rect()
                .width(Size::px(10.0))
                .height(Size::px(10.0))
                .with_corner_radius(5.0)
                .background(swatch_color),
        )
        .child(
            label()
                .color(Color::from_rgb(230, 230, 240))
                .font_size(14.0)
                .text(label_text),
        )
}

fn create_row(on_create: NoArgCallback<()>) -> Rect {
    rect()
        .horizontal()
        .width(Size::fill())
        .cross_align(Alignment::Center)
        .padding((10.0, 12.0))
        .spacing(12.0)
        .with_corner_radius(10.0)
        .on_press(move |_| {
            let mut controller = ModalController::get();
            controller.close();
            on_create.call();
        })
        .child(
            label()
                .color(Color::from_rgb(210, 210, 220))
                .font_size(16.0)
                .text("+"),
        )
        .child(
            label()
                .color(Color::from_rgb(230, 230, 240))
                .font_size(14.0)
                .text("Nuevo workspace"),
        )
}
