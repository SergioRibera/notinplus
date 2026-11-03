//! Table chrome beyond the typed cells: check marks, the totals row and
//! computed columns, see [`Table::evaluate`].

use std::rc::Rc;

use freya::icons::lucide;
use freya::prelude::*;

use super::Shared;
use super::header::{ChipMenu, HeaderChip};
use crate::document::{Aggregate, BlockId, Column, Expr, Scope, Summary, Table};
use crate::labels::Label;
use crate::popup::{Anchored, MenuItem, input_colors, popup_frame};
use crate::theme::EditorTheme;

/// Side of a check mark.
const MARK: f32 = 16.0;

/// The check mark of body row `row`, ticking it when pressed.
pub(crate) fn check_mark(shared: &Shared, block: BlockId, row: usize, checked: bool) -> Element {
    let theme = &shared.theme;
    let colors = &theme.colors;
    let controller = shared.controller;
    let editable = !shared.read_only;
    rect()
        .width(Size::px(theme.px(MARK)))
        .height(Size::px(theme.px(MARK)))
        .corner_radius(theme.px(4.))
        .center()
        .border(
            Border::new()
                .width(theme.px(1.5))
                .alignment(BorderAlignment::Inner)
                .fill(if checked { colors.accent } else { colors.muted }),
        )
        .background(if checked {
            colors.accent
        } else {
            Color::TRANSPARENT
        })
        .maybe(editable, |mark| {
            mark.cursor(CursorIcon::Pointer)
                .on_press(move |event: Event<PressEventData>| {
                    event.stop_propagation();
                    controller.table_toggle_check(block, row);
                })
        })
        .maybe_child(checked.then(|| {
            label()
                .font_size(theme.px(11.))
                .color(colors.surface)
                .font_weight(FontWeight::BOLD)
                .text("✓")
        }))
        .into_element()
}

/// What sits under a column: its total, or while the table is being
/// edited an invitation to pick one.
#[derive(Clone, PartialEq)]
pub(crate) struct TotalCell {
    pub(crate) shared: Shared,
    pub(crate) block: BlockId,
    pub(crate) column: Column,
    pub(crate) summary: Option<Summary>,
    /// The total as text, when there is a summary.
    pub(crate) text: Option<String>,
    /// Whether the column holds numbers.
    pub(crate) numeric: bool,
    /// Whether the table has a check column.
    pub(crate) checks: bool,
    /// Whether this is the check column itself.
    pub(crate) is_check_column: bool,
}

impl TotalCell {
    fn ops(&self) -> Vec<Aggregate> {
        if self.is_check_column {
            return vec![
                Aggregate::Checked,
                Aggregate::Unchecked,
                Aggregate::PercentChecked,
                Aggregate::CountAll,
            ];
        }
        let mut ops = vec![
            Aggregate::CountAll,
            Aggregate::CountFilled,
            Aggregate::CountEmpty,
        ];
        if self.numeric {
            ops.extend([
                Aggregate::Sum,
                Aggregate::Average,
                Aggregate::Median,
                Aggregate::Min,
                Aggregate::Max,
                Aggregate::Range,
            ]);
        }
        ops
    }

    fn menu(&self) -> ChipMenu {
        let theme = &self.shared.theme;
        let controller = self.shared.controller;
        let block = self.block;
        let column = self.column;
        let current = self.summary;
        let scope = current.map(|summary| summary.scope).unwrap_or_default();
        let ops: Vec<MenuItem> = self
            .ops()
            .into_iter()
            .map(|op| {
                MenuItem::new(theme.labels.aggregate(op), move || {
                    controller.table_set_summary(block, column, Some(Summary { op, scope }));
                })
                .selected(current.is_some_and(|summary| summary.op == op))
            })
            .collect();
        let mut groups = vec![ops];
        if self.checks && !self.is_check_column {
            let default_op = if self.numeric {
                Aggregate::Sum
            } else {
                Aggregate::CountAll
            };
            let scopes: Vec<MenuItem> = Scope::ALL
                .into_iter()
                .map(|scope| {
                    MenuItem::new(theme.labels.scope(scope), move || {
                        let op = current.map_or(default_op, |summary| summary.op);
                        controller.table_set_summary(block, column, Some(Summary { op, scope }));
                    })
                    .selected(current.is_some_and(|summary| summary.scope == scope))
                })
                .collect();
            groups.push(scopes);
        }
        if current.is_some() {
            groups.push(vec![
                MenuItem::new(theme.labels.get(Label::NoTotal), move || {
                    controller.table_set_summary(block, column, None);
                })
                .icon(lucide::x(), theme.colors.caution),
            ]);
        }
        ChipMenu::Items(groups)
    }
}

