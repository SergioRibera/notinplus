//! Empty states, notices, progress bars and skeletons.

use std::borrow::Cow;

use freya::prelude::*;

use super::icon::Icon;
use crate::theme::palette;
use crate::tokens::{radius, text};

/// Centered illustration-free empty state with one way forward.
#[derive(Clone, PartialEq, Debug)]
pub struct EmptyState {
    icon: Bytes,
    title: Cow<'static, str>,
    body: Option<Cow<'static, str>>,
    action: Option<Element>,
    dashed: bool,
}

impl EmptyState {
    #[must_use]
    pub fn new(icon: Bytes, title: impl Into<Cow<'static, str>>) -> Self {
        Self {
            icon,
            title: title.into(),
            body: None,
            action: None,
            dashed: false,
        }
    }

    #[must_use]
    pub fn body(mut self, body: impl Into<Cow<'static, str>>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// The way forward (usually a [`super::Button`]).
    #[must_use]
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_element());
        self
    }

    /// Draw inside a dashed box (empty panel sections).
    #[must_use]
    pub const fn dashed(mut self) -> Self {
        self.dashed = true;
        self
    }
}

impl Component for EmptyState {
    #[cfg_attr(feature = "sismo", sismo::function("kit::empty_state"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        rect()
            .width(Size::fill())
            .padding((24.0, 20.0))
            .corner_radius(radius::LG)
            .maybe(self.dashed, |r| {
                r.border(
                    Border::new()
                        .fill(p.line_strong)
                        .width(1.0)
                        .alignment(BorderAlignment::Inner),
                )
            })
            .cross_align(Alignment::Center)
            .spacing(8.0)
            .child(Icon::new(self.icon.clone()).size(28.0).color(p.t3))
            .child(
                label()
                    .width(Size::fill())
                    .text(self.title.clone())
                    .font_size(text::BODY_LG)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(p.t1)
                    .text_align(TextAlign::Center),
            )
            .maybe_child(self.body.clone().map(|body| {
                label()
                    .width(Size::fill())
                    .text(body)
                    .font_size(text::META)
                    .color(p.t2)
                    .line_height(text::LINE_BODY)
                    .text_align(TextAlign::Center)
                    .max_width(Size::px(420.0))
            }))
            .maybe_child(
                self.action
                    .clone()
                    .map(|action| rect().padding((8.0, 0.0, 0.0, 0.0)).child(action)),
            )
    }
}

/// Tone of a [`Notice`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NoticeTone {
    #[default]
    Info,
    Warning,
    Danger,
    Offline,
    /// A check passed (a colour that reads well everywhere).
    Success,
}

/// Strip under a tab bar or inside a sheet. Never modal.
#[derive(Clone, PartialEq, Debug)]
pub struct Notice {
    tone: NoticeTone,
    text: Cow<'static, str>,
    detail: Option<Cow<'static, str>>,
    actions: Vec<Element>,
}

impl Notice {
    #[must_use]
    pub fn new(tone: NoticeTone, text: impl Into<Cow<'static, str>>) -> Self {
        Self {
            tone,
            text: text.into(),
            detail: None,
            actions: Vec::new(),
        }
    }

    #[must_use]
    pub fn detail(mut self, detail: impl Into<Cow<'static, str>>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    #[must_use]
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_element());
        self
    }
}

impl Component for Notice {
    #[cfg_attr(feature = "sismo", sismo::function("kit::notice"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let (icon, color) = match self.tone {
            NoticeTone::Info => (freya::icons::lucide::info(), p.t2),
            NoticeTone::Warning => (freya::icons::lucide::triangle_alert(), p.warn),
            NoticeTone::Danger => (freya::icons::lucide::circle_alert(), p.danger),
            NoticeTone::Offline => (freya::icons::lucide::wifi_off(), p.warn),
            NoticeTone::Success => (freya::icons::lucide::circle_check(), p.ok),
        };
        rect()
            .width(Size::fill())
            .padding((10.0, 12.0))
            .corner_radius(radius::MD)
            .background(match self.tone {
                // A step below the sheet so it still reads as a strip.
                NoticeTone::Info => p.s1,
                NoticeTone::Success => p.ok.with_a(28),
                NoticeTone::Warning | NoticeTone::Offline => p.warn.with_a(28),
                NoticeTone::Danger => p.danger.with_a(28),
            })
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(10.0)
            .a11y_role(AccessibilityRole::Status)
            .child(Icon::new(icon).size(16.0).color(color))
            .child(
                rect()
                    .width(Size::flex(1.0))
                    .spacing(2.0)
                    .child(
                        label()
                            .text(self.text.clone())
                            .font_size(text::META)
                            .color(p.t1)
                            .line_height(text::LINE_BODY),
                    )
                    .maybe_child(
                        self.detail
                            .clone()
                            .map(|d| label().text(d).font_size(text::LABEL).color(p.t3)),
                    ),
            )
            .children(self.actions.clone())
    }
}

/// Horizontal progress bar (0.0 to 1.0).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ProgressBar {
    value: f32,
    color: Option<Color>,
    height: f32,
}

impl ProgressBar {
    #[must_use]
    pub const fn new(value: f32) -> Self {
        Self {
            value,
            color: None,
            height: 6.0,
        }
    }

    /// Fill colour; the accent by default.
    #[must_use]
    pub const fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    #[must_use]
    pub const fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }
}

impl Component for ProgressBar {
    #[cfg_attr(feature = "sismo", sismo::function("kit::progress_bar"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let value = self.value.clamp(0.0, 1.0);
        rect()
            .width(Size::fill())
            .height(Size::px(self.height))
            .corner_radius(radius::FULL)
            .background(if p.dark { p.s4 } else { p.s3 })
            .a11y_role(AccessibilityRole::ProgressIndicator)
            .child(
                rect()
                    .width(Size::percent(value * 100.0))
                    .height(Size::fill())
                    .corner_radius(radius::FULL)
                    .background(self.color.unwrap_or(p.accent)),
            )
    }
}

/// Grey placeholder block shown while content loads.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Skeleton {
    width: Option<f32>,
    height: f32,
    radius: f32,
}

impl Skeleton {
    #[must_use]
    pub const fn new(height: f32) -> Self {
        Self {
            width: None,
            height,
            radius: radius::SM,
        }
    }

    #[must_use]
    pub const fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    #[must_use]
    pub const fn radius(mut self, radius: f32) -> Self {
        self.radius = radius;
        self
    }
}

impl Component for Skeleton {
    #[cfg_attr(feature = "sismo", sismo::function("kit::skeleton"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        rect()
            .width(self.width.map_or_else(Size::fill, Size::px))
            .height(Size::px(self.height))
            .corner_radius(self.radius)
            .background(p.s3)
    }
}

/// Keyboard shortcut hint (`Ctrl K`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Kbd {
    keys: Cow<'static, str>,
}

impl Kbd {
    #[must_use]
    pub fn new(keys: impl Into<Cow<'static, str>>) -> Self {
        Self { keys: keys.into() }
    }
}

impl Component for Kbd {
    #[cfg_attr(feature = "sismo", sismo::function("kit::kbd"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        rect()
            .height(Size::px(22.0))
            .padding((0.0, 6.0))
            .corner_radius(radius::XS)
            .background(p.s3)
            .center()
            .child(
                label()
                    .text(self.keys.clone())
                    .font_size(text::CAPTION)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(p.t2),
            )
    }
}
