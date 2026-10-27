//! Square icon-only button.

use std::borrow::Cow;

use freya::prelude::*;

use super::hint::use_hint;
use super::icon::Icon;
use super::scroll_content::TapExt;
use crate::theme::palette;
use crate::tokens::{radius, size};

/// A tappable icon. Always carries an accessible label.
#[derive(Clone, PartialEq)]
pub struct IconButton {
    icon: Bytes,
    label: Cow<'static, str>,
    size: f32,
    active: bool,
    enabled: bool,
    badge: Option<u32>,
    dot: Option<Color>,
    tint: Option<Color>,
    on_press: Option<EventHandler<Event<PressEventData>>>,
    key: DiffKey,
}

impl std::fmt::Debug for IconButton {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IconButton")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl KeyExt for IconButton {
    fn key_mut(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl IconButton {
    /// `label` is read by screen readers and shown as tooltip.
    #[must_use]
    pub fn new(icon: Bytes, label: impl Into<Cow<'static, str>>) -> Self {
        Self {
            icon,
            label: label.into(),
            size: size::BUTTON,
            active: false,
            enabled: true,
            badge: None,
            dot: None,
            tint: None,
            on_press: None,
            key: DiffKey::None,
        }
    }

    /// Square side in pixels (32, 36, 40 or 44).
    #[must_use]
    pub const fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Toggled on (accent tint).
    #[must_use]
    pub const fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Red counter in the corner; hidden at zero.
    #[must_use]
    pub const fn badge(mut self, count: u32) -> Self {
        self.badge = Some(count);
        self
    }

    /// Small status dot in the corner.
    #[must_use]
    pub const fn dot(mut self, color: Color) -> Self {
        self.dot = Some(color);
        self
    }

    /// Icon colour when not active.
    #[must_use]
    pub const fn tint(mut self, color: Color) -> Self {
        self.tint = Some(color);
        self
    }

    #[must_use]
    pub fn on_press(mut self, handler: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(handler.into());
        self
    }
}

impl Component for IconButton {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut hovering = use_state(|| false);
        let hint = use_hint();
        let a11y_id = use_a11y();
        let focus = use_focus(a11y_id);
        let side = self.size;
        let icon_size = if side <= 32.0 {
            size::ICON_SM
        } else if side <= 36.0 {
            18.0
        } else {
            size::ICON
        };
        let background = if self.active {
            Some(p.accent_soft)
        } else if hovering() && self.enabled {
            Some(p.s3)
        } else {
            None
        };
        let color = if self.active {
            p.accent_text
        } else {
            self.tint.unwrap_or(p.t2)
        };
        let on_press = self.on_press.clone();
        let badge = self.badge.filter(|n| *n > 0);

        rect()
            .a11y_id(a11y_id)
            .a11y_focusable(self.enabled)
            .a11y_role(AccessibilityRole::Button)
            .a11y_alt(self.label.to_string())
            .width(Size::px(side))
            .height(Size::px(side))
            .corner_radius(radius::MD)
            .center()
            .opacity(if self.enabled { 1.0 } else { 0.4 })
            .map(background, Rect::background)
            .maybe(focus() == Focus::Keyboard, |r| {
                r.border(
                    Border::new()
                        .fill(p.focus)
                        .width(p.focus_width)
                        .alignment(BorderAlignment::Outer),
                )
            })
            .on_sized(move |e: Event<SizedEventData>| hint.track(&e))
            .on_pointer_enter(move |e: Event<PointerEventData>| {
                hovering.set(super::press::is_mouse(&e));
                hint.enter(&e);
            })
            .on_pointer_leave(move |_| {
                hovering.set(false);
                hint.leave();
            })
            .on_pointer_down(move |_| hint.leave())
            .maybe(self.enabled, |r| {
                r.map(on_press, |r, handler| r.on_tap(move |e| handler.call(e)))
            })
            .child(Icon::new(self.icon.clone()).size(icon_size).color(color))
            .maybe_child(hint.render(self.label.clone(), side))
            .map(self.dot, |r, dot| {
                r.child(
                    rect()
                        .position(Position::new_absolute().top(4.0).right(4.0))
                        .width(Size::px(7.0))
                        .height(Size::px(7.0))
                        .corner_radius(radius::FULL)
                        .background(dot),
                )
            })
            .map(badge, |r, count| {
                r.child(
                    rect()
                        .position(Position::new_absolute().top(0.0).right(0.0))
                        .min_width(Size::px(16.0))
                        .height(Size::px(16.0))
                        .padding((0.0, 4.0))
                        .corner_radius(radius::FULL)
                        .background(p.danger)
                        .center()
                        .child(
                            label()
                                .text(count.to_string())
                                .font_size(9.0)
                                .font_weight(FontWeight::BOLD)
                                .color(Color::WHITE),
                        ),
                )
            })
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}
