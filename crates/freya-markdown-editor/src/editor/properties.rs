//! Front matter shown as a list of properties, the way people who never
//! wrote YAML expect metadata to look.

use std::rc::Rc;

use freya::icons::lucide;
use freya::prelude::*;

use crate::controller::EditorController;
use crate::document::{BlockId, Properties};
use crate::labels::Label;
use crate::popup::{Anchored, input_colors, popup_frame};
use crate::theme::EditorTheme;

const ROW: f32 = 34.0;

/// Header of a front matter block: name, count, a switch to the YAML and,
/// unfolded, one row per property.
#[derive(Clone, PartialEq)]
pub(crate) struct PropertiesPanel {
    pub(crate) block: BlockId,
    /// `None` when the YAML is too rich for a list.
    pub(crate) properties: Option<Properties>,
    pub(crate) controller: EditorController,
    pub(crate) theme: Rc<EditorTheme>,
    pub(crate) editable: bool,
    pub(crate) unfolded: State<bool>,
    pub(crate) as_text: State<bool>,
}

impl PropertiesPanel {
    fn header(&self) -> Element {
        let theme = &self.theme;
        let colors = &theme.colors;
        let mut unfolded = self.unfolded;
        let mut as_text = self.as_text;
        let listed = self.properties.is_some() && !as_text();
        let open = listed && unfolded();
        let count = self
            .properties
            .as_ref()
            .map(|properties| properties.entries().len());
        let switch = (self.editable && self.properties.is_some()).then(|| {
            let caption = if as_text() {
                Label::ShowAsList
            } else {
                Label::EditAsText
            };
            rect()
                .height(Size::px(theme.px(28.)))
                .padding((0., theme.px(8.)))
                .corner_radius(theme.px(6.))
                .center()
                .cursor(CursorIcon::Pointer)
                .on_press(move |event: Event<PressEventData>| {
                    event.stop_propagation();
                    as_text.toggle();
                })
                .child(
                    label()
                        .font_size(theme.px(12.))
                        .color(colors.accent)
                        .text(theme.labels.get(caption)),
                )
        });
        rect()
            .width(Size::fill())
            .height(Size::px(theme.px(32.)))
            .horizontal()
            .content(Content::flex())
            .cross_align(Alignment::center())
            .child(
                rect()
                    .width(Size::flex(1.))
                    .height(Size::fill())
                    .horizontal()
                    .spacing(theme.px(8.))
                    .cross_align(Alignment::center())
                    .maybe(listed, |row| {
                        row.cursor(CursorIcon::Pointer).on_press(
                            move |event: Event<PressEventData>| {
                                event.stop_propagation();
                                unfolded.toggle();
                            },
                        )
                    })
                    .child(
                        SvgViewer::new(lucide::sliders_horizontal())
                            .color(colors.muted)
                            .width(Size::px(theme.px(14.)))
                            .height(Size::px(theme.px(14.))),
                    )
                    .child(
                        label()
                            .font_size(theme.px(13.))
                            .font_weight(FontWeight::BOLD)
                            .color(colors.text)
                            .text(theme.labels.get(Label::Properties)),
                    )
                    .maybe_child(count.filter(|_| listed).map(|count| {
                        label()
                            .font_size(theme.px(12.))
                            .color(colors.muted)
                            .text(count.to_string())
                    }))
                    .maybe_child(listed.then(|| {
                        SvgViewer::new(if open {
                            lucide::chevron_down()
                        } else {
                            lucide::chevron_right()
                        })
                        .color(colors.muted)
                        .width(Size::px(theme.px(14.)))
                        .height(Size::px(theme.px(14.)))
                    })),
            )
            .maybe_child(switch)
            .into_element()
    }
}

impl Component for PropertiesPanel {
    fn render(&self) -> impl IntoElement {
        let theme = &self.theme;
        let listed = self.properties.as_ref().filter(|_| !(self.as_text)());
        let rows = listed.filter(|_| (self.unfolded)()).map(|properties| {
            let mut rows: Vec<Element> = properties
                .entries()
                .iter()
                .map(|(name, value)| {
                    PropertyRow {
                        block: self.block,
                        properties: properties.clone(),
                        name: Some(name.clone()),
                        value: value.clone(),
                        controller: self.controller,
                        theme: self.theme.clone(),
                        editable: self.editable,
                    }
                    .into_element()
                })
                .collect();
            if self.editable {
                rows.push(
                    PropertyRow {
                        block: self.block,
                        properties: properties.clone(),
                        name: None,
                        value: String::new(),
                        controller: self.controller,
                        theme: self.theme.clone(),
                        editable: true,
                    }
                    .into_element(),
                );
            }
            rect()
                .width(Size::fill())
                .margin((theme.px(2.), 0., theme.px(4.), 0.))
                .children(rows)
        });
        rect()
            .width(Size::fill())
            .child(self.header())
            .maybe_child(rows)
    }
}

