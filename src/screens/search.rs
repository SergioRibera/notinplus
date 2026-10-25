//! The one search: documents by name, and places inside them from every
//! kind (verses, PDF pages, note headings, bookmarks).

use std::sync::Arc;
use std::time::Duration;

use freya::prelude::*;
use notein_plugin::services::Placement;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Kbd, ListRow, Modal, ModalController, Notice, NoticeTone, ProgressBar, SearchField, Section,
    Text, TextRole, tokens, width_class,
};

use crate::library::{ItemId, LibraryData};
use crate::persist::Persisted;
use crate::references::kind_color;
use crate::search::{History, PlaceMatch, Results};
use crate::state::LibraryView;

/// Wait after the last keystroke before searching.
const DEBOUNCE: Duration = Duration::from_millis(140);
/// Width of the panel on wide screens.
const WIDTH: f32 = 640.0;
/// Documents offered while the field is empty.
const RECENT_DOCS: usize = 6;

/// Open the search panel, starting with `query`.
pub fn open(query: String) {
    ModalController::get().open(
        Modal::new(SearchPanel { query })
            .manual()
            .dismiss_on_backdrop(true)
            .backdrop_alpha(110),
    );
}

/// One selectable line of the panel.
#[derive(Clone, Debug, PartialEq)]
enum Line {
    Doc(ItemId),
    Place(PlaceMatch),
    Recent(String),
    CreateNote(String),
}

impl Line {
    fn run(&self, beside: bool, mut query: State<String>) {
        let remember = query.peek().clone();
        let record = move || {
            let mut history = History::load();
            history.push(&remember);
            if let Err(err) = history.save() {
                log::warn!("search history: {err}");
            }
        };
        match self {
            Self::Doc(item) => {
                record();
                ModalController::get().close();
                if beside {
                    crate::flows::open_beside(*item);
                } else {
                    crate::flows::open(*item);
                }
            }
            Self::Place(place) => {
                record();
                ModalController::get().close();
                crate::plugins::navigate::open(
                    &place.target,
                    if beside {
                        Placement::Beside
                    } else {
                        Placement::Here
                    },
                );
            }
            Self::Recent(text) => query.set(text.clone()),
            Self::CreateNote(name) => {
                ModalController::get().close();
                let entry = crate::plugins::catalog().entries().into_iter().find(|e| {
                    e.kind == notein_notes::kind::KIND
                        && e.entry.id == notein_notes::kind::ENTRY_NOTE
                });
                if let Some(entry) = entry {
                    crate::flows::pickers::create_quick(
                        entry,
                        name.clone(),
                        crate::flows::pickers::Purpose::NewTab { pane: None },
                    );
                }
            }
        }
    }
}

/// Recently changed documents, for the empty field.
fn recent_docs(data: &LibraryData) -> Vec<ItemId> {
    let mut items: Vec<_> = data.live_items().collect();
    items.sort_by_key(|i| std::cmp::Reverse(i.updated_at));
    items.into_iter().take(RECENT_DOCS).map(|i| i.id).collect()
}

