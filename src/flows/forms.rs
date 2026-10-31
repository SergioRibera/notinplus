//! The one form behind every create and edit sheet: name, paper,
//! colour, tags and location, plus the few fields one kind needs.

use std::rc::Rc;

use freya::prelude::*;
use notein_i18n::tr;
use notein_ui_kit::TapExt;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Button, ButtonVariant, ColorWheel, Group, IconButton, ListRow, ModalController, Section, Sheet,
    SheetFooter, Tag, TextField, Trailing, tokens,
};

use crate::library::{BackgroundStyle, FolderId, ItemId, ROOT_FOLDER, Rgba, TagId, WorkspaceId};
use crate::state::{self, LibraryView};

/// Card and paper colours offered before the free picker.
pub const COLORS: [Rgba; 6] = [
    [247, 245, 240, 255],
    [228, 240, 232, 255],
    [232, 238, 252, 255],
    [252, 244, 224, 255],
    [252, 232, 236, 255],
    [42, 42, 47, 255],
];

/// Card colours of things without paper (notes, folders, mesas).
pub const TINTS: [Rgba; 6] = [
    [109, 184, 139, 255],
    [127, 196, 240, 255],
    [242, 213, 107, 255],
    [242, 155, 176, 255],
    [183, 156, 242, 255],
    [154, 149, 140, 255],
];

/// What a secondary button of a form does (Trash, Replace).
#[derive(Clone)]
pub struct FormAction(pub Rc<dyn Fn()>);

impl PartialEq for FormAction {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for FormAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FormAction")
    }
}

/// Which optional parts a form shows.
#[derive(Clone, Debug, PartialEq, Default)]
#[allow(clippy::struct_excessive_bools)] // independent sections of one form
pub struct FormSpec {
    pub title: String,
    pub kind_icon: Option<Bytes>,
    pub kind_color: Option<Color>,
    pub paper: bool,
    /// Paper swatches (canvas) or tint swatches (everything else).
    pub color: bool,
    pub tags: bool,
    pub location: bool,
    /// "Create a notebook beside it" (PDF import).
    pub notebook_toggle: bool,
    /// "Pin to Home" (mesa edit).
    pub pin_toggle: bool,
    /// Translation choice (Bible edit): `(id, label)`.
    pub choices: Vec<(String, String)>,
    pub choice_label: String,
    /// File summary line (PDF / markdown import).
    pub file_line: Option<String>,
    /// Button beside the file line, with its label (Replace).
    pub file_action: Option<(String, FormAction)>,
    /// Rendered preview of imported text.
    pub preview: Option<String>,
    pub placeholder: String,
    pub hint: String,
    pub submit: String,
    /// Edit sheets show Trash on the left.
    pub trash: Option<FormAction>,
    /// "Move N selected documents inside" (new folder).
    pub move_selected: Option<usize>,
}

/// What the form collected.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FormValues {
    pub name: String,
    pub paper: BackgroundStyle,
    pub color: Option<Rgba>,
    pub tags: Vec<String>,
    pub folder: FolderId,
    pub notebook: bool,
    pub pinned: bool,
    pub choice: Option<String>,
    pub move_selected: bool,
}

/// Open `spec` prefilled with `initial`; `on_submit` receives the
/// values when the primary button (or Enter in the name) is pressed.
#[sismo::function("flows::form_open")]
pub fn open(spec: FormSpec, initial: FormValues, on_submit: impl Fn(FormValues) + 'static) {
    let values = State::create_in_scope(initial, ScopeId::ROOT);
    let submit: Rc<dyn Fn(FormValues)> = Rc::new(on_submit);
    let mut sheet = Sheet::titled(spec.title.clone());
    if let (Some(icon), Some(color)) = (spec.kind_icon.clone(), spec.kind_color) {
        sheet = sheet.kind(icon, color);
    }
    let footer_spec = spec.clone();
    let footer_submit = Rc::clone(&submit);
    sheet
        .child(Form {
            spec,
            values,
            submit: SubmitHandle(Rc::clone(&submit)),
        })
        .footer(footer(&footer_spec, values, footer_submit))
        .open();
}

