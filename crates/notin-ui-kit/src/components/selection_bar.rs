//! Floating (or docked) bar of actions over a selection.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

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

/// How much of a floating bar shows, from roomiest to tightest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fit {
    /// Icons and labels.
    Labels,
    /// Icons alone.
    Icons,
    /// Icons alone, no caption.
    Bare,
}

impl Fit {
    const ALL: [Self; 3] = [Self::Labels, Self::Icons, Self::Bare];
}

/// Room a docked action needs to show its label whole under the icon.
const TILE_WITH_LABEL: f32 = 80.0;

/// Widths a floating bar measured at each [`Fit`], for the items they
/// were measured with.
#[derive(Default)]
struct Measured {
    items: String,
    widths: [Option<f32>; 3],
}

/// Inverse-surface action bar. Docked bars span the full width at the
/// bottom (phones); floating bars hug their content.
#[derive(Clone, PartialEq, Debug)]
pub struct SelectionBar {
    items: Vec<BarItem>,
    docked: bool,
    /// Width the bar may take; it drops labels, then its caption, to fit.
    room: Option<f32>,
}

impl SelectionBar {
    #[must_use]
    pub fn new(items: impl IntoIterator<Item = BarItem>) -> Self {
        Self {
            items: items.into_iter().collect(),
            docked: false,
            room: None,
        }
    }

    /// Fit in `room` pixels: past it, actions show their icon alone (the
    /// label stays for screen readers), then the caption goes. Docked
    /// bars drop the labels under the icons once they would not fit whole.
    #[must_use]
    pub const fn fit(mut self, room: f32) -> Self {
        self.room = Some(room);
        self
    }

    #[must_use]
    pub const fn docked(mut self, docked: bool) -> Self {
        self.docked = docked;
        self
    }
}

impl BarItem {
    /// A labelled action as an icon over its label, for docked bars; the
    /// icon alone when `labels` is off.
    fn tile(&self, index: usize, labels: bool) -> Element {
        let Self::Action {
            label: text_value,
            icon,
            on_press,
        } = self
        else {
            return self.render(index, Fit::Labels);
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
            .maybe(!labels, |r| {
                r.a11y_alt(text_value.clone().unwrap_or_default())
            })
            .on_tap(move |_| handler.call(()))
            .maybe_child(
                icon.clone()
                    .map(|i| Icon::new(i).size(18.0).color(p.inv_text)),
            )
            .maybe_child(
                text_value
                    .clone()
                    .filter(|_| labels || icon.is_none())
                    .map(|t| {
                        label()
                            .text(t)
                            .font_size(text::META)
                            .color(p.inv_text)
                            .max_lines(1)
                    }),
            )
            .into_element()
    }

    #[cfg_attr(feature = "sismo", sismo::function("kit::bar_item"))]
    fn render(&self, index: usize, level: Fit) -> Element {
        let p = palette();
        match self {
            Self::Action {
                label: text_value,
                icon,
                on_press,
            } => {
                let handler = on_press.clone();
                // Icon alone once labels do not fit; an action without an
                // icon keeps its label.
                let labelled = level == Fit::Labels || icon.is_none();
                rect()
                    .key(index)
                    .height(Size::px(40.0))
                    .padding((0.0, if labelled { 10.0 } else { 8.0 }))
                    .maybe(!labelled, |r| {
                        r.a11y_alt(text_value.clone().unwrap_or_default())
                    })
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
                    .maybe_child(text_value.clone().filter(|_| labelled).map(|t| {
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
            Self::Caption(_) if level == Fit::Bare => rect().key(index).into_element(),
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
    #[cfg_attr(feature = "sismo", sismo::function("kit::selection_bar_docked_rows"))]
    fn docked_rows(&self) -> Rect {
        let p = palette();
        let labelled = |item: &&BarItem| matches!(item, BarItem::Action { label: Some(_), .. });
        let head = self
            .items
            .iter()
            .filter(|item| !labelled(item) && !matches!(item, BarItem::Divider));
        let (lead, trail): (Vec<_>, Vec<_>) =
            head.partition(|item| !matches!(item, BarItem::Action { .. }));
        let count = self.items.iter().filter(labelled).count();
        #[allow(clippy::cast_precision_loss, reason = "a handful of actions")]
        let labels = self
            .room
            .is_none_or(|room| count == 0 || room / count as f32 >= TILE_WITH_LABEL);
        let actions: Vec<Element> = self
            .items
            .iter()
            .filter(labelled)
            .enumerate()
            .map(|(index, item)| item.tile(index, labels))
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
                            .map(|(index, item)| item.render(index, Fit::Labels)),
                    )
                    .child(rect().width(Size::flex(1.0)))
                    .children(
                        trail
                            .iter()
                            .enumerate()
                            .map(|(index, item)| item.render(lead.len() + index, Fit::Labels)),
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
    #[cfg_attr(feature = "sismo", sismo::function("kit::selection_bar"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        if self.docked {
            return self.docked_rows().into_element();
        }
        // The bar measures itself at each fit as it shows it, and shows the
        // roomiest one that fits: a width it has not measured yet is tried.
        let measured = use_hook(|| Rc::new(RefCell::new(Measured::default())));
        let mut retry = use_state(|| 0u32);
        let _ = *retry.read();
        let items = format!("{:?}", self.items);
        if measured.borrow().items != items {
            *measured.borrow_mut() = Measured {
                items,
                widths: [None; 3],
            };
        }
        let room = self.room.unwrap_or(f32::INFINITY);
        let fit = {
            let widths = measured.borrow().widths;
            Fit::ALL
                .into_iter()
                .zip(widths)
                .find(|(_, width)| width.is_none_or(|width| width <= room))
                .map_or(Fit::Bare, |(fit, _)| fit)
        };
        let sizes = Rc::clone(&measured);
        let index = Fit::ALL.iter().position(|f| *f == fit).unwrap_or(2);
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
            .on_sized(move |e: Event<SizedEventData>| {
                let width = e.area.width();
                let mut sizes = sizes.borrow_mut();
                if sizes.widths[index].is_none_or(|known| (known - width).abs() > 0.5) {
                    sizes.widths[index] = Some(width);
                    // Too wide: show the next fit.
                    if width > room && index < 2 {
                        *retry.write() += 1;
                    }
                }
            })
            .children(
                self.shown()
                    .enumerate()
                    .map(|(at, item)| item.render(at, fit)),
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
