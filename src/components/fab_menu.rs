//! Floating action-button popup menu.
//!
//! Sits above the `+` FAB and lists creation shortcuts (new canvas,
//! new folder, import). Rides the shared [`ModalController`] with a
//! transparent backdrop so tapping outside dismisses the menu without
//! blocking the underlying UI visually.

use std::borrow::Cow;

use freya::prelude::*;

use super::modal::{Modal, ModalController};

/// A single row inside a [`FabMenu`].
#[derive(Clone)]
pub struct FabMenuEntry {
    icon: Cow<'static, str>,
    text: Cow<'static, str>,
    on_press: NoArgCallback<()>,
    divider_above: bool,
}

impl std::fmt::Debug for FabMenuEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FabMenuEntry")
            .field("icon", &self.icon)
            .field("text", &self.text)
            .field("divider_above", &self.divider_above)
            .finish_non_exhaustive()
    }
}

impl FabMenuEntry {
    #[must_use]
    pub fn new(
        icon: impl Into<Cow<'static, str>>,
        text: impl Into<Cow<'static, str>>,
        on_press: impl Into<NoArgCallback<()>>,
    ) -> Self {
        Self {
            icon: icon.into(),
            text: text.into(),
            on_press: on_press.into(),
            divider_above: false,
        }
    }

    /// Insert a hairline separator above this entry when the menu is
    /// rendered. Use it to group related actions (creation vs import).
    #[must_use]
    pub const fn with_divider_above(mut self) -> Self {
        self.divider_above = true;
        self
    }
}

/// Popup anchored to the bottom-right of the shell, above the `+` FAB.
///
/// Consumers push it through the modal controller with
/// [`FabMenu::open`] — internally it wraps itself in a
/// transparent-backdrop [`Modal::manual`], so tapping anywhere outside
/// the card closes the menu.
#[derive(Clone, Debug)]
pub struct FabMenu {
    entries: Vec<FabMenuEntry>,
    anchor_right: f32,
    anchor_bottom: f32,
    width: f32,
}

impl PartialEq for FabMenu {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}

impl Default for FabMenu {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            anchor_right: 24.0,
            anchor_bottom: 24.0,
            width: 260.0,
        }
    }
}

impl FabMenu {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a row. Chainable.
    #[must_use]
    pub fn entry(mut self, entry: FabMenuEntry) -> Self {
        self.entries.push(entry);
        self
    }

    /// Distance from the shell's right/bottom edges. Include the
    /// safe-area inset the FAB itself uses so the menu tracks the same
    /// corner across notch / gesture-handle devices.
    #[must_use]
    pub const fn anchor(mut self, right: f32, bottom: f32) -> Self {
        self.anchor_right = right;
        self.anchor_bottom = bottom;
        self
    }

    /// Card width in pixels. Defaults to `260`.
    #[must_use]
    pub const fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Open the menu through the shared [`ModalController`].
    /// Transparent backdrop, dismiss-on-outside-tap.
    pub fn open(self) {
        ModalController::get().open(
            Modal::new(self)
                .manual()
                .backdrop_alpha(0)
                .dismiss_on_backdrop(true),
        );
    }
}

impl Component for FabMenu {
    fn render(&self) -> impl IntoElement {
        let mut card = rect()
            .vertical()
            .width(Size::px(self.width))
            .background(Color::from_rgb(28, 28, 32))
            .with_corner_radius(14.0)
            .padding((6.0, 6.0))
            .position(
                Position::new_absolute()
                    .right(self.anchor_right)
                    .bottom(self.anchor_bottom),
            );

        for entry in &self.entries {
            if entry.divider_above {
                card = card.child(
                    rect()
                        .width(Size::fill())
                        .height(Size::px(1.0))
                        .background(Color::from_rgb(55, 55, 62)),
                );
            }
            card = card.child(row(entry));
        }

        card
    }
}

fn row(entry: &FabMenuEntry) -> impl IntoElement {
    let cb = entry.on_press.clone();
    rect()
        .horizontal()
        .width(Size::fill())
        .cross_align(Alignment::Center)
        .padding((10.0, 12.0))
        .spacing(14.0)
        .on_press(move |e: Event<PressEventData>| {
            e.stop_propagation();
            let mut controller = ModalController::get();
            controller.close();
            cb.call();
        })
        .child(
            label()
                .color(Color::from_rgb(210, 210, 220))
                .font_size(16.0)
                .text(entry.icon.to_string()),
        )
        .child(
            label()
                .color(Color::from_rgb(230, 230, 240))
                .font_size(14.0)
                .text(entry.text.to_string()),
        )
}