impl Component for TotalCell {
    fn render(&self) -> impl IntoElement {
        let theme = &self.shared.theme;
        let colors = &theme.colors;
        let text = match (&self.summary, &self.text) {
            (Some(summary), Some(text)) => {
                format!("{} {text}", theme.labels.aggregate(summary.op))
            }
            _ => theme.labels.get(Label::Calculate).to_owned(),
        };
        HeaderChip {
            icon: lucide::sigma(),
            text,
            color: if self.summary.is_some() {
                colors.text
            } else {
                colors.muted
            },
            theme: theme.clone(),
            editable: !self.shared.read_only,
            auto_open: false,
            menu: self.menu(),
        }
    }
}

/// Name and formula of a computed column, with a way to save or drop it.
#[derive(Clone, PartialEq)]
struct FormulaEditor {
    theme: Rc<EditorTheme>,
    header: String,
    formula: String,
    on_save: EventHandler<(String, String)>,
    on_remove: Option<EventHandler<()>>,
    on_done: EventHandler<()>,
}

impl Component for FormulaEditor {
    fn render(&self) -> impl IntoElement {
        let theme = &self.theme;
        let colors = &theme.colors;
        let header = self.header.clone();
        let formula = self.formula.clone();
        let name = use_state(move || header);
        let source = use_state(move || formula);
        let parsed = Expr::parse(source.read().trim());
        let valid = parsed.is_ok() && !name.read().trim().is_empty();
        let on_save = self.on_save.clone();
        let on_done = self.on_done.clone();
        let save = move || {
            if !valid {
                return;
            }
            on_save.call((
                name.peek().trim().to_owned(),
                source.peek().trim().to_owned(),
            ));
            on_done.call(());
        };
        let save_for_submit = save.clone();
        let button = |text: &str, color: Color, enabled: bool, on_press: Box<dyn Fn()>| {
            rect()
                .padding((5., 10.))
                .corner_radius(6.)
                .background(colors.popup_selected)
                .maybe(enabled, |button| {
                    button
                        .cursor(CursorIcon::Pointer)
                        .on_press(move |_| on_press())
                })
                .child(
                    label()
                        .font_size(12.)
                        .color(if enabled { color } else { colors.muted })
                        .text(text.to_owned()),
                )
        };
        let note = match (&parsed, source.read().trim().is_empty()) {
            (Err(_), false) => (theme.labels.get(Label::FormulaInvalid), colors.caution),
            _ => (theme.labels.get(Label::FormulaHint), colors.muted),
        };
        popup_frame(colors)
            .width(Size::px(300.))
            .spacing(8.)
            .child(
                Input::new(name)
                    .placeholder(theme.labels.get(Label::ColumnName).to_owned())
                    .theme_colors(input_colors(colors))
                    .auto_focus(true)
                    .width(Size::fill())
                    .on_submit(move |_| ()),
            )
            .child(
                Input::new(source)
                    .placeholder(theme.labels.get(Label::FormulaPlaceholder).to_owned())
                    .theme_colors(input_colors(colors))
                    .width(Size::fill())
                    .on_submit(move |_| save_for_submit()),
            )
            .child(
                label()
                    .font_size(11.5)
                    .color(note.1)
                    .text(note.0.to_owned()),
            )
            .child(
                rect()
                    .horizontal()
                    .spacing(6.)
                    .child(button(
                        theme.labels.get(Label::Save),
                        colors.accent,
                        valid,
                        Box::new(save),
                    ))
                    .maybe_child(self.on_remove.clone().map(|on_remove| {
                        let on_done = self.on_done.clone();
                        button(
                            theme.labels.get(Label::RemoveComputedColumn),
                            colors.caution,
                            true,
                            Box::new(move || {
                                on_remove.call(());
                                on_done.call(());
                            }),
                        )
                    })),
            )
            .on_press(|event: Event<PressEventData>| event.stop_propagation())
            // Keep the fields from reaching the editor: the inputs are not
            // part of the document.
            .on_key_down(|event: Event<KeyboardEventData>| event.stop_propagation())
            .into_element()
    }
}

