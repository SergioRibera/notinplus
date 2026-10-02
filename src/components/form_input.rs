//! Generic labeled input built on top of freya's `Input`.
//!
//! Features:
//!   * label (optional) + placeholder + left icon + error message.
//!   * `on_change` fires on submit, `on_validation` returns an
//!     inline error message.
//!   * `.suggestions([...])` renders a filtered dropdown under the
//!     field; picking one writes the value + fires `on_change`.
//!   * `.collapsible()` starts the field as a 36 px circle with the
//!     left icon; tapping expands it to the requested width.
//!   * Global Ctrl/Cmd + V handler that pastes clipboard text into the
//!     field even without OS-level focus routing.
//!
//! Password mode was intentionally dropped — no use case in the notes
//! app. Add it back if a future flow needs it.

use std::borrow::Cow;

use freya::{animation::*, prelude::*};

use crate::components::theme::{
    BORDER, BORDER_FOCUS, ERROR, SURFACE_PRIMARY, SURFACE_SECONDARY, SURFACE_TERTIARY,
    TEXT_PLACEHOLDER, TEXT_PRIMARY, TEXT_SECONDARY,
};

#[derive(Clone, PartialEq)]
pub struct FormInput {
    node_id: Option<AccessibilityId>,
    value: Writable<String>,
    label: Option<Cow<'static, str>>,
    label_size: f32,
    label_weight: FontWeight,
    placeholder: Option<Cow<'static, str>>,
    error_message: Option<String>,
    suggestions: Vec<Cow<'static, str>>,
    on_change: Option<EventHandler<String>>,
    on_validation: Option<Callback<String, Option<Cow<'static, str>>>>,
    left_icon: Option<Bytes>,
    padding: Gaps,
    width: Size,
    min_width: Size,
    corner_radius: Option<CornerRadius>,
    auto_focus: bool,
    collapsible: bool,
    flat: bool,
    background: Option<Color>,
}

impl std::fmt::Debug for FormInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FormInput")
            .field("value", &self.value.read_unchecked())
            .field("label", &self.label)
            .field("label_size", &self.label_size)
            .field("label_weight", &self.label_weight)
            .field("placeholder", &self.placeholder)
            .field("error_message", &self.error_message)
            .field("suggestions", &self.suggestions)
            .field("on_change", &self.on_change)
            .field("on_validation", &self.on_validation)
            .field("left_icon", &self.left_icon)
            .field("width", &self.width)
            .field("min_width", &self.min_width)
            .field("corner_radius", &self.corner_radius)
            .field("auto_focus", &self.auto_focus)
            .field("collapsible", &self.collapsible)
            .field("flat", &self.flat)
            .field("background", &self.background)
            .finish()
    }
}

impl FormInput {
    #[must_use]
    pub fn new(value: impl Into<Writable<String>>) -> Self {
        Self {
            node_id: None,
            value: value.into(),
            label: None,
            padding: (6., 12.).into(),
            label_size: 14.,
            label_weight: FontWeight::MEDIUM,
            placeholder: None,
            error_message: None,
            on_change: None,
            on_validation: None,
            left_icon: None,
            width: Size::Fill,
            min_width: Size::Fill,
            suggestions: vec![],
            background: None,
            corner_radius: None,
            auto_focus: false,
            collapsible: false,
            flat: false,
        }
    }

    #[must_use]
    pub fn a11y_id(mut self, id: AccessibilityId) -> Self {
        self.node_id = Some(id);
        self
    }

    #[must_use]
    pub const fn auto_focus(mut self, auto_focus: bool) -> Self {
        self.auto_focus = auto_focus;
        self
    }

    #[must_use]
    pub fn label(mut self, label: impl Into<Cow<'static, str>>) -> Self {
        self.label = Some(label.into());
        self
    }

    #[must_use]
    pub fn label_weight(mut self, weight: impl Into<FontWeight>) -> Self {
        self.label_weight = weight.into();
        self
    }

    #[must_use]
    pub fn label_size(mut self, size: impl Into<f32>) -> Self {
        self.label_size = size.into();
        self
    }

    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<Cow<'static, str>>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    #[must_use]
    pub fn error_message(mut self, error: Option<String>) -> Self {
        self.error_message = error;
        self
    }

