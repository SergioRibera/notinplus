//! Floating (or docked) bar of actions over a selection.

use std::borrow::Cow;

use freya::prelude::*;

use super::icon::Icon;
use crate::components::TapExt;
use crate::theme::palette;
use crate::tokens::{radius, text};

/// One item of a [`SelectionBar`].
#[derive(Clone, PartialEq)]
pub enum BarItem {
    Action {
        label: Option<Cow<'static, str>>,
        icon: Option<Bytes>,
        on_press: EventHandler<()>,
    },
    /// Highlighter swatches; `active` is the current colour index.
    Swatches {
        colors: Vec<Color>,
        active: Option<usize>,
        on_pick: EventHandler<usize>,
    },
    /// Read-only caption ("3 seleccionados", "Romanos 8:16–17").
    Caption(Cow<'static, str>),
    Divider,
}

impl std::fmt::Debug for BarItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Action { label, .. } => write!(f, "Action({label:?})"),
            Self::Swatches { active, .. } => write!(f, "Swatches({active:?})"),
            Self::Caption(c) => write!(f, "Caption({c})"),
            Self::Divider => f.write_str("Divider"),
        }
    }
}

impl BarItem {
    #[must_use]
    pub fn action(
        label: impl Into<Cow<'static, str>>,
        icon: Bytes,
        on_press: impl Into<EventHandler<()>>,
    ) -> Self {
        Self::Action {
            label: Some(label.into()),
            icon: Some(icon),
            on_press: on_press.into(),
        }
    }

    #[must_use]
    pub fn icon(icon: Bytes, on_press: impl Into<EventHandler<()>>) -> Self {
        Self::Action {
            label: None,
            icon: Some(icon),
            on_press: on_press.into(),
        }
    }
}

/// Inverse-surface action bar. Docked bars span the full width at the
/// bottom (phones); floating bars hug their content.
#[derive(Clone, PartialEq, Debug)]
pub struct SelectionBar {
    items: Vec<BarItem>,
    docked: bool,
}

impl SelectionBar {
    #[must_use]
    pub fn new(items: impl IntoIterator<Item = BarItem>) -> Self {
        Self {
            items: items.into_iter().collect(),
            docked: false,
        }
    }

    #[must_use]
    pub const fn docked(mut self, docked: bool) -> Self {
        self.docked = docked;
        self
    }
}

impl BarItem {
    /// A labelled action as an icon over its label, for docked bars.
    fn tile(&self, index: usize) -> Element {
        let Self::Action {
            label: text_value,
            icon,
            on_press,
        } = self
        else {
            return self.render(index);
        };
        let p = palette();
        let handler = on_press.clone();
        rect()
            .key(index)
            .min_width(Size::px(56.0))
            .padding((6.0, 6.0))
            .corner_radius(radius::SM)
            .cross_align(Alignment::Center)
            .spacing(4.0)
            .a11y_role(AccessibilityRole::Button)
            .on_tap(move |_| handler.call(()))
            .maybe_child(
                icon.clone()
                    .map(|i| Icon::new(i).size(18.0).color(p.inv_text)),
            )
            .maybe_child(text_value.clone().map(|t| {
                label()
                    .text(t)
                    .font_size(text::META)
                    .color(p.inv_text)
                    .max_lines(1)
            }))
            .into_element()
    }

    fn render(&self, index: usize) -> Element {
        let p = palette();
        match self {
            Self::Action {
                label: text_value,
                icon,
                on_press,
            } => {
                let handler = on_press.clone();
                rect()
                    .key(index)
                    .height(Size::px(40.0))
                    .padding((0.0, 10.0))
                    .corner_radius(radius::SM)
                    .horizontal()
                    .center()
                    .spacing(6.0)
                    .a11y_role(AccessibilityRole::Button)
                    .on_tap(move |_| handler.call(()))
                    .maybe_child(
                        icon.clone()
                            .map(|i| Icon::new(i).size(16.0).color(p.inv_text)),
                    )
                    .maybe_child(text_value.clone().map(|t| {
                        label()
                            .text(t)
                            .font_size(text::META)
                            .font_weight(FontWeight::MEDIUM)
                            .color(p.inv_text)
                    }))
                    .into_element()
            }
            Self::Swatches {
                colors,
                active,
                on_pick,
            } => rect()
                .key(index)
                .padding((0.0, 8.0))
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(8.0)
                .children(colors.iter().enumerate().map(|(ci, color)| {
                    let handler = on_pick.clone();
                    let selected = *active == Some(ci);
                    rect()
                        .key(ci)
                        .a11y_role(AccessibilityRole::Button)
                        .width(Size::px(24.0))
                        .height(Size::px(24.0))
                        .corner_radius(radius::FULL)
                        .background(*color)
                        .maybe(selected, |r| {
                            r.border(
                                Border::new()
                                    .fill(p.inv_text)
                                    .width(2.0)
                                    .alignment(BorderAlignment::Outer),
                            )
                        })
                        .on_tap(move |_| handler.call(ci))
                        .into_element()
                }))
                .into_element(),
            Self::Caption(caption) => rect()
                .key(index)
                .padding((0.0, 10.0))
                .child(
                    label()
                        .text(caption.clone())
                        .font_size(text::META)
                        .font_weight(FontWeight::SEMI_BOLD)
                        .color(p.inv_text),
                )
                .into_element(),
            Self::Divider => rect()
                .key(index)
                .width(Size::px(1.0))
                .height(Size::px(20.0))
                .background(p.inv_line)
                .into_element(),
        }
    }
}

