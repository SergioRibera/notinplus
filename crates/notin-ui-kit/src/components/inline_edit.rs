//! A value read as text that turns into a field, in place, to change it.
//!
//! Every in-place edit works the same way: the pencil (or a tap on the
//! row) opens a field with the current value, Enter or ✓ keeps what was
//! typed, Escape or ✕ drops it. Leaving the field does neither: a tap
//! on ✓ takes the focus away from the field before it lands.

use std::borrow::Cow;

use freya::prelude::*;
use notin_i18n::tr;

use super::field::TextField;
use super::icon_button::IconButton;
use super::list_row::{ListRow, Trailing};
use super::text::{Text, TextRole};
use crate::tokens::size;

/// Whether the title of the enclosing [`super::DialogHeader`] is being
/// edited; the header hides its other buttons meanwhile.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct HeaderEditing(pub(super) State<bool>);

/// Where the value sits, which decides how it reads and how the field
/// is laid out.
#[derive(Clone, PartialEq, Debug)]
enum Look {
    /// The title of a dialog header: the field takes the title's place.
    Title,
    /// A list row: the row's label stays above the field, so what is
    /// being changed never disappears behind it.
    Row {
        label: Cow<'static, str>,
        icon: Option<Bytes>,
        hint: Option<Cow<'static, str>>,
    },
}

/// Text that can be changed in place. See the module docs for the
/// behaviour every instance shares.
#[derive(Clone, PartialEq)]
pub struct InlineEdit {
    value: Cow<'static, str>,
    look: Look,
    shown_as: Option<Cow<'static, str>>,
    placeholder: Option<Cow<'static, str>>,
    edit_label: Option<Cow<'static, str>>,
    max_len: Option<usize>,
    allow_empty: bool,
    enabled: bool,
    on_commit: EventHandler<String>,
}

impl std::fmt::Debug for InlineEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InlineEdit")
            .field("value", &self.value)
            .field("look", &self.look)
            .finish_non_exhaustive()
    }
}

impl InlineEdit {
    /// The title of a dialog, for [`super::DialogHeader`] or
    /// [`super::Sheet::title`]. `on_commit` gets the trimmed text.
    #[must_use]
    pub fn title(
        value: impl Into<Cow<'static, str>>,
        on_commit: impl Into<EventHandler<String>>,
    ) -> Self {
        Self::with_look(value.into(), Look::Title, on_commit.into())
    }

    /// A list row showing `value`, labelled `label` while it is edited.
    /// `on_commit` gets the trimmed text.
    #[must_use]
    pub fn row(
        label: impl Into<Cow<'static, str>>,
        value: impl Into<Cow<'static, str>>,
        on_commit: impl Into<EventHandler<String>>,
    ) -> Self {
        let look = Look::Row {
            label: label.into(),
            icon: None,
            hint: None,
        };
        Self::with_look(value.into(), look, on_commit.into())
    }

    const fn with_look(
        value: Cow<'static, str>,
        look: Look,
        on_commit: EventHandler<String>,
    ) -> Self {
        Self {
            value,
            look,
            shown_as: None,
            placeholder: None,
            edit_label: None,
            max_len: None,
            allow_empty: false,
            enabled: true,
            on_commit,
        }
    }

    /// What the row reads while not being edited, when that is not the
    /// value itself (a setting whose value goes in its hint).
    #[must_use]
    pub fn shown_as(mut self, text: impl Into<Cow<'static, str>>) -> Self {
        self.shown_as = Some(text.into());
        self
    }

    /// Shown when the value is empty, and in the empty field.
    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<Cow<'static, str>>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Accessible label of the pencil (default: "Edit").
    #[must_use]
    pub fn edit_label(mut self, label: impl Into<Cow<'static, str>>) -> Self {
        self.edit_label = Some(label.into());
        self
    }

    #[must_use]
    pub const fn max_len(mut self, max_len: usize) -> Self {
        self.max_len = Some(max_len);
        self
    }

    /// Keep an empty text instead of treating it as a cancel (a name
    /// that falls back to a default when cleared).
    #[must_use]
    pub const fn allow_empty(mut self) -> Self {
        self.allow_empty = true;
        self
    }

    /// Read only when `false`: no pencil, no tap.
    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Row icon (row look only).
    #[must_use]
    pub fn icon(mut self, icon: Bytes) -> Self {
        if let Look::Row { icon: slot, .. } = &mut self.look {
            *slot = Some(icon);
        }
        self
    }

    /// Line under the row, kept while editing (row look only).
    #[must_use]
    pub fn hint(mut self, hint: impl Into<Cow<'static, str>>) -> Self {
        if let Look::Row { hint: slot, .. } = &mut self.look {
            *slot = Some(hint.into());
        }
        self
    }
}

/// The edit in progress: whether it is open, what is typed, and the
/// header to tell.
#[derive(Clone, Copy)]
struct Editing {
    open: State<bool>,
    draft: State<String>,
    header: Option<HeaderEditing>,
}