    #[must_use]
    pub fn on_change(mut self, on_change: impl Into<EventHandler<String>>) -> Self {
        self.on_change = Some(on_change.into());
        self
    }

    #[must_use]
    pub fn on_validation(
        mut self,
        f: impl Into<Callback<String, Option<Cow<'static, str>>>>,
    ) -> Self {
        self.on_validation = Some(f.into());
        self
    }

    #[must_use]
    pub fn min_width(mut self, min_width: impl Into<Size>) -> Self {
        self.min_width = min_width.into();
        self
    }

    #[must_use]
    pub fn width(mut self, width: impl Into<Size>) -> Self {
        self.width = width.into();
        self
    }

    #[must_use]
    pub fn left_icon(mut self, icon: impl Into<Bytes>) -> Self {
        self.left_icon = Some(icon.into());
        self
    }

    #[must_use]
    pub const fn flat(mut self) -> Self {
        self.flat = true;
        self
    }

    #[must_use]
    pub const fn collapsible(mut self) -> Self {
        self.collapsible = true;
        self
    }

    #[must_use]
    pub const fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    #[must_use]
    pub fn suggestions(
        mut self,
        suggestions: impl IntoIterator<Item = impl Into<Cow<'static, str>>>,
    ) -> Self {
        self.suggestions = suggestions.into_iter().map(Into::into).collect();
        self
    }
}

impl InputLayoutThemePartialExt for FormInput {
    fn corner_radius(
        self,
        corner_radius: freya_components::theme_setter_param!(CornerRadius),
    ) -> Self {
        Self {
            corner_radius: Some(corner_radius.into()),
            ..self
        }
    }

    fn padding(self, padding: freya_components::theme_setter_param!(Gaps)) -> Self {
        Self {
            padding: padding.into(),
            ..self
        }
    }
}