fn lines(results: &Results, history: &[String], recents: &[ItemId], query: &str) -> Vec<Line> {
    if query.trim().is_empty() {
        return history
            .iter()
            .cloned()
            .map(Line::Recent)
            .chain(recents.iter().copied().map(Line::Doc))
            .collect();
    }
    let mut out: Vec<Line> = results
        .documents
        .iter()
        .map(|d| Line::Doc(d.item))
        .collect();
    out.extend(
        results
            .groups
            .iter()
            .flat_map(|g| g.hits.iter().cloned().map(Line::Place)),
    );
    if out.is_empty() {
        out.push(Line::CreateNote(query.trim().to_owned()));
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SearchPanel {
    query: String,
}

impl Component for SearchPanel {
    fn render(&self) -> impl IntoElement {
        let initial = self.query.clone();
        let query = use_state(move || initial);
        let history = use_hook(|| History::load().recent);
        let data = LibraryView::get().data();
        let (results, mut highlighted, searching) = use_search(query, data.clone());
        let recents = data.as_deref().map(recent_docs).unwrap_or_default();

        let text = query.read().clone();
        let current = results.read().clone();
        let list = lines(&current, &history, &recents, &text);
        let count = list.len();
        let selected = (*highlighted.read()).min(count.saturating_sub(1));
        let mut rows = Rows {
            selected,
            query,
            data: data.as_deref(),
            next: 0,
        };
        let sections = if text.trim().is_empty() {
            rows.idle_sections(&list, !history.is_empty())
        } else {
            rows.result_sections(&list, &current, &text, searching())
        };

        let compact = width_class().is_compact();
        let on_key = move |e: Event<KeyboardEventData>| match &e.key {
            Key::Named(NamedKey::ArrowDown) => {
                e.stop_propagation();
                highlighted.set((selected + 1).min(count.saturating_sub(1)));
            }
            Key::Named(NamedKey::ArrowUp) => {
                e.stop_propagation();
                highlighted.set(selected.saturating_sub(1));
            }
            Key::Named(NamedKey::Enter) => {
                if let Some(line) = list.get(selected) {
                    e.stop_propagation();
                    line.run(e.modifiers.ctrl() || e.modifiers.meta(), query);
                }
            }
            _ => {}
        };

        overlay_frame(72.0)
            .on_global_key_down(on_key)
            .child(field_row(query, compact))
            .child(
                ScrollView::new()
                    .width(Size::fill())
                    .height(Size::flex(1.0))
                    .child(rect().width(Size::fill()).spacing(14.0).children(sections)),
            )
            .maybe_child((!compact).then(key_hints))
    }
}

/// Run the search `DEBOUNCE` after the query stops changing. Returns
/// the results, the highlighted line and whether a search is running.
fn use_search(
    query: State<String>,
    data: Option<Arc<LibraryData>>,
) -> (State<Results>, State<usize>, State<bool>) {
    let mut results = use_state(Results::default);
    let mut highlighted = use_state(|| 0usize);
    let mut searching = use_state(|| false);
    use_side_effect(move || {
        let text = query.read().clone();
        highlighted.set(0);
        let Some(data) = data.clone() else {
            return;
        };
        if text.trim().is_empty() {
            results.set(Results::default());
            return;
        }
        searching.set(true);
        spawn(async move {
            async_io::Timer::after(DEBOUNCE).await;
            if *query.peek() != text {
                return;
            }
            let found = crate::search::run_async(text.clone(), data).await;
            if *query.peek() == text {
                results.set(found);
                searching.set(false);
            }
        });
    });
    (results, highlighted, searching)
}

/// The field, with Cancelar on phones.
fn field_row(query: State<String>, compact: bool) -> Rect {
    rect()
        .width(Size::fill())
        .horizontal()
        .cross_align(Alignment::Center)
        .content(Content::Flex)
        .spacing(8.0)
        .child(
            rect().width(Size::flex(1.0)).child(
                SearchField::new(query, "Buscar documentos, versículos, notas…")
                    .auto_focus(true)
                    .height(44.0),
            ),
        )
        .maybe_child(compact.then(|| {
            notein_ui_kit::Button::new("Cancelar")
                .ghost()
                .on_press(|_| ModalController::get().close())
        }))
}

fn key_hints() -> Rect {
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(6.0)
        .child(Kbd::new("↑↓"))
        .child(Text::new("elegir").role(TextRole::Hint))
        .child(Kbd::new("Enter"))
        .child(Text::new("abrir").role(TextRole::Hint))
        .child(Kbd::new("Ctrl Enter"))
        .child(Text::new("abrir al lado").role(TextRole::Hint))
}

/// Builds the rows in the order the arrow keys walk them.
struct Rows<'a> {
    selected: usize,
    query: State<String>,
    data: Option<&'a LibraryData>,
    /// Index of the next row.
    next: usize,
}

impl Rows<'_> {
    fn row(&mut self, line: &Line) -> Element {
        use freya::icons::lucide;
        let p = palette();
        let on = self.next == self.selected;
        self.next += 1;
        let action = line.clone();
        let query = self.query;
        let base = match line {
            Line::Doc(item) => {
                let Some((d, found)) = self.data.and_then(|d| d.item(*item).map(|f| (d, f))) else {
                    return rect().into_element();
                };
                let info = crate::plugins::kind_info(&found.kind.plugin_id);
                ListRow::new(found.name.clone())
                    .subtitle(crate::flows::forms::folder_path(d, found.folder))
                    .kind(
                        info.as_ref().map_or_else(lucide::file, |k| k.icon.clone()),
                        info.map_or(p.t3, |k| kind_color(&p, k.color)),
                    )
            }
            Line::Place(place) => {
                ListRow::new(place.title.clone()).subtitle(if place.snippet.is_empty() {
                    place.doc_name.clone()
                } else {
                    format!("{} · {}", place.doc_name, place.snippet)
                })
            }
            Line::Recent(text) => ListRow::new(text.clone()).icon(lucide::history()),
            Line::CreateNote(name) => ListRow::new(format!("Crear nota «{name}»"))
                .kind(lucide::notebook_text(), p.kinds.note),
        };
        base.dense(true)
            .selected(on)
            .on_press(move |_| action.run(false, query))
            .into_element()
    }

    fn rows<'l>(&mut self, lines: impl Iterator<Item = &'l Line>) -> notein_ui_kit::Group {
        notein_ui_kit::Group::default().children(lines.map(|l| self.row(l)).collect::<Vec<_>>())
    }

    /// Recent searches and documents, for the empty field.
    fn idle_sections(&mut self, list: &[Line], has_history: bool) -> Vec<Element> {
        let mut sections = Vec::new();
        if has_history {
            let rows = self.rows(list.iter().filter(|l| matches!(l, Line::Recent(_))));
            sections.push(
                Section::new("Búsquedas recientes")
                    .child(rows)
                    .into_element(),
            );
        }
        let docs: Vec<&Line> = list.iter().filter(|l| matches!(l, Line::Doc(_))).collect();
        if !docs.is_empty() {
            let rows = self.rows(docs.into_iter());
            sections.push(
                Section::new("Abiertos hace poco")
                    .child(rows)
                    .into_element(),
            );
        }
        sections
    }

    /// Documents, then one section per kind of place, then what is
    /// still being read.
    fn result_sections(
        &mut self,
        list: &[Line],
        current: &Results,
        text: &str,
        searching: bool,
    ) -> Vec<Element> {
        let p = palette();
        let mut sections = Vec::new();
        if !current.documents.is_empty() {
            let rows = self.rows(list.iter().take(current.documents.len()));
            sections.push(Section::new("Documentos").child(rows).into_element());
        }
        for group in &current.groups {
            let hits: Vec<Line> = group.hits.iter().cloned().map(Line::Place).collect();
            let rows = self.rows(hits.iter());
            sections.push(
                Section::new(group.label.clone())
                    .icon(group.icon.clone())
                    .trailing(
                        Text::new(group.hits.len().to_string())
                            .role(TextRole::Meta)
                            .color(kind_color(&p, group.color)),
                    )
                    .child(rows)
                    .into_element(),
            );
        }
        if current.is_empty() && !searching {
            self.next = 0;
            sections.push(
                rect()
                    .width(Size::fill())
                    .spacing(8.0)
                    .child(Text::new(format!("Nada con «{}»", text.trim())).role(TextRole::Strong))
                    .child(self.row(&Line::CreateNote(text.trim().to_owned())))
                    .into_element(),
            );
        }
        sections.extend(current.indexing.iter().map(|job| {
            #[allow(clippy::cast_precision_loss)] // page counts
            let fraction = if job.total == 0 {
                0.0
            } else {
                job.done as f32 / job.total as f32
            };
            rect()
                .width(Size::fill())
                .spacing(6.0)
                .child(Notice::new(
                    NoticeTone::Info,
                    format!(
                        "Todavía leyendo «{}» ({} de {}). Puede que falten resultados.",
                        job.name, job.done, job.total
                    ),
                ))
                .child(ProgressBar::new(fraction).height(3.0))
                .into_element()
        }));
        sections
    }
}

