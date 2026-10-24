//! The document panel beside the panes: what the focused document
//! contains, its bookmarks, and what it is connected to.

use std::sync::Arc;

use freya::prelude::*;
use notein_plugin::bookmark::SourceRef;
use notein_plugin::kind::{OutlineIcon, OutlineNode};
use notein_plugin::services::Placement;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Button, EmptyState, Group, ListRow, SearchField, Section, Segmented, Sheet, Tag, Text,
    TextRole, tokens,
};

use super::Focus;
use crate::library::{ItemId, LibraryData, LinkKind, refs};
use crate::references::kind_color;

/// Which list the panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PanelTab {
    #[default]
    Contents,
    Bookmarks,
    Links,
}

impl PanelTab {
    const ALL: [Self; 3] = [Self::Contents, Self::Bookmarks, Self::Links];

    const fn label(self) -> &'static str {
        match self {
            Self::Contents => "Contenido",
            Self::Bookmarks => "Marcadores",
            Self::Links => "Vínculos",
        }
    }
}

fn jump(kind: &str, item: ItemId, anchor: Vec<u8>) {
    crate::plugins::navigate::open(
        &SourceRef::Plugin {
            plugin_id: kind.to_owned(),
            item: item.0,
            anchor,
            selection: None,
        },
        Placement::Here,
    );
}

fn outline_icon(icon: OutlineIcon) -> Bytes {
    use freya::icons::lucide;
    match icon {
        OutlineIcon::Section => lucide::list(),
        OutlineIcon::Page => lucide::file(),
        OutlineIcon::Heading => lucide::heading(),
        OutlineIcon::Layer => lucide::layers(),
        OutlineIcon::Book => lucide::book_open(),
    }
}

fn matches(node: &OutlineNode<Vec<u8>>, query: &str) -> bool {
    query.is_empty()
        || node.label.to_lowercase().contains(query)
        || node.children.iter().any(|c| matches(c, query))
}

fn outline_rows(
    nodes: &[OutlineNode<Vec<u8>>],
    depth: u8,
    query: &str,
    kind: &str,
    item: ItemId,
    out: &mut Vec<Element>,
) {
    for node in nodes.iter().filter(|n| matches(n, query)) {
        let anchor = node.anchor.clone();
        let kind_id = kind.to_owned();
        let row = ListRow::new(node.label.clone())
            .icon(outline_icon(node.icon))
            .dense(true)
            .indent(f32::from(depth) * 14.0);
        let row = match &node.meta {
            Some(meta) => row.meta(meta.clone()),
            None => row,
        };
        out.push(
            row.enabled(anchor.is_some())
                .on_press(move |_| {
                    if let Some(anchor) = anchor.clone() {
                        jump(&kind_id, item, anchor);
                    }
                })
                .into_element(),
        );
        outline_rows(
            &node.children,
            depth.saturating_add(1),
            query,
            kind,
            item,
            out,
        );
    }
}

/// The docked panel.
#[derive(Clone, PartialEq)]
pub(super) struct Panel {
    pub(super) data: Arc<LibraryData>,
    pub(super) focus: Option<Focus>,
    pub(super) tab: State<PanelTab>,
}

impl Component for Panel {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        rect()
            .width(Size::px(tokens::size::PANEL_WIDTH))
            .height(Size::fill())
            .padding((4.0, 8.0, 8.0, 12.0))
            .background(p.bg)
            .child(PanelBody {
                data: Arc::clone(&self.data),
                item: self.focus.as_ref().and_then(Focus::item),
                tab: self.tab,
            })
    }
}

/// The panel as a sheet, on phones.
pub(super) fn open_sheet(data: Arc<LibraryData>, focus: Option<&Focus>, start: PanelTab) {
    let tab = State::create_in_scope(start, ScopeId::ROOT);
    Sheet::titled("Documento")
        .child(PanelBody {
            data,
            item: focus.and_then(Focus::item),
            tab,
        })
        .open();
}

#[derive(Clone, PartialEq)]
struct PanelBody {
    data: Arc<LibraryData>,
    item: Option<ItemId>,
    tab: State<PanelTab>,
}

