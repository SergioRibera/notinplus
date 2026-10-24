//! Ctrl+K: documents, places inside them and what can be done with the
//! focused document, from one field. "/" narrows to one kind.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use freya::prelude::*;
use notein_plugin::services::Placement;
use notein_ui_kit::theme::palette as theme;
use notein_ui_kit::{
    Kbd, ListRow, Modal, ModalController, SearchField, Section, Text, TextRole, Trailing, tokens,
    width_class,
};

use super::{Focus, MesaUi, Ops, PanelTab};
use crate::library::{ItemId, LibraryData, SplitOrientation};
use crate::references::kind_color;
use crate::search::{PlaceMatch, Results};

const DEBOUNCE: Duration = Duration::from_millis(120);
const WIDTH: f32 = 640.0;

/// Something the palette can do.
#[derive(Clone)]
struct Command {
    label: String,
    shortcut: Option<&'static str>,
    icon: Bytes,
    run: Rc<dyn Fn()>,
}

impl PartialEq for Command {
    fn eq(&self, other: &Self) -> bool {
        self.label == other.label
    }
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Command")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Line {
    Doc(ItemId),
    Place(PlaceMatch),
    Command(Command),
}

impl Line {
    fn run(&self, beside: bool) {
        ModalController::get().close();
        match self {
            Self::Doc(item) if beside => crate::flows::open_beside(*item),
            Self::Doc(item) => crate::flows::open(*item),
            Self::Place(place) => crate::plugins::navigate::open(
                &place.target,
                if beside {
                    Placement::Beside
                } else {
                    Placement::Here
                },
            ),
            Self::Command(command) => (command.run)(),
        }
    }
}

/// The commands for the focused document and the mesa.
fn commands(data: &LibraryData, ops: Ops, ui: MesaUi, focus: Option<&Focus>) -> Vec<Command> {
    use freya::icons::lucide;
    let mut out = Vec::new();
    let mut push = |label: &str, shortcut: Option<&'static str>, icon: Bytes, run: Rc<dyn Fn()>| {
        out.push(Command {
            label: label.to_owned(),
            shortcut,
            icon,
            run,
        });
    };
    if let Some(focus) = focus {
        let pane = focus.pane;
        push(
            "Nueva pestaña",
            Some("Ctrl T"),
            lucide::plus(),
            Rc::new(move || ops.new_tab(pane)),
        );
        push(
            "Dividir el panel",
            Some("Ctrl \\"),
            lucide::columns_2(),
            Rc::new(move || ops.split(pane, SplitOrientation::Horizontal)),
        );
        push(
            "Borrador en este panel",
            None,
            lucide::pen_line(),
            Rc::new(move || ops.new_scratch(pane)),
        );
        if let Some(tab) = focus.tab.clone() {
            let index = focus.index;
            push(
                "Cerrar pestaña",
                Some("Ctrl W"),
                lucide::x(),
                Rc::new(move || ops.close_tab(pane, index, &tab)),
            );
        }
        if let Some(item) = focus.item().filter(|i| data.item(*i).is_some()) {
            push(
                "Abrir al lado",
                None,
                lucide::columns_2(),
                Rc::new(move || crate::flows::open_beside(item)),
            );
            push(
                "Cuaderno",
                None,
                lucide::notebook_pen(),
                Rc::new(move || crate::flows::notebook(item)),
            );
            push(
                "Vincular con…",
                None,
                lucide::link(),
                Rc::new(move || crate::flows::link_with(item)),
            );
            push(
                "Compartir…",
                None,
                lucide::share_2(),
                Rc::new(move || crate::flows::share_item(item)),
            );
            push(
                "Exportar…",
                None,
                lucide::download(),
                Rc::new(move || crate::flows::export_items(vec![item])),
            );
            push(
                "Editar…",
                None,
                lucide::square_pen(),
                Rc::new(move || crate::flows::edit_item(item)),
            );
            if let Some(kind) = data
                .item(item)
                .and_then(|i| crate::plugins::kind(&i.kind.plugin_id))
            {
                for action in kind.actions() {
                    let id = action.id.to_string();
                    push(
                        &action.label,
                        None,
                        action.icon.clone(),
                        Rc::new(move || crate::flows::run_kind_action(item, &id)),
                    );
                }
            }
            push(
                "Mover a la papelera",
                None,
                lucide::trash_2(),
                Rc::new(move || crate::flows::trash_item(item)),
            );
        }
    }
    push(
        "Panel del documento",
        Some("Ctrl B"),
        lucide::panel_left(),
        Rc::new(move || ui.toggle_panel()),
    );
    push(
        "Marcadores",
        None,
        lucide::bookmark(),
        Rc::new(move || ui.show_panel(PanelTab::Bookmarks)),
    );
    push(
        "Vínculos",
        None,
        lucide::link(),
        Rc::new(move || ui.show_panel(PanelTab::Links)),
    );
    push(
        "Mapa de vínculos",
        None,
        lucide::waypoints(),
        Rc::new(crate::screens::map::open),
    );
    push(
        "Nueva mesa",
        Some("Ctrl Shift N"),
        lucide::layout_grid(),
        Rc::new(crate::flows::create::new_mesa),
    );
    push(
        "Ajustes",
        Some("Ctrl ,"),
        lucide::settings(),
        Rc::new(crate::screens::settings::open),
    );
    out
}

