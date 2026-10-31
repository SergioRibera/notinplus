//! The label of an icon-only control, shown after the mouse rests on
//! it.

use std::borrow::Cow;
use std::time::Duration;

use freya::prelude::*;

use crate::theme::palette;
use crate::tokens::{radius, text};

/// How long the pointer rests before the hint shows.
const DELAY: Duration = Duration::from_millis(450);
/// Room the hint may take on each side of the control's centre.
const REACH: f32 = 140.0;
/// Gap between the control and the hint.
const GAP: f32 = 6.0;

/// Shows and hides a [`Hint`] for one control.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HintState {
    shown: State<bool>,
    task: State<Option<TaskHandle>>,
    area: State<Area>,
}

impl std::fmt::Debug for HintState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HintState")
            .field("shown", &*self.shown.peek())
            .finish_non_exhaustive()
    }
}

/// Hint state for the calling control.
#[cfg_attr(feature = "sismo", sismo::function("kit::use_hint"))]
pub fn use_hint() -> HintState {
    HintState {
        shown: use_state(|| false),
        task: use_state(|| None),
        area: use_state(Area::default),
    }
}

impl HintState {
    /// The pointer came in. Touch never shows a hint: there is no
    /// resting finger, and a press already says what it does.
    #[cfg_attr(feature = "sismo", sismo::function("kit::hint_enter"))]
    pub fn enter(mut self, e: &Event<PointerEventData>) {
        if !matches!(e.data(), PointerEventData::Mouse(_)) {
            return;
        }
        self.cancel();
        let mut shown = self.shown;
        self.task.set(Some(spawn(traced!("task::kit::hint_delay", async move {
            timer(DELAY).await;
            shown.set_if_modified(true);
        }))));
    }

    /// The pointer left or pressed.
    #[cfg_attr(feature = "sismo", sismo::function("kit::hint_leave"))]
    pub fn leave(mut self) {
        self.cancel();
        self.shown.set_if_modified(false);
    }

    fn cancel(&mut self) {
        if let Some(task) = self.task.write().take() {
            task.cancel();
        }
    }

    /// Keep track of where the control is, to keep the hint on screen.
    pub fn track(mut self, e: &Event<SizedEventData>) {
        self.area.set_if_modified(e.area);
    }

    /// The hint for a control `side` px tall, when it is showing.
    #[must_use]
    pub fn render(self, label: Cow<'static, str>, side: f32) -> Option<Hint> {
        (*self.shown.read()).then(|| Hint {
            label,
            side,
            area: *self.area.peek(),
        })
    }
}

/// A small dark label under (or over) a control.
#[derive(Clone, Debug, PartialEq)]
pub struct Hint {
    label: Cow<'static, str>,
    side: f32,
    area: Area,
}

impl Component for Hint {
    #[cfg_attr(feature = "sismo", sismo::function("kit::hint"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let frame = crate::viewport::safe_frame();
        let area = self.area;
        // Centred under the control, pushed inwards near the sides of what
        // the bars and the keyboard leave visible, and above it near the
        // bottom.
        let (left, align) = if area.center().x + REACH > frame.max_x() {
            (2.0f32.mul_add(-REACH, area.width()), Alignment::End)
        } else if area.center().x - REACH < frame.min_x() {
            (0.0, Alignment::Start)
        } else {
            (area.width() / 2.0 - REACH, Alignment::Center)
        };
        let below = area.max_y() + GAP + 32.0 < frame.max_y();
        let position = if below {
            Position::new_absolute().top(self.side + GAP).left(left)
        } else {
            Position::new_absolute().bottom(self.side + GAP).left(left)
        };
        rect()
            .position(position)
            .layer(Layer::OverlayLevel(crate::tokens::layer::HINT))
            .interactive(Interactive::No)
            .width(Size::px(2.0 * REACH))
            .horizontal()
            .main_align(align)
            .child(
                rect()
                    .padding((5.0, 9.0))
                    .corner_radius(radius::SM)
                    .background(p.inv)
                    .shadow(Shadow::new().y(2.0).blur(8.0).color(p.shadow))
                    .child(
                        label()
                            .text(self.label.clone())
                            .font_size(text::LABEL)
                            .font_weight(FontWeight::MEDIUM)
                            .color(p.inv_text)
                            .max_lines(1),
                    ),
            )
    }
}
