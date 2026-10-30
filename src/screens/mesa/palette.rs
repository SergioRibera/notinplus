//! Ctrl+K: documents, places inside them and what can be done with the
//! focused document, from one field. "/" narrows to one kind.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use freya::prelude::*;
use notein_i18n::tr;
use notein_plugin::services::Placement;
use notein_ui_kit::theme::palette as theme;
use notein_ui_kit::{
    Kbd, ListRow, Modal, ModalController, SearchField, Section, Text, TextRole, Trailing,
};

use super::{Focus, MesaUi, Ops, PanelTab};
use crate::library::{ItemId, LibraryData, SplitOrientation};
use crate::references::kind_color;
use crate::search::{PlaceMatch, Results};

const DEBOUNCE: Duration = Duration::from_millis(120);

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

impl Command {
    fn new(
        label: impl Into<String>,
        shortcut: Option<&'static str>,
        icon: Bytes,
        run: impl Fn() + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            shortcut,
            icon,
            run: Rc::new(run),
        }
    }
}

/// The commands for the focused document and the mesa.
fn commands(data: &LibraryData, ops: Ops, ui: MesaUi, focus: Option<&Focus>) -> Vec<Command> {
    let mut out = Vec::new();
    if let Some(focus) = focus {
        out.extend(pane_commands(focus, ops));
        if let Some(item) = focus.item().filter(|i| data.item(*i).is_some()) {
            out.extend(document_commands(data, item));
        }
    }
    out.extend(mesa_commands(ui));
    out
}

/// What can be done with the focused pane.
fn pane_commands(focus: &Focus, ops: Ops) -> Vec<Command> {
    use freya::icons::lucide;
    let pane = focus.pane;
    let mut out = vec![
        Command::new(
            tr!("mesa-tab-new"),
            Some("Ctrl T"),
            lucide::plus(),
            move || {
                ops.new_tab(pane);
            },
        ),
        Command::new(
            tr!("mesa-cmd-split-pane"),
            Some("Ctrl \\"),
            lucide::columns_2(),
            move || {
                ops.split(pane, SplitOrientation::Horizontal);
            },
        ),
        Command::new(
            tr!("mesa-cmd-draft-here"),
            None,
            lucide::pen_line(),
            move || {
                ops.new_scratch(pane);
            },
        ),
    ];
    if let Some(tab) = focus.tab.clone() {
        let index = focus.index;
        out.push(Command::new(
            tr!("mesa-tab-close"),
            Some("Ctrl W"),
            lucide::x(),
            move || {
                ops.close_tab(pane, index, &tab);
            },
        ));
    }
    out
}

/// What can be done with document `item`, its kind's own actions
/// included.
fn document_commands(data: &LibraryData, item: ItemId) -> Vec<Command> {
    use crate::flows;
    use freya::icons::lucide;
    let mut out = vec![
        Command::new(
            tr!("mesa-cmd-open-beside"),
            None,
            lucide::columns_2(),
            move || {
                flows::open_beside(item);
            },
        ),
        Command::new(
            tr!("mesa-cmd-notebook"),
            None,
            lucide::notebook_pen(),
            move || {
                flows::notebook(item);
            },
        ),
        Command::new(tr!("mesa-cmd-link-with"), None, lucide::link(), move || {
            flows::link_with(item);
        }),
        Command::new(tr!("mesa-cmd-share"), None, lucide::share_2(), move || {
            flows::share_item(item);
        }),
        Command::new(
            tr!("mesa-cmd-export"),
            None,
            lucide::download(),
            move || {
                flows::export_items(vec![item]);
            },
        ),
        Command::new(
            tr!("mesa-cmd-edit"),
            None,
            lucide::square_pen(),
            move || {
                flows::edit_item(item);
            },
        ),
    ];
    if let Some(kind) = data
        .item(item)
        .and_then(|i| crate::plugins::kind(&i.kind.plugin_id))
    {
        out.extend(kind.actions().into_iter().map(|action| {
            let id = action.id.to_string();
            Command::new(action.label.clone(), None, action.icon.clone(), move || {
                flows::run_kind_action(item, &id);
            })
        }));
    }
    out.push(Command::new(
        tr!("common-move-to-trash"),
        None,
        lucide::trash_2(),
        move || {
            flows::trash_item(item);
        },
    ));
    out
}

