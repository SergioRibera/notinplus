//! The action list shown by long-press, right click, the `⋯` button
//! and the command palette: one shape for every surface.

use std::borrow::Cow;

use freya::prelude::*;

use super::icon::Icon;
use super::kind_badge::KindBadge;
use super::list_row::strut;
use super::scroll_content::TapExt;
use super::sheet::Popover;
use crate::modal::ModalController;
use crate::theme::palette;
use crate::tokens::{radius, text};

/// One entry of an [`ActionList`].
#[derive(Clone, PartialEq)]
pub enum ActionEntry {
    /// Upper-case group header.
    Header(Cow<'static, str>),
    Divider,
    Item(Action),
}

impl std::fmt::Debug for ActionEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Header(h) => write!(f, "Header({h})"),
            Self::Divider => f.write_str("Divider"),
            Self::Item(a) => write!(f, "Item({})", a.label),
        }
    }
}

/// A runnable action.
#[derive(Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)] // independent display flags of one menu row
pub struct Action {
    pub label: Cow<'static, str>,
    pub subtitle: Option<Cow<'static, str>>,
    pub icon: Option<Bytes>,
    /// Kind colour; draws the icon as a kind badge.
    pub kind_color: Option<Color>,
    pub shortcut: Option<Cow<'static, str>>,
    pub checked: bool,
    pub submenu: bool,
    pub danger: bool,
    pub enabled: bool,
    pub highlighted: bool,
    pub on_run: EventHandler<()>,
}

impl std::fmt::Debug for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Action")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl Action {
    #[must_use]
    pub fn new(label: impl Into<Cow<'static, str>>, on_run: impl Into<EventHandler<()>>) -> Self {
        Self {
            label: label.into(),
            subtitle: None,
            icon: None,
            kind_color: None,
            shortcut: None,
            checked: false,
            submenu: false,
            danger: false,
            enabled: true,
            highlighted: false,
            on_run: on_run.into(),
        }
    }

    /// An action that closes the menu it is in, then runs `run`.
    #[must_use]
    pub fn closing(label: impl Into<Cow<'static, str>>, run: impl Fn() + 'static) -> Self {
        Self::new(
            label,
            EventHandler::new(move |()| {
                ModalController::get().close();
                run();
            }),
        )
    }

    #[must_use]
    pub fn icon(mut self, icon: Bytes) -> Self {
        self.icon = Some(icon);
        self
    }

    #[must_use]
    pub fn subtitle(mut self, subtitle: impl Into<Cow<'static, str>>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    #[must_use]
    pub const fn kind_color(mut self, color: Color) -> Self {
        self.kind_color = Some(color);
        self
    }

    #[must_use]
    pub fn shortcut(mut self, shortcut: impl Into<Cow<'static, str>>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    #[must_use]
    pub const fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    #[must_use]
    pub const fn submenu(mut self) -> Self {
        self.submenu = true;
        self
    }

    #[must_use]
    pub const fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Keyboard cursor is on this row (command palette).
    #[must_use]
    pub const fn highlighted(mut self, highlighted: bool) -> Self {
        self.highlighted = highlighted;
        self
    }
}

/// Vertical list of actions inside a popover or sheet.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ActionList {
    entries: Vec<ActionEntry>,
    width: Option<f32>,
}

impl ActionList {
    #[must_use]
    pub fn new(entries: impl IntoIterator<Item = ActionEntry>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
            width: None,
        }
    }

    #[must_use]
    pub const fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// Show the list in a popover hanging from `anchor`, in window
    /// coordinates.
    #[cfg_attr(feature = "sismo", sismo::function("kit::actions_show_at"))]
    pub fn show_at(self, anchor: Area) {
        let height = self
            .entries
            .iter()
            .map(|e| match e {
                ActionEntry::Item(_) => 36.0,
                ActionEntry::Divider => 9.0,
                ActionEntry::Header(_) => 28.0,
            })
            .sum::<f32>()
            + 12.0;
        Popover::new(anchor, self)
            .width(280.0)
            .estimated_height(height)
            .open();
    }

    /// Show the list hanging from the pointer at `at`.
    #[cfg_attr(feature = "sismo", sismo::function("kit::actions_show"))]
    pub fn show(self, at: CursorPoint) {
        #[allow(clippy::cast_possible_truncation, reason = "window coordinates")]
        let anchor = Area::new((at.x as f32, at.y as f32).into(), (1.0, 1.0).into());
        self.show_at(anchor);
    }
}

