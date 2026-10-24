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
        use freya::icons::lucide;
        let p = palette();
        let initial = self.query.clone();
        let query = use_state(move || initial);
        let mut results = use_state(Results::default);
        let mut highlighted = use_state(|| 0usize);
        let mut searching = use_state(|| false);
        let history = use_hook(|| History::load().recent);
        let data = LibraryView::get().data();
        let recents = data.as_deref().map(recent_docs).unwrap_or_default();

        use_side_effect({
            let data = data.clone();
            move || {
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
                    let found = crate::search::run_async(text.clone(), Arc::clone(&data)).await;
                    if *query.peek() == text {
                        results.set(found);
                        searching.set(false);
                    }
                });
            }
        });

        let text = query.read().clone();
        let current = results.read().clone();
        let list = lines(&current, &history, &recents, &text);
        let count = list.len();
        let selected = (*highlighted.read()).min(count.saturating_sub(1));

        let compact = width_class().is_compact();
        let size = *Platform::get().root_size.read();
        let width = if compact {
            size.width
        } else {
            WIDTH.min(size.width - 32.0)
        };
        let left = ((size.width - width) / 2.0).max(0.0);
        let top = if compact { 0.0 } else { 72.0 };
        let max_height = if compact {
            size.height
        } else {
            (size.height - top - 48.0).max(240.0)
        };

        let key_list = list.clone();
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
                if let Some(line) = key_list.get(selected) {
                    e.stop_propagation();
                    line.run(e.modifiers.ctrl() || e.modifiers.meta(), query);
                }
            }
            _ => {}
        };

        let row = |index: usize, line: &Line, data: Option<&LibraryData>| -> Element {
            let on = index == selected;
            let action = line.clone();
            let base = match line {
                Line::Doc(item) => {
                    let Some((d, found)) = data.and_then(|d| d.item(*item).map(|f| (d, f))) else {
                        return rect().into_element();
                    };
                    let info = crate::plugins::kind_info(&found.kind.plugin_id);
                    let place = crate::flows::forms::folder_path(d, found.folder);
                    ListRow::new(found.name.clone()).subtitle(place).kind(
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
        };

        let data_ref = data.as_deref();
        let mut index = 0usize;
        let mut sections: Vec<Element> = Vec::new();
        if text.trim().is_empty() {
            if !history.is_empty() {
                let rows: Vec<Element> = list
                    .iter()
                    .filter(|l| matches!(l, Line::Recent(_)))
                    .map(|l| {
                        let r = row(index, l, data_ref);
                        index += 1;
                        r
                    })
                    .collect();
                sections.push(
                    Section::new("Búsquedas recientes")
                        .child(notein_ui_kit::Group::default().children(rows))
                        .into_element(),
                );
            }
            let rows: Vec<Element> = list
                .iter()
                .filter(|l| matches!(l, Line::Doc(_)))
                .map(|l| {
                    let r = row(index, l, data_ref);
                    index += 1;
                    r
                })
                .collect();
            if !rows.is_empty() {
                sections.push(
                    Section::new("Abiertos hace poco")
                        .child(notein_ui_kit::Group::default().children(rows))
                        .into_element(),
                );
            }
        } else {
            if !current.documents.is_empty() {
                let rows: Vec<Element> = list
                    .iter()
                    .take(current.documents.len())
                    .map(|l| {
                        let r = row(index, l, data_ref);
                        index += 1;
                        r
                    })
                    .collect();
                sections.push(
                    Section::new("Documentos")
                        .child(notein_ui_kit::Group::default().children(rows))
                        .into_element(),
                );
            }
            for group in &current.groups {
                let tint = kind_color(&p, group.color);
                let rows: Vec<Element> = group
                    .hits
                    .iter()
                    .map(|hit| {
                        let r = row(index, &Line::Place(hit.clone()), data_ref);
                        index += 1;
                        r
                    })
                    .collect();
                sections.push(
                    Section::new(group.label.clone())
                        .icon(group.icon.clone())
                        .trailing(
                            Text::new(group.hits.len().to_string())
                                .role(TextRole::Meta)
                                .color(tint),
                        )
                        .child(notein_ui_kit::Group::default().children(rows))
                        .into_element(),
                );
            }
            if current.is_empty() && !searching() {
                sections.push(
                    rect()
                        .width(Size::fill())
                        .spacing(8.0)
                        .child(
                            Text::new(format!("Nada con «{}»", text.trim())).role(TextRole::Strong),
                        )
                        .child(row(0, &Line::CreateNote(text.trim().to_owned()), data_ref))
                        .into_element(),
                );
            }
            for job in &current.indexing {
                #[allow(clippy::cast_precision_loss)] // page counts
                let fraction = if job.total == 0 {
                    0.0
                } else {
                    job.done as f32 / job.total as f32
                };
                sections.push(
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
                        .into_element(),
                );
            }
        }

        let field = query;
        rect()
            .position(Position::new_absolute().top(top).left(left))
            .width(Size::px(width))
            .max_height(Size::px(max_height))
            .corner_radius(if compact { 0.0 } else { tokens::radius::XL })
            .background(p.s1)
            .border(
                Border::new()
                    .fill(p.line)
                    .width(1.0)
                    .alignment(BorderAlignment::Inner),
            )
            .shadow(Shadow::new().y(18.0).blur(48.0).color(p.shadow))
            .padding(tokens::space::MD)
            .spacing(tokens::space::MD)
            .content(Content::Flex)
            .on_global_key_down(on_key)
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .content(Content::Flex)
                    .spacing(8.0)
                    .child(
                        rect().width(Size::flex(1.0)).child(
                            SearchField::new(field, "Buscar documentos, versículos, notas…")
                                .auto_focus(true)
                                .height(44.0),
                        ),
                    )
                    .maybe_child(compact.then(|| {
                        notein_ui_kit::Button::new("Cancelar")
                            .ghost()
                            .on_press(|_| ModalController::get().close())
                    })),
            )
            .child(
                ScrollView::new()
                    .width(Size::fill())
                    .height(Size::flex(1.0))
                    .child(rect().width(Size::fill()).spacing(14.0).children(sections)),
            )
            .maybe_child((!compact).then(|| {
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
            }))
    }
}
