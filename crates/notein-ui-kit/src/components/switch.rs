//! On/off toggle.

use freya::animation::{
    AnimColor, AnimNum, Ease, Function, OnChange, OnCreation, ReadAnimatedValue,
    use_animation_with_dependencies,
};
use freya::prelude::*;

use crate::theme::palette;
use crate::tokens::radius;

const TRACK_W: f32 = 40.0;
const TRACK_H: f32 = 24.0;
const PAD: f32 = 3.0;
const THUMB: f32 = 18.0;
/// How far the thumb grows while pressed, along the track.
const STRETCH: f32 = 4.0;

/// A switch that applies instantly. The parent owns the value.
#[derive(Clone, PartialEq)]
pub struct Switch {
    on: bool,
    enabled: bool,
    on_toggle: Option<EventHandler<bool>>,
}

impl std::fmt::Debug for Switch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Switch")
            .field("on", &self.on)
            .finish_non_exhaustive()
    }
}

impl Switch {
    #[must_use]
    pub const fn new(on: bool) -> Self {
        Self {
            on,
            enabled: true,
            on_toggle: None,
        }
    }

    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Receives the new value.
    #[must_use]
    pub fn on_toggle(mut self, handler: impl Into<EventHandler<bool>>) -> Self {
        self.on_toggle = Some(handler.into());
        self
    }
}

impl Component for Switch {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let a11y_id = use_a11y();
        let focus = use_focus(a11y_id);
        let mut pressed = use_state(|| false);
        let on = self.on;
        let handler = self.on_toggle.clone();
        let (off_track, on_track) = (p.s4, p.accent);
        let (off_thumb, on_thumb) = (p.t2, Color::WHITE);
        // Slides between the ends; the first frame already sits at `on`.
        let motion = use_animation_with_dependencies(&on, move |conf, on| {
            conf.on_creation(OnCreation::Finish);
            conf.on_change(OnChange::Rerun);
            let (from, to) = if *on { (0.0, 1.0) } else { (1.0, 0.0) };
            let colors = |off: Color, on_color: Color| {
                if *on {
                    AnimColor::new(off, on_color)
                } else {
                    AnimColor::new(on_color, off)
                }
                .time(180)
                .ease(Ease::Out)
            };
            (
                AnimNum::new(from, to)
                    .time(220)
                    .ease(Ease::Out)
                    .function(Function::Back),
                colors(off_track, on_track),
                colors(off_thumb, on_thumb),
            )
        });
        let (at, track, thumb) = if crate::theme::reduce_motion() {
            (
                if on { 1.0 } else { 0.0 },
                if on { on_track } else { off_track },
                if on { on_thumb } else { off_thumb },
            )
        } else {
            motion.get().value()
        };
        let stretch = if pressed() { STRETCH } else { 0.0 };
        let travel = 2.0f32.mul_add(-PAD, TRACK_W) - THUMB - stretch;
        rect()
            .a11y_id(a11y_id)
            .a11y_focusable(self.enabled)
            .a11y_role(AccessibilityRole::Switch)
            .a11y_alt(if on { "Activado" } else { "Desactivado" })
            .width(Size::px(TRACK_W))
            .height(Size::px(TRACK_H))
            .corner_radius(radius::FULL)
            .background(track)
            .opacity(if self.enabled { 1.0 } else { 0.4 })
            .maybe(focus() == Focus::Keyboard, |r| {
                r.border(
                    Border::new()
                        .fill(p.focus)
                        .width(p.focus_width)
                        .alignment(BorderAlignment::Outer),
                )
            })
            .maybe(self.enabled, |r| {
                r.on_pointer_down(move |_| pressed.set(true))
                    .on_pointer_leave(move |_| pressed.set_if_modified(false))
                    .map(handler, |r, handler| {
                        r.on_press(move |e: Event<PressEventData>| {
                            // The row around it toggles too; one toggle is enough.
                            e.stop_propagation();
                            pressed.set(false);
                            handler.call(!on);
                        })
                    })
            })
            .child(
                rect()
                    .position(
                        Position::new_absolute()
                            .top(PAD)
                            .left(at.clamp(0.0, 1.1).mul_add(travel, PAD)),
                    )
                    .width(Size::px(THUMB + stretch))
                    .height(Size::px(THUMB))
                    .corner_radius(radius::FULL)
                    .background(thumb)
                    .shadow(
                        Shadow::new()
                            .y(1.0)
                            .blur(3.0)
                            .color(Color::from_argb(60, 0, 0, 0)),
                    ),
            )
    }
}