impl Component for FormInput {
    fn render(&self) -> impl IntoElement {
        let left_icon = self.left_icon.clone();
        let on_change = self.on_change.clone();
        let validation_error = use_state::<Option<Cow<'static, str>>>(|| None);
        let active_error = validation_error
            .read()
            .clone()
            .or_else(|| self.error_message.as_ref().map(|e| Cow::Owned(e.clone())));
        let has_error = active_error.is_some();

        let focus = self.node_id.unwrap_or_else(use_a11y);
        let focus_status = use_focus(focus);
        let is_focused = focus_status().is_focused();

        let collapsible = self.collapsible;
        let mut is_expanded = use_state(|| false);

        let expanded_width_px = match &self.width {
            Size::Pixels(len) => len.get(),
            _ => 300.0,
        };

        let mut animation_content = use_animation(move |_| {
            AnimNum::new(0., 100.)
                .time(250)
                .function(Function::Expo)
                .ease(Ease::Out)
        });
        let mut animation_width = use_animation(move |_| {
            AnimNum::new(36., expanded_width_px)
                .time(250)
                .function(Function::Expo)
                .ease(Ease::Out)
        });

        let is_running = animation_width.is_running();
        let expand_percent = animation_content.get().value();
        let outer_width = animation_width.get().value();

        let mut was_focused = use_state(|| false);
        let mut has_been_focused = use_state(|| false);
        let value_watch = self.value.clone();
        let mut collapse_width = animation_width;
        let mut collapse_content = animation_content;
        use_side_effect(move || {
            let focused_now = focus_status().is_focused();
            let running = *is_running.read();
            let expanded = is_expanded();

            if focused_now && !*has_been_focused.peek() {
                has_been_focused.set(true);
            }

            if collapsible && expanded && !running && !focused_now && !*has_been_focused.peek() {
                focus.request_focus();
            }

            let was = *was_focused.peek();
            if collapsible
                && expanded
                && was
                && !focused_now
                && value_watch.peek().trim().is_empty()
            {
                collapse_content.reverse();
                collapse_width.reverse();
                is_expanded.set(false);
                has_been_focused.set(false);
                focus.request_unfocus();
            }

            if was != focused_now {
                was_focused.set(focused_now);
            }
        });

        let suggestions = self.suggestions.clone();
        let has_suggestions = !suggestions.is_empty();
        let mut show_suggestions = use_state(|| false);
        let mut current_value = use_state(String::new);

        let border_color = if has_error {
            ERROR
        } else if is_focused {
            BORDER_FOCUS
        } else {
            BORDER
        };
        let border_width = if is_focused { 2. } else { 1. };

        let mut input = Input::new(self.value.clone())
            .padding(Gaps::new_all(0.))
            .auto_focus(self.auto_focus)
            .a11y_id(focus)
            .width(Size::flex(1.))
            .flat();

        if let Some(placeholder) = &self.placeholder {
            input = input.placeholder(placeholder.clone());
        }

        if let Some(on_change) = on_change.as_ref() {
            input = input.on_submit(on_change.clone());
        }

        if let Some(validator) = &self.on_validation {
            let external_handler = validator.clone();
            let mut validation_error = validation_error.clone();
            if has_suggestions {
                input = input.on_validate(move |v: InputValidator| {
                    current_value.set(v.text().clone());
                    show_suggestions.set(!v.text().is_empty());
                    external_handler.call(v.text().clone());
                });
            } else {
                input = input.on_validate(move |e: InputValidator| {
                    let result = external_handler.call(e.text().clone());
                    validation_error.set(result);
                });
            }
        } else if has_suggestions {
            input = input.on_validate(move |v: InputValidator| {
                current_value.set(v.text().clone());
                show_suggestions.set(!v.text().is_empty());
            });
        }

        input = input.theme_colors(InputColorsThemePartial {
            border_fill: Some(Color::TRANSPARENT.into()),
            focus_border_fill: Some(Color::TRANSPARENT.into()),
            background: Some(Color::TRANSPARENT.into()),
            focus_background: Some(Color::TRANSPARENT.into()),
            color: Some(TEXT_PRIMARY.into()),
            placeholder_color: Some(TEXT_PLACEHOLDER.into()),
        });

        let mut paste_value = self.value.clone();
        let paste_validator = self.on_validation.clone();
        let mut paste_error = validation_error.clone();

        rect()
            .on_global_key_down(move |e: Event<KeyboardEventData>| {
                if !focus_status().is_focused() {
                    return;
                }
                if let Key::Character(ch) = &e.key {
                    let is_v = ch == "v" || ch == "V";
                    let with_mod = e.modifiers.contains(Modifiers::CONTROL)
                        || e.modifiers.contains(Modifiers::META);
                    if is_v && with_mod {
                        if let Ok(text) = Clipboard::get() {
                            *paste_value.write() = text.clone();
                            if let Some(validator) = paste_validator.as_ref() {
                                let result = validator.call(text.clone());
                                paste_error.set(result);
                            }
                            if has_suggestions {
                                current_value.set(text.clone());
                                show_suggestions.set(!text.is_empty());
                            }
                        }
                    }
                }
            })
            .width(if collapsible {
                Size::px(outer_width)
            } else {
                self.width.clone()
            })
            .min_width(self.min_width.clone())
            .spacing(6.)
            .maybe_child(
                (!collapsible)
                    .then(|| {
                        self.label.as_ref().map(|label_text| {
                            label()
                                .font_size(self.label_size)
                                .font_weight(self.label_weight)
                                .max_lines(1)
                                .color(TEXT_SECONDARY)
                                .text(label_text.to_string())
                        })
                    })
                    .flatten(),
            )
            .child(
                rect()
                    .horizontal()
                    .spacing(12.)
                    .cross_align(Alignment::center())
                    .width(Size::fill())
                    .content(Content::Flex)
                    .map(self.corner_radius, |r, corner| r.corner_radius(corner))
                    .maybe(self.corner_radius.is_none(), |r| r.rounded_lg())
                    .maybe(collapsible, |r| {
                        r.overflow(Overflow::Clip).height(Size::px(36.))
                    })
                    .background(self.background.unwrap_or(SURFACE_TERTIARY))
                    .maybe(!self.flat, |r| {
                        r.border(
                            Border::new()
                                .width(border_width)
                                .alignment(BorderAlignment::Inner)
                                .fill(border_color),
                        )
                    })
                    .padding(self.padding)
                    .maybe(collapsible && !is_expanded(), |r| {
                        r.on_press(move |_| {
                            is_expanded.set(true);
                            animation_content.start();
                            animation_width.start();
                        })
                    })
                    .maybe_child(left_icon.map(|i| {
                        SvgViewer::new(i)
                            .color(TEXT_PLACEHOLDER)
                            .width(Size::px(14.))
                            .height(Size::px(14.))
                    }))
                    .maybe_child((!collapsible || is_expanded()).then(|| {
                        rect()
                            .horizontal()
                            .cross_align(Alignment::center())
                            .width(Size::flex(1.))
                            .maybe(collapsible, |r| {
                                r.visible_width(VisibleSize::inner_percent(expand_percent))
                            })
                            .child(input)
                    })),
            )
            .maybe(
                has_suggestions && show_suggestions() && !current_value.read().is_empty(),
                move |r| {
                    let filtered: Vec<Cow<'static, str>> = suggestions
                        .iter()
                        .filter(|s| {
                            s.to_lowercase()
                                .contains(&current_value.read().to_lowercase())
                        })
                        .cloned()
                        .collect();

                    if filtered.is_empty() {
                        return r;
                    }

                    r.child(
                        rect()
                            .width(Size::fill())
                            .height(Size::px(0.))
                            .margin((-6., 0., 0., 0.))
                            .child(
                                rect()
                                    .layer(Layer::Overlay)
                                    .width(Size::fill())
                                    .position(Position::new_absolute().bottom(-204.))
                                    .height(Size::px(200.))
                                    .overflow(Overflow::Clip)
                                    .corner_radius(10.)
                                    .background(SURFACE_PRIMARY)
                                    .border(
                                        Border::new()
                                            .width(1.)
                                            .alignment(BorderAlignment::Inner)
                                            .fill(BORDER),
                                    )
                                    .child(
                                        ScrollView::new()
                                            .direction(Direction::Vertical)
                                            .show_scrollbar(false)
                                            .children(filtered.into_iter().enumerate().map(
                                                |(i, suggestion)| {
                                                    SuggestionItem {
                                                        on_change: on_change.clone(),
                                                        suggestion: suggestion.clone(),
                                                        value: self.value.clone(),
                                                        show_suggestions,
                                                        current_value,
                                                        key: DiffKey::U64(i as u64),
                                                    }
                                                    .into_element()
                                                },
                                            )),
                                    ),
                            ),
                    )
                },
            )
            .maybe_child(
                active_error
                    .map(|error| label().font_size(12.).color(ERROR).text(error.to_string())),
            )
    }
}