/// Header cell of computed column `index`: its name, opening the formula
/// editor when pressed.
#[derive(Clone, PartialEq)]
pub(crate) struct ComputedHeader {
    pub(crate) shared: Shared,
    pub(crate) block: BlockId,
    pub(crate) index: usize,
    pub(crate) header: String,
    pub(crate) formula: String,
}

impl Component for ComputedHeader {
    fn render(&self) -> impl IntoElement {
        let mut open = use_state(|| false);
        let theme = &self.shared.theme;
        let colors = &theme.colors;
        let editable = !self.shared.read_only;
        let controller = self.shared.controller;
        let block = self.block;
        let index = self.index;
        let header = rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::flex())
            .spacing(theme.px(4.))
            .cross_align(Alignment::center())
            .maybe(editable, |header| {
                header
                    .cursor(CursorIcon::Pointer)
                    .on_press(move |event: Event<PressEventData>| {
                        event.stop_propagation();
                        open.toggle();
                    })
            })
            .child(
                label()
                    .width(Size::flex(1.))
                    .color(colors.text)
                    .font_size(theme.body_size)
                    .font_weight(FontWeight::BOLD)
                    .text_align(TextAlign::Right)
                    .text(self.header.clone()),
            )
            .maybe_child(editable.then(|| {
                SvgViewer::new(lucide::square_function())
                    .color(colors.muted)
                    .width(Size::px(theme.px(13.)))
                    .height(Size::px(theme.px(13.)))
            }))
            .into_element();
        let popup = (editable && open()).then(|| {
            FormulaEditor {
                theme: theme.clone(),
                header: self.header.clone(),
                formula: self.formula.clone(),
                on_save: EventHandler::new(move |(header, formula): (String, String)| {
                    controller.table_set_computed(block, index, &header, &formula);
                }),
                on_remove: Some(EventHandler::new(move |()| {
                    controller.table_remove_computed(block, index);
                })),
                on_done: EventHandler::new(move |()| open.set(false)),
            }
            .into_element()
        });
        Anchored::new(header, popup)
            .on_dismiss(move || open.set(false))
            .into_element()
    }
}

/// Tool button that adds a computed column to the table.
#[derive(Clone, PartialEq)]
pub(crate) struct AddComputedButton {
    pub(crate) shared: Shared,
    pub(crate) block: BlockId,
    pub(crate) suggested_header: String,
}

impl Component for AddComputedButton {
    fn render(&self) -> impl IntoElement {
        let mut open = use_state(|| false);
        let theme = &self.shared.theme;
        let colors = &theme.colors;
        let controller = self.shared.controller;
        let block = self.block;
        let button = rect()
            .padding((3., 8.))
            .corner_radius(6.)
            .background(if open() {
                colors.popup_selected
            } else {
                colors.code_background
            })
            .cursor(CursorIcon::Pointer)
            .on_press(move |event: Event<PressEventData>| {
                event.stop_propagation();
                open.toggle();
            })
            .child(
                label()
                    .font_size(11.5)
                    .color(colors.muted)
                    .text(theme.labels.get(Label::TableComputedColumn).to_owned()),
            )
            .into_element();
        let popup = open().then(|| {
            FormulaEditor {
                theme: theme.clone(),
                header: self.suggested_header.clone(),
                formula: String::new(),
                on_save: EventHandler::new(move |(header, formula): (String, String)| {
                    controller.table_add_computed(block, &header, &formula);
                }),
                on_remove: None,
                on_done: EventHandler::new(move |()| open.set(false)),
            }
            .into_element()
        });
        Anchored::new(button, popup)
            .on_dismiss(move || open.set(false))
            .into_element()
    }
}

/// Whether data column `column` of `table` holds numbers.
pub(crate) fn column_is_numeric(table: &Table, column: Column) -> bool {
    match column {
        Column::Data(index) => table.column_is_numeric(index),
        Column::Computed(_) => true,
    }
}
