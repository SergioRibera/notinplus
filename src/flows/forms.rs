//! The one form behind every create and edit sheet: name, paper,
//! colour, tags and location, plus the few fields one kind needs.

use std::rc::Rc;

use freya::prelude::*;
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

/// What Papelera does in an edit sheet.
#[derive(Clone)]
pub struct OnTrash(pub Rc<dyn Fn()>);

impl PartialEq for OnTrash {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for OnTrash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OnTrash")
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
    /// "Crear cuaderno al lado" (PDF import).
    pub notebook_toggle: bool,
    /// "Fijar en Home" (mesa edit).
    pub pin_toggle: bool,
    /// Translation choice (Bible edit): `(id, label)`.
    pub choices: Vec<(String, String)>,
    pub choice_label: String,
    /// File summary line (PDF / markdown import).
    pub file_line: Option<String>,
    /// Rendered preview of imported text.
    pub preview: Option<String>,
    pub placeholder: String,
    pub hint: String,
    pub submit: String,
    /// Edit sheets show Papelera on the left.
    pub trash: Option<OnTrash>,
    /// "Mover N documentos seleccionados adentro" (new folder).
    pub move_selected: Option<usize>,
}

/// What the form collected.
#[derive(Clone, Debug, PartialEq, Default)]
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

fn footer(
    spec: &FormSpec,
    values: State<FormValues>,
    submit: Rc<dyn Fn(FormValues)>,
) -> SheetFooter {
    let submit_label = spec.submit.clone();
    let base = SheetFooter::new([
        Button::new("Cancelar")
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
            Button::new("Papelera")
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

#[derive(Clone, PartialEq)]
struct Form {
    spec: FormSpec,
    values: State<FormValues>,
    submit: SubmitHandle,
}

impl Component for Form {
    fn render(&self) -> impl IntoElement {
        let spec = &self.spec;
        let mut values = self.values;
        let current = values.read().clone();
        let name = use_state(move || current.name.clone());
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
            .maybe_child(spec.file_line.clone().map(|line| {
                label()
                    .text(line)
                    .font_size(tokens::text::META)
                    .color(palette().t2)
            }))
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
                    .label("Nombre")
                    .placeholder(spec.placeholder.clone())
                    .auto_focus(true)
                    .on_submit(move |_| {
                        let collected = values.peek().clone();
                        ModalController::get().close();
                        (submit.0)(collected);
                    }),
            )
            .maybe_child(spec.paper.then(|| PaperPicker { values }))
            .maybe_child(spec.color.then(|| ColorRow {
                values,
                paper: spec.paper,
            }))
            .maybe_child((!spec.choices.is_empty()).then(|| ChoiceList {
                values,
                label: spec.choice_label.clone(),
                choices: spec.choices.clone(),
            }))
            .maybe_child(spec.tags.then(|| TagsField { values }))
            .maybe_child(spec.notebook_toggle.then(|| {
                Group::default().child(
                    ListRow::new("Crear cuaderno al lado")
                        .subtitle("Un lienzo vinculado para tus apuntes")
                        .icon(freya::icons::lucide::notebook_pen())
                        .trailing(Trailing::Switch {
                            on: current.notebook,
                            on_toggle: EventHandler::new(move |on| values.write().notebook = on),
                        }),
                )
            }))
            .maybe_child(spec.pin_toggle.then(|| {
                Group::default().child(
                    ListRow::new("Fijar en Home")
                        .icon(freya::icons::lucide::pin())
                        .trailing(Trailing::Switch {
                            on: current.pinned,
                            on_toggle: EventHandler::new(move |on| values.write().pinned = on),
                        }),
                )
            }))
            .maybe_child(spec.move_selected.map(|n| {
                Group::default().child(
                    ListRow::new(format!(
                        "Mover {} adentro",
                        crate::format::count(
                            n,
                            "documento seleccionado",
                            "documentos seleccionados"
                        )
                    ))
                    .icon(freya::icons::lucide::folder_input())
                    .trailing(Trailing::Switch {
                        on: current.move_selected,
                        on_toggle: EventHandler::new(move |on| values.write().move_selected = on),
                    }),
                )
            }))
            .maybe_child(spec.location.then(|| Location { values }))
    }
}

#[derive(Clone, Copy, PartialEq)]
struct PaperPicker {
    values: State<FormValues>,
}

impl Component for PaperPicker {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut values = self.values;
        let current = values.read().paper;
        let options = [
            (BackgroundStyle::Blank, "Blanco"),
            (BackgroundStyle::Line, "Líneas"),
            (BackgroundStyle::Grid, "Cuadrícula"),
            (BackgroundStyle::DotGrid, "Puntos"),
        ];
        let paper = values
            .read()
            .color
            .map_or(p.paper, |c| Color::from_argb(c[3], c[0], c[1], c[2]));
        Section::new("Papel").child(
            rect()
                .width(Size::fill())
                .horizontal()
                .content(Content::Flex)
                .spacing(10.0)
                .children(options.into_iter().map(|(style, label_text)| {
                    let selected = current == style;
                    rect()
                        .key(label_text)
                        .width(Size::flex(1.0))
                        .spacing(6.0)
                        .cross_align(Alignment::Center)
                        .a11y_role(AccessibilityRole::RadioButton)
                        .a11y_alt(label_text)
                        .on_press(move |_| values.write().paper = style)
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
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let mut values = self.values;
        let mut custom = use_state(|| false);
        let current = values.read().color;
        let swatches = if self.paper { COLORS } else { TINTS };
        let title = if self.paper {
            "Color del papel"
        } else {
            "Color de la tarjeta"
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
                            .on_press(move |_| values.write().color = Some(c))
                            .into_element()
                    }))
                    .child(
                        IconButton::new(freya::icons::lucide::palette(), "Otro color")
                            .size(32.0)
                            .active(custom())
                            .on_press(move |_| custom.toggle()),
                    ),
            )
            .maybe_child(custom().then(|| {
                ColorWheel::new()
                    .initial(current.map_or(p.paper, |c| Color::from_argb(c[3], c[0], c[1], c[2])))
                    .on_change(move |color: Color| {
                        values.write().color = Some([color.r(), color.g(), color.b(), 255])
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
        let mut add_typed = add.clone();
        Section::new("Etiquetas")
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
                    .placeholder("Escribí para buscar o crear")
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
                        let mut add = add.clone();
                        Tag::new(t)
                            .icon(freya::icons::lucide::plus())
                            .on_press(move |_| add(name.clone()))
                            .into_element()
                    }))
            }))
            .child(
                label()
                    .text("Enter agrega la etiqueta")
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
    fn render(&self) -> impl IntoElement {
        let values = self.values;
        let folder = values.read().folder;
        let data = LibraryView::get().data();
        let path = data
            .as_deref()
            .map_or_else(|| "Biblioteca".to_owned(), |d| folder_path(d, folder));
        Group::default().child(
            ListRow::new("Se guarda en")
                .subtitle(path)
                .icon(freya::icons::lucide::folder())
                .trailing(Trailing::Element(
                    Button::new("Cambiar")
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

/// "Biblioteca › Estudio › Romanos" for `folder`.
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
    names.push("Biblioteca".to_owned());
    names.reverse();
    names.join(" › ")
}

/// Turn tag names into ids, creating the missing ones.
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
pub fn edit_item(item: ItemId) {
    let Some(data) = LibraryView::get().data() else {
        return;
    };
    let Some(found) = data.item(item).cloned() else {
        return;
    };
    let info = crate::plugins::kind_info(&found.kind.plugin_id);
    let p = palette();
    let label = info
        .as_ref()
        .map_or_else(|| "documento".to_owned(), |k| k.label.to_lowercase());
    let is_canvas = found.kind.is(crate::plugins::builtin::canvas::KIND);
    let is_pdf = found.kind.is(crate::plugins::builtin::pdf::KIND);
    let is_bible = found.kind.is(notein_bible::kind::KIND);
    let mut spec = FormSpec {
        title: format!("Editar {label}"),
        kind_icon: info.as_ref().map(|k| k.icon.clone()),
        kind_color: info
            .as_ref()
            .map(|k| crate::references::kind_color(&p, k.color)),
        paper: is_canvas,
        color: !is_bible,
        tags: true,
        location: false,
        placeholder: "Sin título".to_owned(),
        submit: "Guardar".to_owned(),
        trash: Some(OnTrash(Rc::new(move || super::trash_item(item)))),
        ..FormSpec::default()
    };
    if is_pdf {
        spec.file_line = crate::plugins::content::read(item, crate::plugins::builtin::pdf::KIND)
            .and_then(|b| <crate::plugins::builtin::pdf::PdfContent as notein_plugin::payload::Payload>::from_bytes(&b).ok())
            .map(|c| format!("{} · {} · {}", c.file_name, crate::format::count(c.page_count as usize, "página", "páginas"), crate::format::size(c.size_bytes)));
    }
    let mut initial_choice = None;
    if is_bible && let Some(bible) = crate::plugins::builtin::bible_kind() {
        spec.choice_label = "Traducción".to_owned();
        spec.choices = bible
            .db()
            .list_bibles()
            .unwrap_or_default()
            .into_iter()
            .map(|b| (b.id.clone(), format!("{} · {}", b.abbrev, b.name_local)))
            .collect();
        initial_choice = crate::plugins::content::read(item, notein_bible::kind::KIND)
            .and_then(|b| {
                <notein_bible::BibleContent as notein_plugin::payload::Payload>::from_bytes(&b).ok()
            })
            .map(|c| c.bible_id);
    }
    let initial = FormValues {
        name: found.name.clone(),
        paper: found.background,
        color: found.color,
        tags: tag_names(&data, &found.tags),
        folder: found.folder,
        choice: initial_choice,
        ..FormValues::default()
    };
    open(spec, initial, move |values| {
        state::edit("guardar los cambios", async move |lib| {
            let tags = resolve_tags(lib, &values.tags).await;
            lib.rename_item(item, values.name.trim()).await?;
            lib.set_item_color(item, values.color).await?;
            lib.set_item_tags(item, tags).await?;
            if is_canvas {
                lib.set_item_background(item, values.paper).await?;
                if let Some(board) = crate::active_boards::get_item(item) {
                    let surface = values.color.map_or(crate::route::DEFAULT_PAPER, |c| {
                        freya_engine::prelude::SkColor::from_argb(c[3], c[0], c[1], c[2])
                    });
                    crate::route::apply_canvas_background(&board, values.paper, surface);
                }
            }
            if is_bible && let Some(choice) = values.choice.clone() {
                super::create::set_bible_translation(item, &choice);
            }
            Ok(())
        });
    });
}

/// The edit sheet of `folder`.
pub fn edit_folder(folder: FolderId) {
    let Some(data) = LibraryView::get().data() else {
        return;
    };
    let Some(found) = data.folder(folder).cloned() else {
        return;
    };
    let p = palette();
    let spec = FormSpec {
        title: "Editar carpeta".to_owned(),
        kind_icon: Some(freya::icons::lucide::folder()),
        kind_color: Some(p.kinds.folder),
        color: true,
        tags: true,
        placeholder: "Sin nombre".to_owned(),
        submit: "Guardar".to_owned(),
        trash: Some(OnTrash(Rc::new(move || super::trash_folder(folder)))),
        ..FormSpec::default()
    };
    let initial = FormValues {
        name: found.name.clone(),
        color: found.color,
        tags: tag_names(&data, &found.tags),
        ..FormValues::default()
    };
    open(spec, initial, move |values| {
        state::edit("guardar la carpeta", async move |lib| {
            let tags = resolve_tags(lib, &values.tags).await;
            lib.rename_folder(folder, values.name.trim()).await?;
            lib.set_folder_color(folder, values.color).await?;
            lib.set_folder_tags(folder, tags).await
        });
    });
}

/// The edit sheet of a mesa.
pub fn edit_mesa(workspace: WorkspaceId) {
    let Some(data) = LibraryView::get().data() else {
        return;
    };
    let Some(found) = data.workspace(workspace).cloned() else {
        return;
    };
    let p = palette();
    let spec = FormSpec {
        title: "Editar mesa".to_owned(),
        kind_icon: Some(freya::icons::lucide::layout_grid()),
        kind_color: Some(p.highlighters[2]),
        color: true,
        pin_toggle: true,
        placeholder: data.workspace_title(workspace),
        submit: "Guardar".to_owned(),
        trash: Some(OnTrash(Rc::new(move || super::delete_mesa(workspace)))),
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
        state::edit("guardar la mesa", async move |lib| {
            lib.rename_workspace(workspace, values.name.trim()).await?;
            lib.set_workspace_color(workspace, values.color).await?;
            lib.set_workspace_pinned(workspace, values.pinned).await
        });
    });
}