/// The floating panel search and the command palette share: centred
/// under the top on wide screens, the whole screen on phones.
pub(crate) fn overlay_frame(top: f32) -> Rect {
    let p = palette();
    let compact = width_class().is_compact();
    let size = *Platform::get().root_size.read();
    // What the system bars and the keyboard leave of the window.
    let frame = notein_ui_kit::safe_frame();
    let pad = tokens::space::MD;
    let panel = if compact {
        // The whole screen, its content kept off the bars and the keyboard.
        rect()
            .position(Position::new_absolute().top(0.0).left(0.0))
            .width(Size::px(size.width))
            .max_height(Size::px(size.height))
            .padding((
                frame.min_y() + pad,
                size.width - frame.max_x() + pad,
                size.height - frame.max_y() + pad,
                frame.min_x() + pad,
            ))
    } else {
        let width = WIDTH.min(frame.width() - 32.0);
        let top = frame.min_y() + top;
        rect()
            .position(
                Position::new_absolute()
                    .top(top)
                    .left(frame.min_x() + ((frame.width() - width) / 2.0).max(0.0)),
            )
            .width(Size::px(width))
            .max_height(Size::px((frame.max_y() - top - 16.0).max(240.0)))
            .padding(pad)
    };
    panel
        .corner_radius(if compact { 0.0 } else { tokens::radius::XL })
        .background(p.s1)
        .border(
            Border::new()
                .fill(p.line)
                .width(1.0)
                .alignment(BorderAlignment::Inner),
        )
        .shadow(Shadow::new().y(18.0).blur(48.0).color(p.shadow))
        .spacing(tokens::space::MD)
        .content(Content::Flex)
}
