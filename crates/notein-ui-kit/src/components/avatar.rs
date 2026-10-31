//! People: avatar, presence chip and the wrapped presence list.

use std::borrow::Cow;

use freya::prelude::*;
use notein_i18n::tr;

use crate::components::TapExt;
use crate::theme::palette;
use crate::tokens::{radius, text};

/// Circle with initials.
#[derive(Clone, Debug, PartialEq)]
pub struct Avatar {
    initials: Cow<'static, str>,
    color: Color,
    size: f32,
    presence: Option<bool>,
}

impl Avatar {
    #[must_use]
    pub fn new(initials: impl Into<Cow<'static, str>>, color: Color) -> Self {
        Self {
            initials: initials.into(),
            color,
            size: 28.0,
            presence: None,
        }
    }

    #[must_use]
    pub const fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Green dot when present now, grey when gone.
    #[must_use]
    pub const fn presence(mut self, live: bool) -> Self {
        self.presence = Some(live);
        self
    }
}

impl Component for Avatar {
    #[cfg_attr(feature = "sismo", sismo::function("kit::avatar"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let side = self.size;
        rect()
            .width(Size::px(side))
            .height(Size::px(side))
            .corner_radius(radius::FULL)
            .background(self.color)
            .center()
            .child(
                label()
                    .text(self.initials.clone())
                    .font_size((side * 0.42).round())
                    .font_weight(FontWeight::BOLD)
                    .color(Color::from_rgb(21, 21, 26)),
            )
            .map(self.presence, |r, live| {
                r.child(
                    rect()
                        .position(Position::new_absolute().bottom(-1.0).right(-1.0))
                        .width(Size::px(10.0))
                        .height(Size::px(10.0))
                        .corner_radius(radius::FULL)
                        .background(if live { p.ok } else { p.t3 })
                        .border(
                            Border::new()
                                .fill(p.s3)
                                .width(2.0)
                                .alignment(BorderAlignment::Outer),
                        ),
                )
            })
    }
}

/// One participant in a shared session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub initials: Cow<'static, str>,
    pub name: Cow<'static, str>,
    /// "Dueño", "Edita · Cuaderno", "Mira".
    pub status: Cow<'static, str>,
    pub color: Color,
    pub live: bool,
}

/// Chip: avatar, name and status.
#[derive(Clone, Debug, PartialEq)]
pub struct PresenceChip {
    person: Person,
    compact: bool,
    on_press: Option<EventHandler<Event<PressEventData>>>,
}

impl PresenceChip {
    #[must_use]
    pub const fn new(person: Person) -> Self {
        Self {
            person,
            compact: false,
            on_press: None,
        }
    }

    /// Hide the status text.
    #[must_use]
    pub const fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    #[must_use]
    pub fn on_press(mut self, handler: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(handler.into());
        self
    }
}

impl Component for PresenceChip {
    #[cfg_attr(feature = "sismo", sismo::function("kit::presence_chip"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let person = &self.person;
        let on_press = self.on_press.clone();
        rect()
            .height(Size::px(36.0))
            .padding((0.0, 12.0, 0.0, 4.0))
            .corner_radius(radius::FULL)
            .background(p.s3)
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(8.0)
            .a11y_role(AccessibilityRole::Button)
            .a11y_alt(format!("{} · {}", person.name, person.status))
            .map(on_press, |r, handler| r.on_tap(move |e| handler.call(e)))
            .child(Avatar::new(person.initials.clone(), person.color).presence(person.live))
            .child(
                label()
                    .text(person.name.clone())
                    .font_size(text::META)
                    .font_weight(FontWeight::SEMI_BOLD)
                    .color(p.t1)
                    .max_lines(1),
            )
            .maybe_child((!self.compact).then(|| {
                label()
                    .text(person.status.clone())
                    .font_size(text::LABEL)
                    .color(p.t3)
                    .max_lines(1)
            }))
    }
}

/// People in a session, wrapped to at most two rows. What does not
/// fit is summarised as a trailing `+N` chip that opens the full list.
#[derive(Clone, Debug, PartialEq)]
pub struct PresenceList {
    people: Vec<Person>,
    capacity: usize,
    on_person: Option<EventHandler<usize>>,
    on_more: Option<EventHandler<()>>,
}

impl PresenceList {
    /// `capacity` is how many chips fit in two rows at the current
    /// width; the caller measures it (it is width dependent).
    #[must_use]
    pub const fn new(people: Vec<Person>, capacity: usize) -> Self {
        Self {
            people,
            capacity,
            on_person: None,
            on_more: None,
        }
    }

    /// Receives the index of the tapped person.
    #[must_use]
    pub fn on_person(mut self, handler: impl Into<EventHandler<usize>>) -> Self {
        self.on_person = Some(handler.into());
        self
    }

    #[must_use]
    pub fn on_more(mut self, handler: impl Into<EventHandler<()>>) -> Self {
        self.on_more = Some(handler.into());
        self
    }

    /// How many chips to show and how many go into `+N`.
    #[must_use]
    pub const fn split(total: usize, capacity: usize) -> (usize, usize) {
        if total <= capacity {
            (total, 0)
        } else {
            let shown = capacity.saturating_sub(1);
            (shown, total - shown)
        }
    }
}

impl Component for PresenceList {
    #[cfg_attr(feature = "sismo", sismo::function("kit::presence_list"))]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let (shown, hidden) = Self::split(self.people.len(), self.capacity);
        let on_more = self.on_more.clone();
        rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::wrap_spacing(8.0))
            .spacing(8.0)
            .children(
                self.people
                    .iter()
                    .take(shown)
                    .enumerate()
                    .map(|(index, person)| {
                        let handler = self.on_person.clone();
                        PresenceChip::new(person.clone())
                            .on_press(move |_| {
                                if let Some(h) = &handler {
                                    h.call(index);
                                }
                            })
                            .into_element()
                    }),
            )
            .maybe_child((hidden > 0).then(|| {
                rect()
                    .height(Size::px(36.0))
                    .padding((0.0, 14.0))
                    .corner_radius(radius::FULL)
                    .background(p.accent_soft)
                    .center()
                    .a11y_role(AccessibilityRole::Button)
                    .a11y_alt(tr!("kit-avatar-more", "count" => hidden))
                    .map(on_more, |r, handler| r.on_tap(move |_| handler.call(())))
                    .child(
                        label()
                            .text(format!("+{hidden}"))
                            .font_size(text::META)
                            .font_weight(FontWeight::BOLD)
                            .color(p.accent_text),
                    )
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::PresenceList;

    #[test]
    fn overflow_reserves_the_last_slot_for_more() {
        assert_eq!(PresenceList::split(3, 6), (3, 0));
        assert_eq!(PresenceList::split(6, 6), (6, 0));
        assert_eq!(PresenceList::split(9, 6), (5, 4));
    }
}