#[derive(Clone)]
struct SubmitHandle(Rc<dyn Fn(FormValues)>);

impl PartialEq for SubmitHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

#[sismo::function("flows::footer")]
fn footer(
    spec: &FormSpec,
    values: State<FormValues>,
    submit: Rc<dyn Fn(FormValues)>,
) -> SheetFooter {
    let submit_label = spec.submit.clone();
    let base = SheetFooter::new([
        Button::new(tr!("common-cancel"))
            .ghost()
            .on_press(|_| ModalController::get().close())
            .into_element(),
        Button::new(submit_label)
            .primary()
            .on_press(move |_| {
                let collected = values.peek().clone();
                ModalController::get().close();
                submit(collected);
            })
            .into_element(),
    ])
    .hint(spec.hint.clone());
    match spec.trash.clone() {
        Some(trash) => base.leading(
            Button::new(tr!("common-trash"))
                .variant(ButtonVariant::Danger)
                .icon(freya::icons::lucide::trash_2())
                .on_press(move |_| {
                    ModalController::get().close();
                    (trash.0)();
                }),
        ),
        None => base,
    }
}

impl FormSpec {
    /// The file summary, with its button (Replace) when there is one.
    fn file_line(&self) -> Option<Rect> {
        let line = self.file_line.clone()?;
        Some(
            rect()
                .width(Size::fill())
                .horizontal()
                .content(Content::Flex)
                .cross_align(Alignment::Center)
                .spacing(8.0)
                .child(
                    label()
                        .width(Size::flex(1.0))
                        .text(line)
                        .font_size(tokens::text::META)
                        .color(palette().t2),
                )
                .maybe_child(self.file_action.clone().map(|(text, action)| {
                    Button::new(text).ghost().small().on_press(move |_| {
                        ModalController::get().close();
                        (action.0)();
                    })
                })),
        )
    }
}

#[derive(Clone, PartialEq)]
struct Form {
    spec: FormSpec,
    values: State<FormValues>,
    submit: SubmitHandle,
}

impl Component for Form {
    #[sismo::function("flows::form")]
    fn render(&self) -> impl IntoElement {
        let spec = &self.spec;
        let mut values = self.values;
        let current = values.read().clone();
        let name = use_state(move || current.name);
        use_side_effect(move || {
            let typed = name.read().clone();
            if values.peek().name != typed {
                values.write().name = typed;
            }
        });
        let submit = self.submit.clone();
        rect()
            .width(Size::fill())
            .spacing(18.0)
            .maybe_child(spec.file_line())
            .maybe_child(spec.preview.clone().map(|preview| {
                rect()
                    .width(Size::fill())
                    .max_height(Size::px(140.0))
                    .padding(12.0)
                    .corner_radius(tokens::radius::MD)
                    .background(palette().s1)
                    .overflow(Overflow::Clip)
                    .child(
                        label()
                            .text(preview)
                            .font_size(tokens::text::META)
                            .color(palette().t2)
                            .line_height(1.5),
                    )
            }))
            .child(
                TextField::new(name)
                    .label(tr!("common-name"))
                    .placeholder(spec.placeholder.clone())
                    .auto_focus(true)
                    .on_submit(move |_| {
                        let collected = values.peek().clone();
                        ModalController::get().close();
                        (submit.0)(collected);
                    }),
            )
            .maybe_child(spec.paper.then_some(PaperPicker { values }))
            .maybe_child(spec.color.then_some(ColorRow {
                values,
                paper: spec.paper,
            }))
            .maybe_child((!spec.choices.is_empty()).then(|| ChoiceList {
                values,
                label: spec.choice_label.clone(),
                choices: spec.choices.clone(),
            }))
            .maybe_child(spec.tags.then_some(TagsField { values }))
            .maybe_child(spec.notebook_toggle.then(|| {
                Group::default().child(
                    ListRow::new(tr!("flows-forms-notebook-beside"))
                        .subtitle(tr!("flows-forms-notebook-beside-hint"))
                        .icon(freya::icons::lucide::notebook_pen())
                        .trailing(Trailing::Switch {
                            on: current.notebook,
                            on_toggle: EventHandler::new(move |on| values.write().notebook = on),
                        }),
                )
            }))
            .maybe_child(spec.pin_toggle.then(|| {
                Group::default().child(
                    ListRow::new(tr!("flows-forms-pin-home"))
                        .icon(freya::icons::lucide::pin())
                        .trailing(Trailing::Switch {
                            on: current.pinned,
                            on_toggle: EventHandler::new(move |on| values.write().pinned = on),
                        }),
                )
            }))
            .maybe_child(spec.move_selected.map(|n| {
                Group::default().child(
                    ListRow::new(tr!("flows-forms-move-selected", "count" => n))
                        .icon(freya::icons::lucide::folder_input())
                        .trailing(Trailing::Switch {
                            on: current.move_selected,
                            on_toggle: EventHandler::new(move |on| {
                                values.write().move_selected = on;
                            }),
                        }),
                )
            }))
            .maybe_child(spec.location.then_some(Location { values }))
    }
}