/// What can be done with the mesa itself.
fn mesa_commands(ui: MesaUi) -> Vec<Command> {
    use freya::icons::lucide;
    vec![
        Command::new(
            tr!("mesa-document-panel"),
            Some("Ctrl B"),
            lucide::panel_left(),
            move || {
                ui.toggle_panel();
            },
        ),
        Command::new(
            tr!("mesa-panel-bookmarks"),
            None,
            lucide::bookmark(),
            move || {
                ui.show_panel(PanelTab::Bookmarks);
            },
        ),
        Command::new(tr!("mesa-panel-links"), None, lucide::link(), move || {
            ui.show_panel(PanelTab::Links);
        }),
        Command::new(
            tr!("mesa-cmd-link-map"),
            None,
            lucide::waypoints(),
            crate::screens::map::open,
        ),
        Command::new(
            tr!("mesa-cmd-new-desk"),
            Some("Ctrl Shift N"),
            lucide::layout_grid(),
            crate::flows::create::new_mesa,
        ),
        Command::new(
            tr!("common-settings"),
            Some("Ctrl ,"),
            lucide::settings(),
            crate::screens::settings::open,
        ),
    ]
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
            .instant()
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
        let (results, mut highlighted) = use_palette_search(query, &self.data);
        let raw = query.read().clone();
        let (kind_filter, text) = parse(&raw);
        let found = self.found(
            kind_filter.as_deref(),
            &text.trim().to_lowercase(),
            &results.read(),
        );
        let all = found.all();
        let count = all.len();
        let selected = (*highlighted.read()).min(count.saturating_sub(1));
        let mut rows = PaletteRows {
            selected,
            data: &self.data,
            next: 0,
        };
        let mut sections = found.sections(&mut rows, text.trim().is_empty());
        if sections.is_empty() {
            sections.push(
                Text::new(tr!("mesa-palette-no-results", "query" => text.trim()))
                    .role(TextRole::Hint)
                    .into_element(),
            );
        }

        let mut field_query = query;
        crate::screens::search::overlay_frame(64.0)
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
                    if let Some(line) = all.get(selected) {
                        e.stop_propagation();
                        line.run(e.modifiers.ctrl() || e.modifiers.meta());
                    }
                }
                _ => {}
            })
            .child(
                SearchField::new(query, tr!("mesa-palette-placeholder"))
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
                        notein_ui_kit::Tag::new(
                            tr!("mesa-palette-only-kind", "kinds" => info.plural.to_string()),
                        )
                        .dot(kind_color(&p, info.color))
                    }),
            )
            .child(
                ScrollView::new()
                    .width(Size::fill())
                    .height(Size::flex(1.0))
                    .child(notein_ui_kit::ScrollContent::new(
                        rect().width(Size::fill()).spacing(12.0).children(sections),
                    )),
            )
    }
}

/// Search the query (without its "/kind" filter) a moment after it
/// stops changing. Returns the results and the highlighted line.
fn use_palette_search(
    query: State<String>,
    data: &Arc<LibraryData>,
) -> (State<Results>, State<usize>) {
    let mut results = use_state(Results::default);
    let mut highlighted = use_state(|| 0usize);
    let data = Arc::clone(data);
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
    (results, highlighted)
}

/// What the palette lists for one query.
struct Found {
    commands: Vec<Line>,
    docs: Vec<Line>,
    places: Vec<(String, Vec<Line>)>,
}

impl Palette {
    /// Lines for `needle` (lower-cased), only of `kind` when given.
    fn found(&self, kind: Option<&str>, needle: &str, current: &Results) -> Found {
        let keep_kind = |k: &str| kind.is_none_or(|wanted| wanted == k);
        let docs = if needle.is_empty() {
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
        let places = current
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
        // A kind filter is about documents; actions would only get in the way.
        let commands = if kind.is_some() {
            Vec::new()
        } else {
            self.commands
                .iter()
                .filter(|c| needle.is_empty() || c.label.to_lowercase().contains(needle))
                .cloned()
                .map(Line::Command)
                .collect()
        };
        Found {
            commands,
            docs,
            places,
        }
    }
}

impl Found {
    /// Every line in the order the arrow keys walk them.
    fn all(&self) -> Vec<Line> {
        self.commands
            .iter()
            .chain(self.docs.iter())
            .chain(self.places.iter().flat_map(|(_, l)| l.iter()))
            .cloned()
            .collect()
    }

    fn sections(&self, rows: &mut PaletteRows<'_>, idle: bool) -> Vec<Element> {
        let mut sections = Vec::new();
        if !self.commands.is_empty() {
            sections.push(rows.section(tr!("mesa-palette-actions"), &self.commands));
        }
        if !self.docs.is_empty() {
            let title = if idle {
                tr!("mesa-palette-recent")
            } else {
                tr!("mesa-palette-documents")
            };
            sections.push(rows.section(title, &self.docs));
        }
        for (label, lines) in &self.places {
            sections.push(rows.section(label.clone(), lines));
        }
        sections
    }
}

/// Builds the rows in the order the arrow keys walk them.
struct PaletteRows<'a> {
    selected: usize,
    data: &'a LibraryData,
    /// Index of the next row.
    next: usize,
}

impl PaletteRows<'_> {
    fn section(&mut self, title: String, lines: &[Line]) -> Element {
        let rows: Vec<Element> = lines.iter().map(|l| self.row(l)).collect();
        Section::new(title)
            .child(notein_ui_kit::Group::default().children(rows))
            .into_element()
    }

    fn row(&mut self, line: &Line) -> Element {
        let p = theme();
        let on = self.next == self.selected;
        self.next += 1;
        let action = line.clone();
        let base = match line {
            Line::Command(c) => {
                let row = ListRow::new(c.label.clone()).icon(c.icon.clone());
                match c.shortcut {
                    Some(keys) => row.trailing(Trailing::Element(Kbd::new(keys).into_element())),
                    None => row,
                }
            }
            Line::Doc(item) => self.data.item(*item).map_or_else(
                || ListRow::new(tr!("mesa-palette-document")),
                |found| {
                    let info = crate::plugins::kind_info(&found.kind.plugin_id);
                    ListRow::new(found.name.clone())
                        .subtitle(crate::flows::forms::folder_path(self.data, found.folder))
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
    }
}
