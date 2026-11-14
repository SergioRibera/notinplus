//! The one row at the top of every dialog, sheet and full-screen modal.
//!
//! `[‹ back] [badge] [title ………] [actions] [×]`: what the dialog is,
//! what can be done to it as a whole, and the way out, on one line. A
//! dialog never adds a second title row in its body.

use std::borrow::Cow;

use freya::prelude::*;
use notin_i18n::tr;

use super::action_list::{Action, ActionEntry, ActionList};
use super::icon_button::IconButton;
use super::inline_edit::HeaderEditing;
use super::kind_badge::KindBadge;
use super::list_row::strut;
use super::press::press_anchor;
use super::text::{Text, TextRole};
use crate::modal::ModalController;
use crate::tokens::size;
use crate::width::width_class;

/// What a [`DialogHeader`] shows as its title.
#[derive(Clone, PartialEq)]
pub enum HeaderTitle {
    Text(Cow<'static, str>),
    /// Read on every render, for dialogs whose content changes what they
    /// are about (a tab, a page).
    State(State<String>),
    /// A component of its own: an [`super::InlineEdit::title`], or a
    /// title that follows live data.
    Element(Element),
}

impl std::fmt::Debug for HeaderTitle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text(text) => write!(f, "Text({text})"),
            Self::State(_) => f.write_str("State"),
            Self::Element(_) => f.write_str("Element"),
        }
    }
}

impl HeaderTitle {
    #[must_use]
    pub fn element(element: impl IntoElement) -> Self {
        Self::Element(element.into_element())
    }
}

impl From<&'static str> for HeaderTitle {
    fn from(text: &'static str) -> Self {
        Self::Text(Cow::Borrowed(text))
    }
}

impl From<String> for HeaderTitle {
    fn from(text: String) -> Self {
        Self::Text(Cow::Owned(text))
    }
}

impl From<Cow<'static, str>> for HeaderTitle {
    fn from(text: Cow<'static, str>) -> Self {
        Self::Text(text)
    }
}

impl From<State<String>> for HeaderTitle {
    fn from(title: State<String>) -> Self {
        Self::State(title)
    }
}

impl From<super::InlineEdit> for HeaderTitle {
    fn from(edit: super::InlineEdit) -> Self {
        Self::Element(edit.into_element())
    }
}

/// Something done to the dialog as a whole, as an icon in its header.
#[derive(Clone, PartialEq)]
pub struct HeaderAction {
    icon: Bytes,
    label: Cow<'static, str>,
    on_press: EventHandler<()>,
}

impl std::fmt::Debug for HeaderAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeaderAction")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl HeaderAction {
    /// `label` is the tooltip, the screen reader name and, on phones,
    /// the entry in the `⋯` menu.
    #[must_use]
    pub fn new(
        icon: Bytes,
        label: impl Into<Cow<'static, str>>,
        on_press: impl Into<EventHandler<()>>,
    ) -> Self {
        Self {
            icon,
            label: label.into(),
            on_press: on_press.into(),
        }
    }
}

/// How the header's × closes the dialog.
#[derive(Clone, PartialEq, Debug, Default)]
enum Close {
    /// Close the top modal.
    #[default]
    Top,
    Custom(EventHandler<()>),
    /// No × (flows that end with a button).
    None,
}

/// Header row of a dialog. See the module docs.
#[derive(Clone, PartialEq, Debug)]
pub struct DialogHeader {
    title: HeaderTitle,
    back: Option<EventHandler<()>>,
    kind: Option<(Bytes, Color)>,
    actions: Vec<HeaderAction>,
    close: Close,
}

impl DialogHeader {
    #[must_use]
    pub fn new(title: impl Into<HeaderTitle>) -> Self {
        Self {
            title: title.into(),
            back: None,
            kind: None,
            actions: Vec::new(),
            close: Close::Top,
        }
    }

    /// Back chevron before the title (multi-step dialogs, sub-pages).
    #[must_use]
    pub fn back(mut self, handler: impl Into<EventHandler<()>>) -> Self {
        self.back = Some(handler.into());
        self
    }