#[derive(Clone, Copy, PartialEq)]
struct PaperPicker {
    values: State<FormValues>,
}

impl Component for PaperPicker {
    #[sismo::function("flows::paper_picker")]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut values = self.values;
        let current = values.read().paper;
        let options = [
            (BackgroundStyle::Blank, tr!("flows-forms-paper-blank")),
            (BackgroundStyle::Line, tr!("flows-forms-paper-lines")),
            (BackgroundStyle::Grid, tr!("flows-forms-paper-grid")),
            (BackgroundStyle::DotGrid, tr!("flows-forms-paper-dots")),
        ];
        let paper = values
            .read()
            .color
            .map_or(p.paper, |c| Color::from_argb(c[3], c[0], c[1], c[2]));
        Section::new(tr!("flows-forms-paper")).child(
            rect()
                .width(Size::fill())
                .horizontal()
                .content(Content::Flex)
                .spacing(10.0)
                .children(options.into_iter().map(|(style, label_text)| {
                    let selected = current == style;
                    rect()
                        .key(format!("{style:?}"))
                        .width(Size::flex(1.0))
                        .spacing(6.0)
                        .cross_align(Alignment::Center)
                        .a11y_role(AccessibilityRole::RadioButton)
                        .a11y_alt(label_text.clone())
                        .on_tap(move |_| values.write().paper = style)
                        .child(
                            rect()
                                .width(Size::fill())
                                .height(Size::px(64.0))
                                .corner_radius(tokens::radius::MD)
                                .background(paper)
                                .overflow(Overflow::Clip)
                                .border(
                                    Border::new()
                                        .fill(if selected { p.accent } else { p.line })
                                        .width(if selected { 2.0 } else { 1.0 })
                                        .alignment(BorderAlignment::Inner),
                                )
                                .child(super::create::paper_swatch(style)),
                        )
                        .child(
                            label()
                                .text(label_text)
                                .font_size(tokens::text::LABEL)
                                .font_weight(if selected {
                                    FontWeight::SEMI_BOLD
                                } else {
                                    FontWeight::NORMAL
                                })
                                .color(if selected { p.t1 } else { p.t2 }),
                        )
                        .into_element()
                })),
        )
    }
}

#[derive(Clone, Copy, PartialEq)]
struct ColorRow {
    values: State<FormValues>,
    paper: bool,
}