/// Split "/pdf romanos" into the kind filter and the query.
fn parse(text: &str) -> (Option<String>, String) {
    let text = text.trim_start();
    if let Some(rest) = text.strip_prefix('/') {
        let mut parts = rest.splitn(2, ' ');
        let kind = parts.next().unwrap_or_default().to_lowercase();
        let query = parts.next().unwrap_or_default().to_owned();
        let matched = crate::plugins::catalog()
            .kinds()
            .iter()
            .map(|k| k.info())
            .find(|k| {
                !kind.is_empty()
                    && (k.label.to_lowercase().starts_with(&kind)
                        || k.plural.to_lowercase().starts_with(&kind))
            })
            .map(|k| k.id.into_owned());
        return (matched, query);
    }
    (None, text.to_owned())
}

/// Open the palette.
pub(super) fn open(data: Arc<LibraryData>, ops: Ops, ui: MesaUi, focus: Option<&Focus>) {
    let commands = commands(&data, ops, ui, focus);
    ModalController::get().open(
        Modal::new(Palette { data, commands })
            .manual()
            .dismiss_on_backdrop(true)
            .backdrop_alpha(100),
    );
}

#[derive(Clone, Debug, PartialEq)]
struct Palette {
    data: Arc<LibraryData>,
    commands: Vec<Command>,
}