impl Component for ActionList {
    #[cfg_attr(feature = "sismo", sismo::function("kit::action_list"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        rect()
            .width(self.width.map_or_else(Size::fill, Size::px))
            .padding(6.0)
            .a11y_role(AccessibilityRole::Menu)
            .children(self.entries.iter().enumerate().map(|(index, entry)| {
                match entry {
                    ActionEntry::Header(title) => rect()
                        .key(index)
                        .padding((8.0, 10.0, 4.0, 10.0))
                        .child(
                            label()
                                .text(title.to_uppercase())
                                .font_size(text::CAPTION)
                                .font_weight(FontWeight::BOLD)
                                .letter_spacing(text::LABEL_TRACKING)
                                .color(p.t3),
                        )
                        .into_element(),
                    ActionEntry::Divider => rect()
                        .key(index)
                        .padding((4.0, 0.0))
                        .child(
                            rect()
                                .width(Size::fill())
                                .height(Size::px(1.0))
                                .background(p.line),
                        )
                        .into_element(),
                    ActionEntry::Item(action) => ActionRow {
                        action: action.clone(),
                    }
                    .into_element(),
                }
            }))
    }
}

#[derive(Clone, PartialEq)]
struct ActionRow {
    action: Action,
}

impl Component for ActionRow {
    #[cfg_attr(feature = "sismo", sismo::function("kit::action_row"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut hovering = use_state(|| false);
        let action = &self.action;
        let on_run = action.on_run.clone();
        let enabled = action.enabled;
        let background = (action.highlighted || hovering()).then_some(p.s4);
        rect()
            .a11y_role(AccessibilityRole::MenuItem)
            .a11y_alt(action.label.to_string())
            .width(Size::fill())
            .padding((4.0, 10.0))
            .corner_radius(radius::SM)
            .map(background, Rect::background)
            .opacity(if enabled { 1.0 } else { 0.4 })
            .maybe(enabled, |r| {
                r.on_pointer_enter(move |e: Event<PointerEventData>| {
                    hovering.set(super::press::is_mouse(&e));
                })
                .on_pointer_leave(move |_| hovering.set(false))
                .on_tap(move |_| on_run.call(()))
            })
            .child(self.content())
    }
}

impl ActionRow {
    /// Icon, labels and trailing marks.
    #[cfg_attr(feature = "sismo", sismo::function("kit::actions_content"))]
    fn content(&self) -> Rect {
        let p = palette();
        let action = &self.action;
        let color = if action.danger { p.danger } else { p.t1 };
        // Row height without its vertical padding.
        let min_height = if action.subtitle.is_some() {
            40.0
        } else {
            28.0
        };
        rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(10.0)
            .maybe_child(action.icon.clone().map(|icon| {
                match action.kind_color {
                    Some(kind) => KindBadge::new(icon, kind).size(26.0).into_element(),
                    None => Icon::new(icon)
                        .size(16.0)
                        .color(if action.danger { p.danger } else { p.t2 })
                        .into_element(),
                }
            }))
            .child(
                rect()
                    .width(Size::flex(1.0))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .child(strut(min_height))
                    .child(
                        rect()
                            .width(Size::fill())
                            .spacing(1.0)
                            .child(
                                label()
                                    .text(action.label.clone())
                                    .font_size(text::BODY)
                                    .color(color)
                                    .max_lines(1),
                            )
                            .maybe_child(action.subtitle.clone().map(|sub| {
                                label()
                                    .text(sub)
                                    .font_size(text::LABEL)
                                    .color(p.t3)
                                    .max_lines(1)
                            })),
                    ),
            )
            .maybe_child(
                action
                    .shortcut
                    .clone()
                    .map(|s| label().text(s).font_size(text::LABEL).color(p.t3)),
            )
            .maybe_child(action.checked.then(|| {
                Icon::new(freya::icons::lucide::check())
                    .size(16.0)
                    .color(p.accent)
            }))
            .maybe_child(action.submenu.then(|| {
                Icon::new(freya::icons::lucide::chevron_right())
                    .size(14.0)
                    .color(p.t3)
            }))
    }
}