/// One property, or with no `name` the row that adds one. Pressing it
/// opens a small form.
#[derive(Clone, PartialEq)]
struct PropertyRow {
    block: BlockId,
    properties: Properties,
    name: Option<String>,
    value: String,
    controller: EditorController,
    theme: Rc<EditorTheme>,
    editable: bool,
}

impl PropertyRow {
    fn form(&self, open: State<bool>, name: State<String>, value: State<String>) -> Element {
        let theme = &self.theme;
        let colors = &theme.colors;
        let (controller, block) = (self.controller, self.block);
        let adding = self.name.is_none();
        let save = {
            let properties = self.properties.clone();
            let original = self.name.clone();
            move || {
                let mut properties = properties.clone();
                let new_name = name.peek().trim().to_owned();
                if let Some(original) = original.as_deref().filter(|old| *old != new_name) {
                    properties.remove(original);
                }
                properties.set(&new_name, &value.peek());
                controller.set_front_matter(block, &properties.to_yaml());
                let mut open = open;
                open.set(false);
            }
        };
        let remove = self.name.clone().map(|original| {
            let properties = self.properties.clone();
            move || {
                let mut properties = properties.clone();
                properties.remove(&original);
                controller.set_front_matter(block, &properties.to_yaml());
                let mut open = open;
                open.set(false);
            }
        });
        let button = |text: &'static str, color: Color, action: Box<dyn Fn()>| {
            rect()
                .height(Size::px(32.))
                .padding((0., 12.))
                .corner_radius(6.)
                .center()
                .background(color.with_a(36))
                .cursor(CursorIcon::Pointer)
                .on_press(move |event: Event<PressEventData>| {
                    event.stop_propagation();
                    action();
                })
                .child(label().font_size(13.).color(color).text(text))
        };
        let field = |state: State<String>, placeholder: Label, focus: bool| {
            let submit = save.clone();
            Input::new(state)
                .placeholder(theme.labels.get(placeholder))
                .theme_colors(input_colors(colors))
                .auto_focus(focus)
                .width(Size::fill())
                .on_submit(move |_| submit())
        };
        let save_action = save.clone();
        popup_frame(colors)
            .padding(8.)
            .width(Size::px(280.))
            .spacing(8.)
            .child(field(name, Label::PropertyName, adding))
            .child(field(value, Label::PropertyValue, !adding))
            .child(
                rect()
                    .horizontal()
                    .spacing(8.)
                    .child(button(
                        theme
                            .labels
                            .get(if adding { Label::Add } else { Label::Save }),
                        colors.accent,
                        Box::new(save_action),
                    ))
                    .maybe_child(remove.map(|remove| {
                        button(
                            theme.labels.get(Label::RemoveProperty),
                            colors.caution,
                            Box::new(remove),
                        )
                    })),
            )
            .into_element()
    }
}

impl Component for PropertyRow {
    fn render(&self) -> impl IntoElement {
        let mut open = use_state(|| false);
        let name = use_state(String::new);
        let value = use_state(String::new);
        let theme = &self.theme;
        let colors = &theme.colors;
        let adding = self.name.is_none();
        let (start_name, start_value) = (self.name.clone().unwrap_or_default(), self.value.clone());
        let row = rect()
            .width(Size::fill())
            .height(Size::px(theme.px(ROW)))
            .padding((0., theme.px(6.)))
            .corner_radius(theme.px(6.))
            .horizontal()
            .content(Content::flex())
            .spacing(theme.px(10.))
            .cross_align(Alignment::center())
            .background(if open() {
                colors.popup_selected
            } else {
                Color::TRANSPARENT
            })
            .maybe(self.editable, |row| {
                let (mut name, mut value) = (name, value);
                row.cursor(CursorIcon::Pointer)
                    .on_press(move |event: Event<PressEventData>| {
                        event.stop_propagation();
                        name.set(start_name.clone());
                        value.set(start_value.clone());
                        open.toggle();
                    })
            })
            .map(self.name.clone(), |row, property| {
                row.child(
                    label()
                        .width(Size::flex(0.4))
                        .font_size(theme.px(13.))
                        .color(colors.muted)
                        .max_lines(1)
                        .text_overflow(TextOverflow::Ellipsis)
                        .text(property),
                )
                .child(
                    label()
                        .width(Size::flex(0.6))
                        .font_size(theme.px(13.))
                        .color(colors.text)
                        .max_lines(1)
                        .text_overflow(TextOverflow::Ellipsis)
                        .text(self.value.clone()),
                )
            })
            .maybe(adding, |row| {
                row.child(
                    SvgViewer::new(lucide::plus())
                        .color(colors.accent)
                        .width(Size::px(theme.px(14.)))
                        .height(Size::px(theme.px(14.))),
                )
                .child(
                    label()
                        .font_size(theme.px(13.))
                        .color(colors.accent)
                        .text(theme.labels.get(Label::AddProperty)),
                )
            })
            .into_element();
        let popup = open().then(|| self.form(open, name, value));
        Anchored::new(row, popup)
            .on_dismiss(move || open.set(false))
            .into_element()
    }
}
