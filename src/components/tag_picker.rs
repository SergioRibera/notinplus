//! Combined tag entry widget: `FormInput` for typing + suggestion
//! dropdown, plus a horizontal wrap of `Tag` chips for the current
//! selection. Adds on submit / suggestion click, removes on chip press.
//!
//! Purely name-based (`Vec<String>`). Persistence (mapping names to
//! `TagId`s, creating missing ones) is the caller's responsibility so
//! the picker stays library-agnostic and reusable across "create
//! folder", "create canvas", and "import pdf" flows.

use std::borrow::Cow;

use freya::prelude::*;

use crate::components::theme::TEXT_SECONDARY;
use crate::components::{FormInput, Tag};

#[derive(Clone, PartialEq)]
pub struct TagPicker {
    selected: Writable<Vec<String>>,
    available: Vec<String>,
    label: Option<Cow<'static, str>>,
    placeholder: Cow<'static, str>,
    on_change: Option<EventHandler<Vec<String>>>,
}

impl TagPicker {
    #[must_use]
    pub fn new(selected: impl Into<Writable<Vec<String>>>) -> Self {
        Self {
            selected: selected.into(),
            available: Vec::new(),
            label: None,
            placeholder: Cow::Borrowed("Escribí para buscar o crear"),
            on_change: None,
        }
    }

    #[must_use]
    pub fn available(mut self, available: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.available = available.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub fn label(mut self, label: impl Into<Cow<'static, str>>) -> Self {
        self.label = Some(label.into());
        self
    }

    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<Cow<'static, str>>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    #[must_use]
    pub fn on_change(mut self, cb: impl Into<EventHandler<Vec<String>>>) -> Self {
        self.on_change = Some(cb.into());
        self
    }
}

impl Component for TagPicker {
    fn render(&self) -> impl IntoElement {
        let selected_ref = self.selected.clone();
        let selected_snapshot: Vec<String> = selected_ref.read().clone();
        let on_change = self.on_change.clone();

        let mut search = use_state(String::new);

        // Available tags minus the ones already selected (the FormInput
        // dropdown filters again by substring — this pass just hides
        // names the user already picked).
        let already: Vec<String> = selected_snapshot.iter().map(|s| s.to_lowercase()).collect();
        let suggestions: Vec<Cow<'static, str>> = self
            .available
            .iter()
            .filter(|name| !already.contains(&name.to_lowercase()))
            .cloned()
            .map(Cow::Owned)
            .collect();

        let add_on_change = {
            let mut selected = self.selected.clone();
            let on_change = on_change.clone();
            move |raw: String| {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return;
                }
                let lc = trimmed.to_lowercase();
                let already = selected.read().iter().any(|s| s.to_lowercase() == lc);
                if !already {
                    selected.write().push(trimmed.to_owned());
                    if let Some(cb) = &on_change {
                        cb.call(selected.read().clone());
                    }
                }
                search.set(String::new());
            }
        };

        let chip_selected = self.selected.clone();
        let browse_selected = self.selected.clone();
        let browse_on_change = on_change.clone();
        // When the user hasn't typed anything, surface every existing
        // library tag as an outlined chip they can tap to add — makes
        // the "these come from your library" story obvious even before
        // typing triggers the suggestion dropdown.
        let search_empty = search.read().is_empty();
        let browse_names: Vec<String> = self
            .available
            .iter()
            .filter(|name| !already.contains(&name.to_lowercase()))
            .cloned()
            .collect();
        let show_browse = search_empty && !browse_names.is_empty();

        rect()
            .vertical()
            .width(Size::fill())
            .spacing(10.)
            .maybe_child(self.label.as_ref().map(|l| {
                label()
                    .font_size(14.)
                    .font_weight(FontWeight::MEDIUM)
                    .color(TEXT_SECONDARY)
                    .text(l.to_string())
            }))
            .child(
                FormInput::new(search)
                    .placeholder(self.placeholder.clone())
                    .width(Size::fill())
                    .suggestions(suggestions)
                    .on_change(add_on_change),
            )
            .maybe(show_browse, move |r| {
                r.child(browse_row(browse_names, browse_selected, browse_on_change))
            })
            .maybe(!selected_snapshot.is_empty(), move |r| {
                r.child(chip_wrap(selected_snapshot, chip_selected, on_change))
            })
    }
}

/// Row of tap-to-add chips for library tags not yet in the selection.
/// Only rendered when the user hasn't typed anything.
fn browse_row(
    names: Vec<String>,
    selected: Writable<Vec<String>>,
    on_change: Option<EventHandler<Vec<String>>>,
) -> impl IntoElement {
    rect()
        .horizontal()
        .width(Size::fill())
        .spacing(6.)
        .content(Content::wrap())
        .children(names.into_iter().enumerate().map(move |(i, name)| {
            let add_name = name.clone();
            let mut sel = selected.clone();
            let on_change = on_change.clone();
            Tag::new(name)
                .selectable(false)
                .on_press(move |_| {
                    if !sel.read().iter().any(|x| x == &add_name) {
                        sel.write().push(add_name.clone());
                        if let Some(cb) = &on_change {
                            cb.call(sel.read().clone());
                        }
                    }
                })
                .key(i)
                .into_element()
        }))
}

fn chip_wrap(
    names: Vec<String>,
    selected: Writable<Vec<String>>,
    on_change: Option<EventHandler<Vec<String>>>,
) -> impl IntoElement {
    rect()
        .horizontal()
        .width(Size::fill())
        .spacing(6.)
        .content(Content::wrap())
        .children(names.into_iter().enumerate().map(move |(i, name)| {
            let remove_name = name.clone();
            let mut sel = selected.clone();
            let on_change = on_change.clone();
            Tag::new(name)
                .closable()
                .on_press(move |_| {
                    sel.write().retain(|x| x != &remove_name);
                    if let Some(cb) = &on_change {
                        cb.call(sel.read().clone());
                    }
                })
                .key(i)
                .into_element()
        }))
}