impl Component for ColorRow {
    #[sismo::function("flows::color_row")]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut values = self.values;
        let mut custom = use_state(|| false);
        let current = values.read().color;
        let swatches = if self.paper { COLORS } else { TINTS };
        let title = if self.paper {
            tr!("flows-forms-paper-color")
        } else {
            tr!("flows-forms-card-color")
        };
        Section::new(title)
            .child(
                rect()
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(10.0)
                    .children(swatches.into_iter().map(|c| {
                        let selected = current == Some(c);
                        rect()
                            .key(format!("{c:?}"))
                            .a11y_role(AccessibilityRole::RadioButton)
                            .width(Size::px(30.0))
                            .height(Size::px(30.0))
                            .corner_radius(tokens::radius::FULL)
                            .background(Color::from_argb(c[3], c[0], c[1], c[2]))
                            .border(
                                Border::new()
                                    .fill(if selected { p.accent } else { p.line_strong })
                                    .width(if selected { 2.5 } else { 1.0 })
                                    .alignment(BorderAlignment::Outer),
                            )
                            .on_tap(move |_| values.write().color = Some(c))
                            .into_element()
                    }))
                    .child(
                        IconButton::new(
                            freya::icons::lucide::palette(),
                            tr!("flows-forms-other-color"),
                        )
                        .size(32.0)
                        .active(custom())
                        .on_press(move |_| custom.toggle()),
                    ),
            )
            .maybe_child(custom().then(|| {
                ColorWheel::new()
                    .initial(current.map_or(p.paper, |c| Color::from_argb(c[3], c[0], c[1], c[2])))
                    .on_change(move |color: Color| {
                        values.write().color = Some([color.r(), color.g(), color.b(), 255]);
                    })
            }))
    }
}

#[derive(Clone, PartialEq)]
struct ChoiceList {
    values: State<FormValues>,
    label: String,
    choices: Vec<(String, String)>,
}

impl Component for ChoiceList {
    #[sismo::function("flows::choice_list")]
    fn render(&self) -> impl IntoElement {
        let mut values = self.values;
        let current = values.read().choice.clone();
        Section::new(self.label.clone()).child(Group::default().children(self.choices.iter().map(
            |(id, text)| {
                let id = id.clone();
                let selected = current.as_deref() == Some(id.as_str());
                ListRow::new(text.clone())
                    .leading(notein_ui_kit::Leading::Check(selected))
                    .selected(selected)
                    .on_press(move |_| values.write().choice = Some(id.clone()))
                    .into_element()
            },
        )))
    }
}

#[derive(Clone, Copy, PartialEq)]
struct TagsField {
    values: State<FormValues>,
}

impl Component for TagsField {
    #[sismo::function("flows::tags_field")]
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut values = self.values;
        let typed = use_state(String::new);
        let current = values.read().tags.clone();
        let known: Vec<String> = LibraryView::get()
            .data()
            .map(|d| d.tags().iter().map(|t| t.name.clone()).collect())
            .unwrap_or_default();
        let query = typed.read().trim().to_lowercase();
        let suggestions: Vec<String> = known
            .into_iter()
            .filter(|t| {
                !current.contains(t) && (query.is_empty() || t.to_lowercase().contains(&query))
            })
            .take(8)
            .collect();
        let add = move |name: String| {
            let name = name.trim().to_owned();
            if !name.is_empty() && !values.peek().tags.contains(&name) {
                values.write().tags.push(name);
            }
            let mut typed = typed;
            typed.set(String::new());
        };
        let mut add_typed = add;
        Section::new(tr!("flows-forms-tags"))
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::wrap_spacing(6.0))
                    .spacing(6.0)
                    .children(current.iter().map(|t| {
                        let name = t.clone();
                        Tag::new(t.clone())
                            .active(true)
                            .on_close(move |()| values.write().tags.retain(|x| x != &name))
                            .into_element()
                    })),
            )
            .child(
                TextField::new(typed)
                    .placeholder(tr!("flows-forms-tags-placeholder"))
                    .on_submit(move |text: String| add_typed(text)),
            )
            .maybe_child((!suggestions.is_empty()).then(|| {
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::wrap_spacing(6.0))
                    .spacing(6.0)
                    .children(suggestions.into_iter().map(|t| {
                        let name = t.clone();
                        let mut add = add;
                        Tag::new(t)
                            .icon(freya::icons::lucide::plus())
                            .on_press(move |_| add(name.clone()))
                            .into_element()
                    }))
            }))
            .child(
                label()
                    .text(tr!("flows-forms-tags-hint"))
                    .font_size(tokens::text::CAPTION)
                    .color(p.t3),
            )
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Location {
    values: State<FormValues>,
}