#[derive(Clone, PartialEq)]
struct SuggestionItem {
    value: Writable<String>,
    suggestion: Cow<'static, str>,
    show_suggestions: State<bool>,
    current_value: State<String>,
    on_change: Option<EventHandler<String>>,
    key: DiffKey,
}

impl KeyExt for SuggestionItem {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for SuggestionItem {
    fn render(&self) -> impl IntoElement {
        let mut is_hover = use_state(|| false);
        let suggestion = self.suggestion.clone();
        let on_change = self.on_change.clone();
        let mut value = self.value.clone();
        let mut show_suggestions = self.show_suggestions;
        let mut current_value = self.current_value;

        rect()
            .width(Size::fill())
            .padding((10., 16.))
            .background(if is_hover() {
                SURFACE_SECONDARY
            } else {
                Color::TRANSPARENT
            })
            .corner_radius(6.)
            .on_pointer_enter(move |_| is_hover.set(true))
            .on_pointer_leave(move |_| is_hover.set(false))
            .on_press(move |_| {
                *value.write() = suggestion.to_string();
                current_value.set(suggestion.to_string());
                if let Some(f) = on_change.as_ref() {
                    f.call(suggestion.to_string());
                }
                show_suggestions.set(false);
            })
            .child(
                label()
                    .font_size(14.)
                    .color(TEXT_PRIMARY)
                    .text(self.suggestion.to_string()),
            )
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}
