//! The document panel beside the panes: what the focused document
//! contains, its bookmarks, and what it is connected to.

use std::sync::Arc;

use freya::prelude::*;
use notein_i18n::tr;
use notein_plugin::bookmark::SourceRef;
use notein_plugin::services::Placement;
use notein_ui_kit::theme::palette;
use notein_ui_kit::{
    Button, EmptyState, Group, ListRow, SearchField, Section, Segmented, Sheet, Tag, Text,
    TextRole, tokens,
};

mod outline;

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

    fn label(self) -> String {
        match self {
            Self::Contents => tr!("mesa-panel-contents"),
            Self::Bookmarks => tr!("mesa-panel-bookmarks"),
            Self::Links => tr!("mesa-panel-links"),
        }
    }
}

#[sismo::function("mesa::panel_jump")]
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

/// The docked panel.
#[derive(Clone, PartialEq)]
pub(super) struct Panel {
    pub(super) data: Arc<LibraryData>,
    pub(super) focus: Option<Focus>,
    pub(super) tab: State<PanelTab>,
}

impl Component for Panel {
    #[sismo::function("mesa::panel")]
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
                in_sheet: false,
            })
    }
}

/// The panel as a sheet, on phones.
#[sismo::function("mesa::panel_open_sheet")]
pub(super) fn open_sheet(data: Arc<LibraryData>, focus: Option<&Focus>, start: PanelTab) {
    let tab = State::create_in_scope(start, ScopeId::ROOT);
    Sheet::titled(tr!("mesa-panel-sheet-title"))
        .child(PanelBody {
            data,
            item: focus.and_then(Focus::item),
            tab,
            in_sheet: true,
        })
        .open();
}

#[derive(Clone, PartialEq)]
struct PanelBody {
    data: Arc<LibraryData>,
    item: Option<ItemId>,
    tab: State<PanelTab>,
    /// In the phone sheet, which scrolls on its own.
    in_sheet: bool,
}

/// The tabs and, except on Vínculos, a search field.
#[sismo::function("mesa::panel_panel_head")]
fn panel_head(mut tab: State<PanelTab>, query: State<String>) -> Rect {
    let current = *tab.read();
    let index = PanelTab::ALL
        .iter()
        .position(|t| *t == current)
        .unwrap_or(0);
    rect()
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
                    tr!("mesa-panel-search-contents")
                } else {
                    tr!("mesa-panel-search-bookmarks")
                },
            )
            .height(34.0)
        }))
}

/// The document's marks, newest first, filtered by colour and `text`.
#[sismo::function("mesa::panel_bookmarks")]
fn bookmarks(
    marks: Vec<notein_plugin::kind::Mark<Vec<u8>>>,
    text: &str,
    kind_id: &str,
    id: ItemId,
    mut color: State<Option<[u8; 3]>>,
) -> Element {
    let p = palette();
    let mut colors: Vec<[u8; 3]> = marks.iter().filter_map(|m| m.color).collect();
    colors.sort_unstable();
    colors.dedup();
    let chosen = *color.read();
    let mut marks: Vec<_> = marks
        .into_iter()
        .filter(|m| chosen.is_none() || m.color == chosen)
        .filter(|m| {
            text.is_empty()
                || m.title.to_lowercase().contains(text)
                || m.snippet.to_lowercase().contains(text)
        })
        .collect();
    marks.sort_by_key(|m| std::cmp::Reverse(m.updated_at));
    let rows: Vec<Element> = marks
        .into_iter()
        .map(|mark| {
            let anchor = mark.anchor.clone();
            let kind_id = kind_id.to_owned();
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
                .child(
                    Tag::new(tr!("mesa-panel-bookmarks-all"))
                        .active(chosen.is_none())
                        .on_press(move |_| color.set(None)),
                )
                .children(colors.into_iter().map(|c| {
                    Tag::new("")
                        .dot(Color::from_rgb(c[0], c[1], c[2]))
                        .active(chosen == Some(c))
                        .on_press(move |_| color.set(Some(c)))
                        .into_element()
                }))
        }))
        .child(if rows.is_empty() {
            Text::new(tr!("mesa-panel-bookmarks-empty"))
                .role(TextRole::Hint)
                .into_element()
        } else {
            Group::default().children(rows).into_element()
        })
        .into_element()
}