impl Component for Location {
    #[sismo::function("flows::location")]
    fn render(&self) -> impl IntoElement {
        let values = self.values;
        let folder = values.read().folder;
        let data = LibraryView::get().data();
        let path = data
            .as_deref()
            .map_or_else(|| tr!("common-library"), |d| folder_path(d, folder));
        Group::default().child(
            ListRow::new(tr!("flows-forms-saved-in"))
                .subtitle(path)
                .icon(freya::icons::lucide::folder())
                .trailing(Trailing::Element(
                    Button::new(tr!("flows-forms-change"))
                        .ghost()
                        .small()
                        .on_press(move |_| {
                            super::pickers::choose_folder(folder, move |chosen| {
                                let mut values = values;
                                values.write().folder = chosen;
                            });
                        })
                        .into_element(),
                )),
        )
    }
}

/// "Library › Study › Romans" for `folder`.
#[must_use]
pub fn folder_path(data: &crate::library::LibraryData, folder: FolderId) -> String {
    let mut names = Vec::new();
    let mut cursor = folder;
    for _ in 0..32 {
        if cursor == ROOT_FOLDER {
            break;
        }
        let Some(f) = data.folder(cursor) else {
            break;
        };
        names.push(f.name.clone());
        cursor = f.parent;
    }
    names.push(tr!("common-library"));
    names.reverse();
    names.join(" › ")
}

/// Turn tag names into ids, creating the missing ones.
#[sismo::function("flows::resolve_tags")]
pub async fn resolve_tags(lib: &mut crate::library::Library, names: &[String]) -> Vec<TagId> {
    let mut ids = Vec::new();
    for name in names {
        let existing = lib
            .tags()
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .map(|t| t.id);
        let id = match existing {
            Some(id) => Some(id),
            None => lib
                .create_tag(name, {
                    let c = notein_ui_kit::auto_color(name);
                    [c.r(), c.g(), c.b(), c.a()]
                })
                .await
                .ok(),
        };
        if let Some(id) = id {
            ids.push(id);
        }
    }
    ids
}

fn tag_names(data: &crate::library::LibraryData, ids: &[TagId]) -> Vec<String> {
    ids.iter()
        .filter_map(|id| data.tag(*id).map(|t| t.name.clone()))
        .collect()
}

/// The edit sheet of `item`, shaped by its kind.
#[sismo::function("flows::edit_item_form")]
pub fn edit_item(item: ItemId) {
    let Some(data) = LibraryView::get().data() else {
        return;
    };
    let Some(found) = data.item(item).cloned() else {
        return;
    };
    let kind = crate::plugins::kind(&found.kind.plugin_id);
    let info = kind.as_ref().map(|k| k.info());
    let p = palette();
    let label = info
        .as_ref()
        .map_or_else(|| tr!("flows-forms-document"), |k| k.label.to_lowercase());
    let has_paper = info.as_ref().is_some_and(|k| k.has_paper);
    let content = kind.as_ref().map(|k| {
        crate::plugins::content::read(item, &found.kind.plugin_id)
            .unwrap_or_else(|| k.empty_content())
    });
    let mut spec = FormSpec {
        title: tr!("flows-forms-edit-kind", "kind" => label),
        kind_icon: info.as_ref().map(|k| k.icon.clone()),
        kind_color: info
            .as_ref()
            .map(|k| crate::references::kind_color(&p, k.color)),
        paper: has_paper,
        color: info.as_ref().is_none_or(|k| k.tint),
        tags: true,
        location: false,
        placeholder: tr!("common-untitled"),
        submit: tr!("common-save"),
        trash: Some(FormAction(Rc::new(move || super::trash_item(item)))),
        ..FormSpec::default()
    };
    if info.as_ref().is_some_and(|k| k.attachment.is_some()) {
        let kind_id = found.kind.plugin_id.clone();
        spec.file_action = Some((
            tr!("flows-forms-replace"),
            FormAction(Rc::new(move || {
                super::create::replace_attachment(item, kind_id.clone());
            })),
        ));
    }
    let choice = kind
        .as_ref()
        .zip(content.as_deref())
        .and_then(|(kind, content)| {
            spec.file_line = kind.details(content);
            kind.choice(content)
        });
    let initial_choice = choice.and_then(|choice| {
        spec.choice_label = choice.label.into_owned();
        spec.choices = choice
            .options
            .into_iter()
            .map(|option| (option.id.into_owned(), option.label.into_owned()))
            .collect();
        choice.current
    });
    let initial = FormValues {
        name: found.name.clone(),
        paper: found.background,
        color: found.color,
        tags: tag_names(&data, &found.tags),
        folder: found.folder,
        choice: initial_choice.clone(),
        ..FormValues::default()
    };
    let kind_id = found.kind.plugin_id.clone();
    open(spec, initial, move |values| {
        let kind_id = kind_id.clone();
        let picked = values
            .choice
            .clone()
            .filter(|choice| Some(choice) != initial_choice.as_ref());
        state::edit(crate::state::LibraryEdit::SaveChanges, async move |lib| {
            let tags = resolve_tags(lib, &values.tags).await;
            lib.rename_item(item, values.name.trim()).await?;
            lib.set_item_color(item, values.color).await?;
            lib.set_item_tags(item, tags).await?;
            if has_paper {
                lib.set_item_background(item, values.paper).await?;
                if let Some(board) = crate::active_boards::get_item(item) {
                    let surface = values.color.map_or(crate::route::DEFAULT_PAPER, |c| {
                        freya_engine::prelude::SkColor::from_argb(c[3], c[0], c[1], c[2])
                    });
                    crate::route::apply_canvas_background(&board, values.paper, surface);
                }
            }
            if let Some(choice) = picked {
                super::create::apply_choice(item, &kind_id, &choice);
            }
            Ok(())
        });
    });
}

