//! Text button with optional leading icon.

use std::borrow::Cow;

use freya::prelude::*;

use super::icon::Icon;
use super::spinner::Spinner;
use crate::theme::{Palette, palette};
use crate::tokens::{radius, size, text};

/// Visual weight of a [`Button`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonVariant {
    /// The one main action of a surface (accent fill).
    Primary,
    /// Neutral filled action.
    #[default]
    Secondary,
    /// Text-only action (Cancelar, Cambiar).
    Ghost,
    /// Accent-tinted action (Compartir in the mesa bar).
    Tonal,
    /// Destructive text action (Papelera, Terminar sesión).
    Danger,
    /// Destructive filled action (Borrar para siempre).
    DangerFill,
}

/// Height of a [`Button`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonSize {
    /// 32 px: inside rows and toasts.
    Small,
    /// 40 px: sheets and bars.
    #[default]
    Medium,
    /// 48 px: phone primary actions.
    Large,
}

impl ButtonSize {
    const fn height(self) -> f32 {
        match self {
            Self::Small => size::BUTTON_SM,
            Self::Medium => size::BUTTON,
            Self::Large => size::BUTTON_LG,
        }
    }

    const fn padding(self) -> f32 {
        match self {
            Self::Small => 12.0,
            Self::Medium | Self::Large => 16.0,
        }
    }

    const fn font_size(self) -> f32 {
        match self {
            Self::Small => text::META,
            Self::Medium | Self::Large => text::BODY,
        }
    }
}

impl ButtonVariant {
    /// `(background, foreground)` for this variant.
    const fn colors(self, p: &Palette) -> (Option<Color>, Color) {
        match self {
            Self::Primary => (Some(p.accent), p.on_accent),
            Self::Secondary => (Some(p.s3), p.t1),
            Self::Ghost => (None, p.t2),
            Self::Tonal => (Some(p.accent_soft), p.accent_text),
            Self::Danger => (None, p.danger),
            Self::DangerFill => (Some(p.danger), Color::WHITE),
        }
    }
}

/// A labelled button.
///
/// ```rust,ignore
/// Button::new("Crear y abrir").primary().on_press(|_| create())
/// ```
#[derive(Clone, PartialEq)]
pub struct Button {
    label: Cow<'static, str>,
    variant: ButtonVariant,
    size: ButtonSize,
    icon: Option<Bytes>,
    loading: bool,
    enabled: bool,
    full_width: bool,
    on_press: Option<EventHandler<Event<PressEventData>>>,
    key: DiffKey,
}

impl std::fmt::Debug for Button {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Button")
            .field("label", &self.label)
            .field("variant", &self.variant)
            .finish_non_exhaustive()
    }
}

impl KeyExt for Button {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Button {
    #[must_use]
    pub fn new(label: impl Into<Cow<'static, str>>) -> Self {
        Self {
            label: label.into(),
            variant: ButtonVariant::default(),
            size: ButtonSize::default(),
            icon: None,
            loading: false,
            enabled: true,
            full_width: false,
            on_press: None,
            key: DiffKey::None,
        }
    }

    #[must_use]
    pub const fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    #[must_use]
    pub const fn primary(self) -> Self {
        self.variant(ButtonVariant::Primary)
    }

    #[must_use]
    pub const fn ghost(self) -> Self {
        self.variant(ButtonVariant::Ghost)
    }

    #[must_use]
    pub const fn tonal(self) -> Self {
        self.variant(ButtonVariant::Tonal)
    }

    #[must_use]
    pub const fn danger(self) -> Self {
        self.variant(ButtonVariant::Danger)
    }

    #[must_use]
    pub const fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    #[must_use]
    pub const fn small(self) -> Self {
        self.size(ButtonSize::Small)
    }

    #[must_use]
    pub fn icon(mut self, icon: Bytes) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Replace the icon with a spinner and ignore presses.
    #[must_use]
    pub const fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Stretch to the parent's width.
    #[must_use]
    pub const fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    #[must_use]
    pub fn on_press(mut self, handler: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(handler.into());
        self
    }
}

impl Component for Button {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut hovering = use_state(|| false);
        let mut pressed = use_state(|| false);
        let a11y_id = use_a11y();
        let focus = use_focus(a11y_id);

        let interactive = self.enabled && !self.loading;
        let (background, foreground) = self.variant.colors(&p);
        let background = match background {
            Some(bg) if pressed() => Some(bg.mul_if(true, 0.85)),
            Some(bg) if hovering() && interactive => Some(bg.mul_if(true, 0.93)),
            Some(bg) => Some(bg),
            None if pressed() => Some(p.s4),
            None if hovering() && interactive => Some(p.s3),
            None => None,
        };
        let on_press = self.on_press.clone();
        let leading = if self.loading {
            Some(Spinner::default().color(foreground).into_element())
        } else {
            self.icon
                .clone()
                .map(|icon| Icon::new(icon).size(18.0).color(foreground).into_element())
        };

        rect()
            .a11y_id(a11y_id)
            .a11y_focusable(interactive)
            .a11y_role(AccessibilityRole::Button)
            .a11y_alt(self.label.to_string())
            .height(Size::px(self.size.height()))
            .maybe(self.full_width, |r| r.width(Size::fill()))
            .padding((0.0, self.size.padding()))
            .corner_radius(radius::MD)
            .horizontal()
            .center()
            .spacing(8.0)
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
            .maybe(interactive, |r| {
                r.on_pointer_enter(move |_| hovering.set(true))
                    .on_pointer_leave(move |_| {
                        hovering.set(false);
                        pressed.set(false);
                    })
                    .on_pointer_down(move |_| pressed.set(true))
                    .on_pointer_press(move |_| pressed.set(false))
                    .map(on_press, |r, handler| r.on_press(move |e| handler.call(e)))
            })
            .maybe_child(leading)
            .child(
                label()
                    .text(self.label.clone())
                    .font_size(self.size.font_size())
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(foreground)
                    .max_lines(1),
            )
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}