impl Component for PanelBody {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let mut tab = self.tab;
        let current = *tab.read();
        let query = use_state(String::new);
        let mut color = use_state(|| None::<[u8; 3]>);
        let text = query.read().trim().to_lowercase();
        let index = PanelTab::ALL
            .iter()
            .position(|t| *t == current)
            .unwrap_or(0);
        let head = rect()
            .width(Size::fill())
            .spacing(8.0)
            .child(
                Segmented::new(PanelTab::ALL.map(PanelTab::label), index)
                    .full_width()
                    .height(32.0)
                    .on_select(move |i: usize| tab.set(PanelTab::ALL[i])),
            )
            .maybe_child((current != PanelTab::Links).then(|| {
                SearchField::new(
                    query,
                    if current == PanelTab::Contents {
                        "Buscar en el contenido"
                    } else {
                        "Buscar marcadores"
                    },
                )
                .height(34.0)
            }));
        let Some(item) = self.item.and_then(|id| self.data.item(id)) else {
            return rect()
                .width(Size::fill())
                .spacing(12.0)
                .child(head)
                .child(
                    EmptyState::new(lucide::panel_left(), "Elegí un documento")
                        .body("El panel muestra el documento del panel enfocado."),
                )
                .into_element();
        };
        let id = item.id;
        let kind_id = item.kind.plugin_id.clone();
        let kind = crate::plugins::kind(&kind_id);
        let content = crate::plugins::content::read(id, &kind_id)
            .unwrap_or_else(|| kind.as_ref().map(|k| k.empty_content()).unwrap_or_default());
        let body: Element = match current {
            PanelTab::Contents => {
                let outline = kind
                    .as_ref()
                    .map(|k| k.outline(&content))
                    .unwrap_or_default();
                let mut rows = Vec::new();
                outline_rows(&outline, 0, &text, &kind_id, id, &mut rows);
                if rows.is_empty() {
                    Text::new(if text.is_empty() {
                        "Este documento no tiene índice."
                    } else {
                        "Nada con ese nombre."
                    })
                    .role(TextRole::Hint)
                    .into_element()
                } else {
                    Group::default().children(rows).into_element()
                }
            }
            PanelTab::Bookmarks => {
                let p = palette();
                let marks = kind.as_ref().map(|k| k.marks(&content)).unwrap_or_default();
                let mut colors: Vec<[u8; 3]> = marks.iter().filter_map(|m| m.color).collect();
                colors.dedup();
                colors.sort_unstable();
                colors.dedup();
                let chosen = *color.read();
                let mut marks: Vec<_> = marks
                    .into_iter()
                    .filter(|m| chosen.is_none() || m.color == chosen)
                    .filter(|m| {
                        text.is_empty()
                            || m.title.to_lowercase().contains(&text)
                            || m.snippet.to_lowercase().contains(&text)
                    })
                    .collect();
                marks.sort_by_key(|m| std::cmp::Reverse(m.updated_at));
                let rows: Vec<Element> = marks
                    .into_iter()
                    .map(|mark| {
                        let anchor = mark.anchor.clone();
                        let kind_id = kind_id.clone();
                        let dot = mark
                            .color
                            .map_or(p.t3, |[r, g, b]| Color::from_rgb(r, g, b));
                        ListRow::new(mark.title.clone())
                            .subtitle(mark.snippet)
                            .leading(notein_ui_kit::Leading::Element(
                                rect()
                                    .width(Size::px(10.0))
                                    .height(Size::px(10.0))
                                    .corner_radius(tokens::radius::FULL)
                                    .background(dot)
                                    .into_element(),
                            ))
                            .dense(true)
                            .on_press(move |_| jump(&kind_id, id, anchor.clone()))
                            .into_element()
                    })
                    .collect();
                rect()
                    .width(Size::fill())
                    .spacing(10.0)
                    .maybe_child((colors.len() > 1).then(|| {
                        rect()
                            .width(Size::fill())
                            .horizontal()
                            .content(Content::wrap_spacing(6.0))
                            .spacing(6.0)
                            .child(Tag::new("Todos").active(chosen.is_none()).on_press(move |_| color.set(None)))
                            .children(colors.into_iter().map(|c| {
                                Tag::new("")
                                    .dot(Color::from_rgb(c[0], c[1], c[2]))
                                    .active(chosen == Some(c))
                                    .on_press(move |_| color.set(Some(c)))
                                    .into_element()
                            }))
                    }))
                    .child(if rows.is_empty() {
                        Text::new("Sin marcadores. Mantené apretado sobre el texto o la hoja para marcar.")
                            .role(TextRole::Hint)
                            .into_element()
                    } else {
                        Group::default().children(rows).into_element()
                    })
                    .into_element()
            }
            PanelTab::Links => Links {
                data: Arc::clone(&self.data),
                item: id,
            }
            .into_element(),
        };
        rect()
            .width(Size::fill())
            .height(Size::fill())
            .spacing(12.0)
            .content(Content::Flex)
            .child(head)
            .child(
                ScrollView::new()
                    .width(Size::fill())
                    .height(Size::flex(1.0))
                    .child(
                        rect()
                            .width(Size::fill())
                            .padding((0.0, 0.0, 24.0, 0.0))
                            .child(body),
                    ),
            )
            .into_element()
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Links {
    data: Arc<LibraryData>,
    item: ItemId,
}

impl Links {
    fn row(&self, other: ItemId) -> Option<Element> {
        let p = palette();
        let found = self.data.item(other).filter(|i| i.trashed.is_none())?;
        let info = crate::plugins::kind_info(&found.kind.plugin_id);
        Some(
            ListRow::new(found.name.clone())
                .kind(
                    info.as_ref()
                        .map_or_else(freya::icons::lucide::file, |k| k.icon.clone()),
                    info.map_or(p.t3, |k| kind_color(&p, k.color)),
                )
                .dense(true)
                .on_press(move |_| crate::flows::open_beside(other))
                .into_element(),
        )
    }
}

impl Component for Links {
    fn render(&self) -> impl IntoElement {
        use freya::icons::lucide;
        let item = self.item;
        let notebooks: Vec<Element> = self
            .data
            .companions_of(item)
            .filter_map(|n| self.row(n))
            .collect();
        let linked: Vec<Element> = self
            .data
            .links_of(item)
            .filter(|l| l.kind == LinkKind::Related)
            .map(|l| if l.from == item { l.to } else { l.from })
            .filter_map(|other| self.row(other))
            .collect();
        let mut mentions: Vec<ItemId> = refs::refs_to(item).into_iter().map(|r| r.from).collect();
        mentions.dedup();
        let mentions: Vec<Element> = mentions.into_iter().filter_map(|m| self.row(m)).collect();
        let broken: Vec<Element> = refs::refs_from(item)
            .into_iter()
            .filter(|r| self.data.item(r.to).is_none_or(|i| i.trashed.is_some()))
            .map(|r| {
                ListRow::new("Referencia a un documento que ya no está")
                    .subtitle(match r.origin {
                        refs::RefOrigin::Bookmark(_) => "Desde un marcador",
                        refs::RefOrigin::TextBox => "Desde un cuadro de texto",
                        refs::RefOrigin::Note => "Desde el texto de la nota",
                    })
                    .icon(lucide::unlink())
                    .danger(true)
                    .dense(true)
                    .into_element()
            })
            .collect();
        let section = |title: &'static str,
                       rows: Vec<Element>,
                       empty: &'static str,
                       action: Option<Element>| {
            let none = rows.is_empty();
            Section::new(title)
                .maybe_child(none.then(|| Text::new(empty).role(TextRole::Hint)))
                .maybe_child((!none).then(|| Group::default().children(rows)))
                .maybe_child(action)
                .into_element()
        };
        rect()
            .width(Size::fill())
            .spacing(18.0)
            .child(section(
                "Cuadernos",
                notebooks,
                "Sin cuadernos todavía.",
                Some(
                    Button::new("Nuevo cuaderno")
                        .ghost()
                        .small()
                        .icon(lucide::plus())
                        .on_press(move |_| crate::flows::notebook(item))
                        .into_element(),
                ),
            ))
            .child(section(
                "Vinculados",
                linked,
                "No está vinculado con nada.",
                Some(
                    Button::new("Vincular con…")
                        .ghost()
                        .small()
                        .icon(lucide::link())
                        .on_press(move |_| crate::flows::link_with(item))
                        .into_element(),
                ),
            ))
            .child(section(
                "Lo mencionan",
                mentions,
                "Ningún documento lo cita todavía.",
                None,
            ))
            .maybe_child(
                (!broken.is_empty()).then(|| section("Referencias rotas", broken, "", None)),
            )
    }
}