/// The edit sheet of `folder`.
#[sismo::function("flows::edit_folder_form")]
pub fn edit_folder(folder: FolderId) {
    let Some(data) = LibraryView::get().data() else {
        return;
    };
    let Some(found) = data.folder(folder).cloned() else {
        return;
    };
    let p = palette();
    let spec = FormSpec {
        title: tr!("flows-forms-edit-folder"),
        kind_icon: Some(freya::icons::lucide::folder()),
        kind_color: Some(p.kinds.folder),
        color: true,
        tags: true,
        placeholder: tr!("flows-create-unnamed"),
        submit: tr!("common-save"),
        trash: Some(FormAction(Rc::new(move || super::trash_folder(folder)))),
        ..FormSpec::default()
    };
    let initial = FormValues {
        name: found.name.clone(),
        color: found.color,
        tags: tag_names(&data, &found.tags),
        ..FormValues::default()
    };
    open(spec, initial, move |values| {
        state::edit(crate::state::LibraryEdit::SaveFolder, async move |lib| {
            let tags = resolve_tags(lib, &values.tags).await;
            lib.rename_folder(folder, values.name.trim()).await?;
            lib.set_folder_color(folder, values.color).await?;
            lib.set_folder_tags(folder, tags).await
        });
    });
}

/// The edit sheet of a mesa.
#[sismo::function("flows::edit_mesa_form")]
pub fn edit_mesa(workspace: WorkspaceId) {
    let Some(data) = LibraryView::get().data() else {
        return;
    };
    let Some(found) = data.workspace(workspace).cloned() else {
        return;
    };
    let p = palette();
    let spec = FormSpec {
        title: tr!("flows-forms-edit-desk"),
        kind_icon: Some(freya::icons::lucide::layout_grid()),
        kind_color: Some(p.highlighters[2]),
        color: true,
        pin_toggle: true,
        placeholder: data.workspace_title(workspace),
        submit: tr!("common-save"),
        trash: Some(FormAction(Rc::new(move || super::delete_mesa(workspace)))),
        ..FormSpec::default()
    };
    let initial = FormValues {
        name: if found.named {
            found.name.clone()
        } else {
            String::new()
        },
        color: found.color,
        pinned: found.pinned,
        ..FormValues::default()
    };
    open(spec, initial, move |values| {
        state::edit(crate::state::LibraryEdit::SaveDesk, async move |lib| {
            lib.rename_workspace(workspace, values.name.trim()).await?;
            lib.set_workspace_color(workspace, values.color).await?;
            lib.set_workspace_pinned(workspace, values.pinned).await
        });
    });
}