    /// [`Self::back`] when `handler` is some.
    #[must_use]
    pub fn maybe_back(mut self, handler: Option<EventHandler<()>>) -> Self {
        self.back = handler;
        self
    }

    /// Kind badge before the title (create and edit dialogs).
    #[must_use]
    pub fn kind(mut self, icon: Bytes, color: Color) -> Self {
        self.kind = Some((icon, color));
        self
    }

    #[must_use]
    pub fn action(mut self, action: HeaderAction) -> Self {
        self.actions.push(action);
        self
    }

    #[must_use]
    pub fn actions(mut self, actions: impl IntoIterator<Item = HeaderAction>) -> Self {
        self.actions.extend(actions);
        self
    }

    /// The × runs `handler` instead of closing the top modal.
    #[must_use]
    pub fn on_close(mut self, handler: impl Into<EventHandler<()>>) -> Self {
        self.close = Close::Custom(handler.into());
        self
    }

    /// No × (flows that must end with a button).
    #[must_use]
    pub fn without_close(mut self) -> Self {
        self.close = Close::None;
        self
    }
}

impl DialogHeader {
    /// The actions as icons, or behind `⋯` on phones once there is more
    /// than one, so the title keeps its room.
    fn action_buttons(&self, compact: bool, side: f32) -> Vec<Element> {
        if compact && self.actions.len() > 1 {
            let entries: Vec<ActionEntry> = self
                .actions
                .iter()
                .map(|a| {
                    let run = a.on_press.clone();
                    ActionEntry::Item(
                        Action::closing(a.label.clone(), move || run.call(())).icon(a.icon.clone()),
                    )
                })
                .collect();
            return vec![
                IconButton::new(freya::icons::lucide::ellipsis(), tr!("kit-more"))
                    .size(side)
                    .on_press(move |e: Event<PressEventData>| {
                        ActionList::new(entries.clone()).show_at(press_anchor(e.data()));
                    })
                    .into_element(),
            ];
        }
        self.actions
            .iter()
            .map(|a| {
                let run = a.on_press.clone();
                IconButton::new(a.icon.clone(), a.label.clone())
                    .size(side)
                    .on_press(move |_| run.call(()))
                    .into_element()
            })
            .collect()
    }

    fn title_element(&self) -> Element {
        match &self.title {
            HeaderTitle::Text(text) => title_text(text.clone()),
            HeaderTitle::State(title) => title_text(Cow::Owned(title.read().clone())),
            HeaderTitle::Element(element) => element.clone(),
        }
    }
}

fn title_text(text: Cow<'static, str>) -> Element {
    Text::new(text)
        .role(TextRole::Title)
        .lines(2)
        .fill()
        .into_element()
}

impl Component for DialogHeader {
    #[cfg_attr(feature = "sismo", sismo::function("kit::dialog_header"))]
    fn render(&self) -> impl IntoElement {
        let editing = use_state(|| false);
        use_provide_context(|| HeaderEditing(editing));
        let compact = width_class().is_compact();
        // Touch screens get the bigger target.
        let side = if compact {
            size::BUTTON
        } else {
            size::BUTTON_SM
        };
        let editing = *editing.read();
        let back = self.back.clone();
        let close = match &self.close {
            Close::None => None,
            Close::Top => Some(EventHandler::new(|()| ModalController::get().close())),
            Close::Custom(handler) => Some(handler.clone()),
        };
        rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(8.0)
            .child(strut(side))
            .maybe_child(back.filter(|_| !editing).map(|handler| {
                IconButton::new(freya::icons::lucide::chevron_left(), tr!("kit-sheet-back"))
                    .size(side)
                    .on_press(move |_| handler.call(()))
            }))
            .maybe_child(
                self.kind
                    .clone()
                    .map(|(icon, color)| KindBadge::new(icon, color).size(size::BUTTON_SM)),
            )
            .child(rect().width(Size::flex(1.0)).child(self.title_element()))
            .children(if editing {
                Vec::new()
            } else {
                self.action_buttons(compact, side)
            })
            .maybe_child(close.filter(|_| !editing).map(|handler| {
                IconButton::new(freya::icons::lucide::x(), tr!("kit-sheet-close"))
                    .size(side)
                    .on_press(move |_| handler.call(()))
            }))
    }
}