impl Component for Palette {
    fn render(&self) -> impl IntoElement {
        let p = theme();
        let query = use_state(String::new);
        let mut results = use_state(Results::default);
        let mut highlighted = use_state(|| 0usize);
        let data = Arc::clone(&self.data);
        use_side_effect(move || {
            let (_, text) = parse(&query.read());
            highlighted.set(0);
            if text.trim().is_empty() {
                results.set(Results::default());
                return;
            }
            let data = Arc::clone(&data);
            spawn(async move {
                async_io::Timer::after(DEBOUNCE).await;
                if parse(&query.peek()).1 != text {
                    return;
                }
                let found = crate::search::run_async(text.clone(), data).await;
                if parse(&query.peek()).1 == text {
                    results.set(found);
                }
            });
        });
        let raw = query.read().clone();
        let (kind_filter, text) = parse(&raw);
        let needle = text.trim().to_lowercase();
        let current = results.read().clone();
        let keep_kind = |kind: &str| kind_filter.as_deref().is_none_or(|k| k == kind);

        let docs: Vec<Line> = if needle.is_empty() {
            let mut recent: Vec<_> = self
                .data
                .live_items()
                .filter(|i| keep_kind(&i.kind.plugin_id))
                .collect();
            recent.sort_by_key(|i| std::cmp::Reverse(i.updated_at));
            recent
                .into_iter()
                .take(5)
                .map(|i| Line::Doc(i.id))
                .collect()
        } else {
            current
                .documents
                .iter()
                .filter(|d| keep_kind(&d.kind))
                .map(|d| Line::Doc(d.item))
                .collect()
        };
        let places: Vec<(String, Vec<Line>)> = current
            .groups
            .iter()
            .filter(|g| keep_kind(&g.kind))
            .map(|g| {
                (
                    g.label.clone(),
                    g.hits.iter().cloned().map(Line::Place).collect(),
                )
            })
            .collect();
        let commands: Vec<Line> = if kind_filter.is_some() {
            Vec::new()
        } else {
            self.commands
                .iter()
                .filter(|c| needle.is_empty() || c.label.to_lowercase().contains(&needle))
                .cloned()
                .map(Line::Command)
                .collect()
        };
        let all: Vec<Line> = commands
            .iter()
            .chain(docs.iter())
            .chain(places.iter().flat_map(|(_, l)| l.iter()))
            .cloned()
            .collect();
        let count = all.len();
        let selected = (*highlighted.read()).min(count.saturating_sub(1));

        let mut index = 0usize;
        let mut row = |line: &Line| -> Element {
            let on = index == selected;
            index += 1;
            let action = line.clone();
            let base = match line {
                Line::Command(c) => {
                    let row = ListRow::new(c.label.clone()).icon(c.icon.clone());
                    match c.shortcut {
                        Some(keys) => {
                            row.trailing(Trailing::Element(Kbd::new(keys).into_element()))
                        }
                        None => row,
                    }
                }
                Line::Doc(item) => self.data.item(*item).map_or_else(
                    || ListRow::new("Documento"),
                    |found| {
                        let info = crate::plugins::kind_info(&found.kind.plugin_id);
                        ListRow::new(found.name.clone())
                            .subtitle(crate::flows::forms::folder_path(&self.data, found.folder))
                            .kind(
                                info.as_ref()
                                    .map_or_else(freya::icons::lucide::file, |k| k.icon.clone()),
                                info.map_or(p.t3, |k| kind_color(&p, k.color)),
                            )
                    },
                ),
                Line::Place(place) => {
                    ListRow::new(place.title.clone()).subtitle(if place.snippet.is_empty() {
                        place.doc_name.clone()
                    } else {
                        format!("{} · {}", place.doc_name, place.snippet)
                    })
                }
            };
            base.dense(true)
                .selected(on)
                .on_press(move |_| action.run(false))
                .into_element()
        };
        let mut sections: Vec<Element> = Vec::new();
        if !commands.is_empty() {
            let rows: Vec<Element> = commands.iter().map(&mut row).collect();
            sections.push(
                Section::new("Acciones")
                    .child(notein_ui_kit::Group::default().children(rows))
                    .into_element(),
            );
        }
        if !docs.is_empty() {
            let rows: Vec<Element> = docs.iter().map(&mut row).collect();
            sections.push(
                Section::new(if needle.is_empty() {
                    "Recientes"
                } else {
                    "Documentos"
                })
                .child(notein_ui_kit::Group::default().children(rows))
                .into_element(),
            );
        }
        for (label, lines) in &places {
            let rows: Vec<Element> = lines.iter().map(&mut row).collect();
            sections.push(
                Section::new(label.clone())
                    .child(notein_ui_kit::Group::default().children(rows))
                    .into_element(),
            );
        }
        if sections.is_empty() {
            sections.push(
                Text::new(format!("Nada con «{}»", text.trim()))
                    .role(TextRole::Hint)
                    .into_element(),
            );
        }

        let key_lines = all;
        let compact = width_class().is_compact();
        let size = *Platform::get().root_size.read();
        let width = if compact {
            size.width
        } else {
            WIDTH.min(size.width - 32.0)
        };
        let top = if compact { 0.0 } else { 64.0 };
        let mut field_query = query;
        rect()
            .position(
                Position::new_absolute()
                    .top(top)
                    .left(((size.width - width) / 2.0).max(0.0)),
            )
            .width(Size::px(width))
            .max_height(Size::px(if compact {
                size.height
            } else {
                (size.height - top - 48.0).max(240.0)
            }))
            .padding(tokens::space::MD)
            .spacing(tokens::space::MD)
            .corner_radius(if compact { 0.0 } else { tokens::radius::XL })
            .background(p.s1)
            .border(
                Border::new()
                    .fill(p.line)
                    .width(1.0)
                    .alignment(BorderAlignment::Inner),
            )
            .shadow(Shadow::new().y(18.0).blur(48.0).color(p.shadow))
            .content(Content::Flex)
            .on_global_key_down(move |e: Event<KeyboardEventData>| match &e.key {
                Key::Named(NamedKey::ArrowDown) => {
                    e.stop_propagation();
                    highlighted.set((selected + 1).min(count.saturating_sub(1)));
                }
                Key::Named(NamedKey::ArrowUp) => {
                    e.stop_propagation();
                    highlighted.set(selected.saturating_sub(1));
                }
                Key::Named(NamedKey::Enter) => {
                    if let Some(line) = key_lines.get(selected) {
                        e.stop_propagation();
                        line.run(e.modifiers.ctrl() || e.modifiers.meta());
                    }
                }
                _ => {}
            })
            .child(
                SearchField::new(query, "Buscar o hacer algo… («/pdf» filtra por tipo)")
                    .auto_focus(true)
                    .height(44.0)
                    .on_change(move |text: String| {
                        if text != *field_query.peek() {
                            field_query.set(text);
                        }
                    }),
            )
            .maybe_child(
                kind_filter
                    .as_ref()
                    .and_then(|k| crate::plugins::kind_info(k))
                    .map(|info| {
                        notein_ui_kit::Tag::new(format!("Solo {}", info.plural))
                            .dot(kind_color(&p, info.color))
                    }),
            )
            .child(
                ScrollView::new()
                    .width(Size::fill())
                    .height(Size::flex(1.0))
                    .child(rect().width(Size::fill()).spacing(12.0).children(sections)),
            )
    }
}