impl Component for PanelBody {
    #[sismo::function("mesa::panel_body")]
    fn render(&self) -> impl IntoElement {
        let current = *self.tab.read();
        let query = use_state(String::new);
        let color = use_state(|| None::<[u8; 3]>);
        let folds = use_state(outline::Folds::default);
        let cache = use_hook(outline::OutlineCache::default);
        let text = query.read().trim().to_lowercase();
        let head = panel_head(self.tab, query);
        let Some(item) = self.item.and_then(|id| self.data.item(id)) else {
            return rect()
                .width(Size::fill())
                .spacing(12.0)
                .child(head)
                .child(
                    EmptyState::new(
                        freya::icons::lucide::panel_left(),
                        tr!("mesa-panel-none-title"),
                    )
                    .body(tr!("mesa-panel-none-body")),
                )
                .into_element();
        };
        let id = item.id;
        let kind_id = item.kind.plugin_id.clone();
        let kind = crate::plugins::kind(&kind_id);
        let content = crate::plugins::content::read(id, &kind_id)
            .unwrap_or_else(|| kind.as_ref().map(|k| k.empty_content()).unwrap_or_default());
        let page = rect()
            .width(Size::fill())
            .height(Size::fill())
            .spacing(12.0)
            .content(Content::Flex)
            .child(head);
        let body = match current {
            // The outline scrolls on its own and builds only its visible rows.
            PanelTab::Contents => {
                let outline = cache.get(id, &content, || {
                    kind.as_ref()
                        .map(|k| k.outline(&content))
                        .unwrap_or_default()
                });
                return page
                    .child(
                        rect().width(Size::fill()).height(Size::flex(1.0)).child(
                            outline::OutlineList {
                                outline,
                                query: text,
                                kind: kind_id,
                                item: id,
                                folds,
                                max_height: self
                                    .in_sheet
                                    .then(|| notein_ui_kit::safe_frame().height() * 0.6),
                            },
                        ),
                    )
                    .into_element();
            }
            PanelTab::Bookmarks => {
                let marks = kind.as_ref().map(|k| k.marks(&content)).unwrap_or_default();
                bookmarks(marks, &text, &kind_id, id, color)
            }
            PanelTab::Links => Links {
                data: Arc::clone(&self.data),
                item: id,
            }
            .into_element(),
        };
        page.child(
            ScrollView::new()
                .width(Size::fill())
                .height(Size::flex(1.0))
                .child(notein_ui_kit::ScrollContent::new(
                    rect()
                        .width(Size::fill())
                        .padding((0.0, 0.0, 24.0, 0.0))
                        .child(body),
                )),
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
    #[sismo::function("mesa::panel_row")]
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
    #[sismo::function("mesa::links")]
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
                ListRow::new(tr!("mesa-links-broken-row"))
                    .subtitle(match r.origin {
                        refs::RefOrigin::Bookmark(_) => tr!("mesa-links-from-bookmark"),
                        refs::RefOrigin::TextBox => tr!("mesa-links-from-text-box"),
                        refs::RefOrigin::Note => tr!("mesa-links-from-note"),
                    })
                    .icon(lucide::unlink())
                    .danger(true)
                    .dense(true)
                    .into_element()
            })
            .collect();
        let section =
            |title: String, rows: Vec<Element>, empty: String, action: Option<Element>| {
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
                tr!("mesa-links-notebooks"),
                notebooks,
                tr!("mesa-links-notebooks-empty"),
                Some(
                    Button::new(tr!("mesa-links-new-notebook"))
                        .ghost()
                        .small()
                        .icon(lucide::plus())
                        .on_press(move |_| crate::flows::notebook(item))
                        .into_element(),
                ),
            ))
            .child(section(
                tr!("mesa-links-linked"),
                linked,
                tr!("mesa-links-linked-empty"),
                Some(
                    Button::new(tr!("mesa-cmd-link-with"))
                        .ghost()
                        .small()
                        .icon(lucide::link())
                        .on_press(move |_| crate::flows::link_with(item))
                        .into_element(),
                ),
            ))
            .child(section(
                tr!("mesa-links-mentions"),
                mentions,
                tr!("mesa-links-mentions-empty"),
                None,
            ))
            .maybe_child(
                (!broken.is_empty())
                    .then(|| section(tr!("mesa-links-broken"), broken, String::new(), None)),
            )
    }
}