impl Editing {
    fn set_open(mut self, open: bool) {
        self.open.set(open);
        if let Some(HeaderEditing(mut header)) = self.header {
            header.set(open);
        }
    }

    fn start(mut self, value: &str) {
        self.draft.set(value.to_owned());
        self.set_open(true);
    }
}

impl InlineEdit {
    /// Keep `typed`, unless it is empty (and may not be) or unchanged.
    fn commit(&self, editing: Editing, typed: &str) {
        let typed = typed.trim();
        if (!typed.is_empty() || self.allow_empty) && typed != self.value.trim() {
            self.on_commit.call(typed.to_owned());
        }
        editing.set_open(false);
    }

    fn pencil_label(&self) -> Cow<'static, str> {
        self.edit_label
            .clone()
            .unwrap_or_else(|| Cow::Owned(tr!("kit-edit")))
    }

    /// The field with ✓ and ✕, as wide as its slot.
    fn field(&self, editing: Editing) -> Element {
        let keep = self.clone();
        let keep_typed = self.clone();
        let mut field = TextField::new(editing.draft)
            .auto_focus(true)
            .on_submit(move |typed: String| keep_typed.commit(editing, &typed));
        if let Some(placeholder) = &self.placeholder {
            field = field.placeholder(placeholder.clone());
        }
        if let Some(max) = self.max_len {
            field = field.max_len(max);
        }
        rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Start)
            .content(Content::Flex)
            .spacing(4.0)
            // Escape cancels here; taken before the dialog's own Escape.
            .on_key_down(move |e: Event<KeyboardEventData>| {
                if e.key == Key::Named(NamedKey::Escape) {
                    e.prevent_default();
                    e.stop_propagation();
                    editing.set_open(false);
                }
            })
            .child(rect().width(Size::flex(1.0)).child(field))
            .child(
                IconButton::new(freya::icons::lucide::check(), tr!("kit-edit-keep"))
                    .size(size::BUTTON)
                    .active(true)
                    .on_press(move |_| {
                        let typed = editing.draft.peek().clone();
                        keep.commit(editing, &typed);
                    }),
            )
            .child(
                IconButton::new(freya::icons::lucide::x(), tr!("kit-edit-cancel"))
                    .size(size::BUTTON)
                    .on_press(move |_| editing.set_open(false)),
            )
            .into_element()
    }

    fn pencil(&self, editing: Editing, side: f32) -> IconButton {
        let value = self.value.clone();
        IconButton::new(freya::icons::lucide::pencil_line(), self.pencil_label())
            .size(side)
            .on_press(move |_| editing.start(&value))
    }

    fn shown(&self) -> Cow<'static, str> {
        if let Some(shown) = &self.shown_as {
            return shown.clone();
        }
        if self.value.is_empty() {
            self.placeholder.clone().unwrap_or_default()
        } else {
            self.value.clone()
        }
    }

    fn title_view(&self, editing: Editing) -> Element {
        if *editing.open.read() {
            return self.field(editing);
        }
        let side = if crate::width_class().is_compact() {
            size::BUTTON
        } else {
            size::BUTTON_SM
        };
        rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .content(Content::Flex)
            .spacing(4.0)
            .child(
                rect().width(Size::flex(1.0)).child(
                    Text::new(self.shown())
                        .role(TextRole::Title)
                        .lines(2)
                        .fill(),
                ),
            )
            .maybe_child(self.enabled.then(|| self.pencil(editing, side)))
            .into_element()
    }

    fn row_view(
        &self,
        editing: Editing,
        label: &str,
        icon: Option<&Bytes>,
        hint: Option<&Cow<'static, str>>,
    ) -> Element {
        if *editing.open.read() {
            return rect()
                .width(Size::fill())
                .padding((8.0, 12.0))
                .spacing(6.0)
                .child(Text::new(label.to_owned()).role(TextRole::Meta))
                .child(self.field(editing))
                .maybe_child(hint.map(|h| Text::new(h.clone()).role(TextRole::Hint)))
                .into_element();
        }
        let mut row = ListRow::new(self.shown());
        if let Some(icon) = icon {
            row = row.icon(icon.clone());
        }
        if let Some(hint) = hint {
            row = row.subtitle(hint.clone());
        }
        if !self.enabled {
            return row.into_element();
        }
        let value = self.value.clone();
        row.trailing(Trailing::Element(
            self.pencil(editing, size::BUTTON_SM).into_element(),
        ))
        .on_press(move |_| editing.start(&value))
        .into_element()
    }
}

impl Component for InlineEdit {
    #[cfg_attr(feature = "sismo", sismo::function("kit::inline_edit"))]
    fn render(&self) -> impl IntoElement {
        let header = match self.look {
            Look::Title => try_consume_context::<HeaderEditing>(),
            Look::Row { .. } => None,
        };
        let editing = Editing {
            open: use_state(|| false),
            draft: use_state(String::new),
            header,
        };
        match &self.look {
            Look::Title => self.title_view(editing),
            Look::Row { label, icon, hint } => {
                self.row_view(editing, label, icon.as_ref(), hint.as_ref())
            }
        }
    }
}