impl SelectionBar {
    /// The items with every divider that separates nothing left out: at
    /// either end, or right after another one.
    fn shown(&self) -> impl Iterator<Item = &BarItem> {
        let is_divider = |item: &BarItem| matches!(item, BarItem::Divider);
        let last = self.items.iter().rposition(|i| !is_divider(i));
        self.items
            .iter()
            .enumerate()
            .filter(move |(at, item)| {
                !is_divider(item)
                    || (*at > 0
                        && last.is_some_and(|last| *at < last)
                        && !is_divider(&self.items[at - 1]))
            })
            .map(|(_, item)| item)
    }
}

impl SelectionBar {
    /// Docked, the bar takes two rows so a phone fits it: the caption, the
    /// inks and icon buttons first, then every labelled action as an icon
    /// over its label.
    fn docked_rows(&self) -> Rect {
        let p = palette();
        let labelled = |item: &&BarItem| matches!(item, BarItem::Action { label: Some(_), .. });
        let head = self
            .items
            .iter()
            .filter(|item| !labelled(item) && !matches!(item, BarItem::Divider));
        let (lead, trail): (Vec<_>, Vec<_>) =
            head.partition(|item| !matches!(item, BarItem::Action { .. }));
        let actions: Vec<Element> = self
            .items
            .iter()
            .filter(labelled)
            .enumerate()
            .map(|(index, item)| item.tile(index))
            .collect();
        rect()
            .a11y_role(AccessibilityRole::Toolbar)
            .width(Size::fill())
            .padding((4.0, 8.0, 6.0, 8.0))
            .background(p.inv)
            .border(
                Border::new()
                    .fill(p.inv_line)
                    .width(1.0)
                    .alignment(BorderAlignment::Inner),
            )
            .shadow(Shadow::new().y(8.0).blur(24.0).color(p.shadow))
            .spacing(2.0)
            .child(
                rect()
                    .width(Size::fill())
                    .min_height(Size::px(44.0))
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .children(
                        lead.iter()
                            .enumerate()
                            .map(|(index, item)| item.render(index)),
                    )
                    .child(rect().width(Size::flex(1.0)))
                    .children(
                        trail
                            .iter()
                            .enumerate()
                            .map(|(index, item)| item.render(lead.len() + index)),
                    ),
            )
            .maybe(!actions.is_empty(), |r| {
                r.child(
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .main_align(Alignment::SpaceAround)
                        .children(actions),
                )
            })
    }
}

impl Component for SelectionBar {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        if self.docked {
            return self.docked_rows().into_element();
        }
        rect()
            .a11y_role(AccessibilityRole::Toolbar)
            .maybe(self.docked, |r| {
                r.width(Size::fill()).main_align(Alignment::SpaceAround)
            })
            .min_height(Size::px(48.0))
            .padding((4.0, 6.0))
            .corner_radius(if self.docked { 0.0 } else { radius::LG })
            .background(p.inv)
            .border(
                Border::new()
                    .fill(p.inv_line)
                    .width(1.0)
                    .alignment(BorderAlignment::Inner),
            )
            .shadow(Shadow::new().y(8.0).blur(24.0).color(p.shadow))
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(2.0)
            .children(
                self.shown()
                    .enumerate()
                    .map(|(index, item)| item.render(index)),
            )
            .into_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(items: Vec<BarItem>) -> Vec<String> {
        SelectionBar::new(items)
            .shown()
            .map(|item| format!("{item:?}"))
            .collect()
    }

    #[test]
    fn dividers_only_separate_something() {
        let caption = || BarItem::Caption("Tocá para elegir".into());
        assert_eq!(
            shown(vec![caption(), BarItem::Divider]),
            ["Caption(Tocá para elegir)"]
        );
        assert_eq!(
            shown(vec![
                BarItem::Divider,
                caption(),
                BarItem::Divider,
                BarItem::Divider,
                caption(),
            ]),
            [
                "Caption(Tocá para elegir)",
                "Divider",
                "Caption(Tocá para elegir)"
            ],
        );
    }
}
