//! The `/` suggestion menu and its extension points.
//!
//! Typing `/` at the start of a word opens one menu that aggregates every
//! registered [`SuggestionSource`]: the built in [`BlockSuggestions`] plus
//! whatever the integrator adds (references, mentions, snippets...). Each
//! source becomes a titled section. A source with a [`keyword`] can be
//! targeted directly: `/ref romans` only asks the source keyed `ref`.
//!
//! [`keyword`]: SuggestionSource::keyword

use std::borrow::Cow;
use std::cell::Cell;
use std::fmt;
use std::rc::Rc;

use freya::prelude::*;

use crate::EditorController;
use crate::document::{BlockKind, BlockType, CalloutKind, HeadingLevel, Inline, Table};
use crate::theme::EditorTheme;

/// What picking a suggestion does. The `/` and the typed query are removed
/// first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuggestionAction {
    InsertText(String),
    InsertInline(Inline),
    InsertLink {
        label: String,
        url: String,
    },
    InsertEmbed {
        label: String,
        url: String,
    },
    SetBlockType(BlockType),
    InsertBlock(BlockKind),
    /// Anything else, done through the controller: update data the
    /// suggestion depends on, then insert whatever fits.
    Run(SuggestionRun),
}

/// Custom suggestion action, see [`SuggestionAction::Run`].
#[derive(Clone)]
pub struct SuggestionRun(Rc<dyn Fn(EditorController)>);

impl SuggestionRun {
    pub fn new(run: impl Fn(EditorController) + 'static) -> Self {
        Self(Rc::new(run))
    }

    pub(crate) fn call(&self, controller: EditorController) {
        (self.0)(controller);
    }
}

impl PartialEq for SuggestionRun {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for SuggestionRun {}

impl fmt::Debug for SuggestionRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SuggestionRun")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub label: String,
    pub detail: Option<String>,
    /// Short glyph shown before the label.
    pub icon: Option<String>,
    pub action: SuggestionAction,
}

impl Suggestion {
    #[must_use]
    pub fn new(label: impl Into<String>, action: SuggestionAction) -> Self {
        Self {
            label: label.into(),
            detail: None,
            icon: None,
            action,
        }
    }

    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    #[must_use]
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    fn matches(&self, query: &str) -> bool {
        let contains = |text: &str| text.to_lowercase().contains(query);
        query.is_empty()
            || contains(&self.label)
            || self.detail.as_deref().is_some_and(contains)
            || self.icon.as_deref().is_some_and(contains)
    }
}

/// An extension of the `/` menu.
pub trait SuggestionSource {
    /// Section title shown above this source's entries.
    fn title(&self) -> &str;

    /// Word that scopes the menu to this source: `/<keyword> query`.
    fn keyword(&self) -> Option<&str> {
        None
    }

    /// Entries for the text typed after `/` (lowercased, keyword removed).
    fn suggestions(&self, query: &str) -> Vec<Suggestion>;
}

/// Built in block conversions and inserts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BlockSuggestions;

impl SuggestionSource for BlockSuggestions {
    fn title(&self) -> &'static str {
        "Blocks"
    }

    fn keyword(&self) -> Option<&str> {
        Some("block")
    }

    fn suggestions(&self, query: &str) -> Vec<Suggestion> {
        block_entries()
            .into_iter()
            .filter(|entry| entry.matches(query))
            .collect()
    }
}

type Provider = Rc<dyn Fn(&str) -> Vec<Suggestion>>;

/// A [`SuggestionSource`] built from a closure.
///
/// ```rust,ignore
/// SuggestionGroup::new("People", |query| people_matching(query)).keyword("at")
/// ```
#[derive(Clone)]
pub struct SuggestionGroup {
    title: Cow<'static, str>,
    keyword: Option<Cow<'static, str>>,
    provider: Provider,
}

impl fmt::Debug for SuggestionGroup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SuggestionGroup")
            .field("title", &self.title)
            .field("keyword", &self.keyword)
            .finish_non_exhaustive()
    }
}

impl SuggestionGroup {
    #[must_use]
    pub fn new(
        title: impl Into<Cow<'static, str>>,
        provider: impl Fn(&str) -> Vec<Suggestion> + 'static,
    ) -> Self {
        Self {
            title: title.into(),
            keyword: None,
            provider: Rc::new(provider),
        }
    }

    #[must_use]
    pub fn keyword(mut self, keyword: impl Into<Cow<'static, str>>) -> Self {
        self.keyword = Some(keyword.into());
        self
    }
}

impl SuggestionSource for SuggestionGroup {
    fn title(&self) -> &str {
        &self.title
    }

    fn keyword(&self) -> Option<&str> {
        self.keyword.as_deref()
    }

    fn suggestions(&self, query: &str) -> Vec<Suggestion> {
        (self.provider)(query)
    }
}

fn block_entries() -> Vec<Suggestion> {
    let block = |label: &str, icon: &str, detail: &str, block_type| {
        Suggestion::new(label, SuggestionAction::SetBlockType(block_type))
            .icon(icon)
            .detail(detail)
    };
    let insert = |label: &str, icon: &str, detail: &str, kind| {
        Suggestion::new(label, SuggestionAction::InsertBlock(kind))
            .icon(icon)
            .detail(detail)
    };
    vec![
        block("Text", "¶", "Plain paragraph", BlockType::Paragraph),
        block(
            "Heading 1",
            "H1",
            "Big section heading",
            BlockType::Heading(HeadingLevel::H1),
        ),
        block(
            "Heading 2",
            "H2",
            "Medium section heading",
            BlockType::Heading(HeadingLevel::H2),
        ),
        block(
            "Heading 3",
            "H3",
            "Small section heading",
            BlockType::Heading(HeadingLevel::H3),
        ),
        block("Bulleted list", "•", "Simple list", BlockType::BulletList),
        block(
            "Numbered list",
            "1.",
            "Ordered list",
            BlockType::OrderedList,
        ),
        block("To-do list", "☐", "Checklist", BlockType::TaskList),
        block("Quote", "❝", "Quotation", BlockType::Quote),
        block(
            "Note",
            "ℹ",
            "Callout",
            BlockType::Callout(CalloutKind::Note),
        ),
        block("Tip", "✦", "Callout", BlockType::Callout(CalloutKind::Tip)),
        block(
            "Warning",
            "⚠",
            "Callout",
            BlockType::Callout(CalloutKind::Warning),
        ),
        block("Code", "</>", "Code block", BlockType::Code),
        block("Math", "∑", "TeX formula", BlockType::Math),
        block("Term", "Dt", "Definition list term", BlockType::Term),
        block(
            "Definition",
            "Dd",
            "Definition list details",
            BlockType::Definition,
        ),
        insert("Divider", "—", "Horizontal rule", BlockKind::Divider),
        insert(
            "Table",
            "▦",
            "3 x 3 table",
            BlockKind::Table(Table::new(3, 3)),
        ),
        insert(
            "Table of contents",
            "≡",
            "Generated from headings",
            BlockKind::TableOfContents,
        ),
    ]
}

/// Every registered source, compared by identity.
#[derive(Clone, Default)]
pub(crate) struct SlashMenu(pub(crate) Rc<Vec<Rc<dyn SuggestionSource>>>);

impl PartialEq for SlashMenu {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// Entries kept per section so a broad query stays responsive.
const SECTION_LIMIT: usize = 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MenuRow {
    Header(String),
    Item(Suggestion),
}

impl SlashMenu {
    pub(crate) fn is_enabled(&self) -> bool {
        !self.0.is_empty()
    }

    /// Rows for `query` (the text typed after `/`).
    pub(crate) fn rows(&self, query: &str) -> Vec<MenuRow> {
        let query = query.trim_start().to_lowercase();
        let scoped = self.0.iter().find_map(|source| {
            let keyword = source.keyword()?.to_lowercase();
            let rest = query.strip_prefix(&keyword)?;
            (rest.is_empty() || rest.starts_with(' ')).then(|| (source, rest.trim().to_owned()))
        });
        let sections: Vec<(&str, Vec<Suggestion>)> = match scoped {
            Some((source, rest)) => vec![(source.title(), source.suggestions(&rest))],
            None => self
                .0
                .iter()
                .map(|source| (source.title(), source.suggestions(&query)))
                .collect(),
        };
        let non_empty: Vec<_> = sections
            .into_iter()
            .filter(|(_, entries)| !entries.is_empty())
            .collect();
        let headers = non_empty.len() > 1;
        non_empty
            .into_iter()
            .flat_map(|(title, entries)| {
                headers
                    .then(|| MenuRow::Header(title.to_owned()))
                    .into_iter()
                    .chain(entries.into_iter().take(SECTION_LIMIT).map(MenuRow::Item))
            })
            .collect()
    }
}

/// The menu open inside one text block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct OpenMenu {
    /// UTF-16 offset of the `/`.
    pub(crate) at: usize,
    /// Index among selectable items (headers excluded).
    pub(crate) selected: usize,
    /// Popup position relative to the paragraph, fixed when it opens so it
    /// does not move while typing.
    pub(crate) top: f32,
    pub(crate) left: f32,
}

pub(crate) const MENU_WIDTH: f32 = 280.0;
const MENU_MAX_HEIGHT: f32 = 300.0;
const ROW_HEIGHT: f32 = 40.0;

pub(crate) fn item_count(rows: &[MenuRow]) -> usize {
    rows.iter()
        .filter(|row| matches!(row, MenuRow::Item(_)))
        .count()
}

pub(crate) fn item(rows: &[MenuRow], index: usize) -> Option<&Suggestion> {
    rows.iter()
        .filter_map(|row| match row {
            MenuRow::Item(item) => Some(item),
            MenuRow::Header(_) => None,
        })
        .nth(index)
}

fn row_of_item(rows: &[MenuRow], index: usize) -> usize {
    rows.iter()
        .enumerate()
        .filter(|(_, row)| matches!(row, MenuRow::Item(_)))
        .nth(index)
        .map_or(0, |(row, _)| row)
}

/// Floating list of suggestions. Picks happen on press (not pointer down)
/// and are ignored when the list scrolled in between, so touch users can
/// drag through it.
#[derive(Clone, PartialEq)]
pub(crate) struct SuggestionMenu {
    pub(crate) rows: Rc<Vec<MenuRow>>,
    pub(crate) selected: usize,
    pub(crate) theme: Rc<EditorTheme>,
    pub(crate) on_pick: EventHandler<usize>,
}

impl Component for SuggestionMenu {
    fn render(&self) -> impl IntoElement {
        let mut scroll = use_scroll_controller(ScrollConfig::default);
        let pressed_at = use_hook(|| Rc::new(Cell::new(i32::MIN)));
        let shown_selected = use_hook(|| Rc::new(Cell::new(usize::MAX)));
        let colors = self.theme.colors.clone();
        let rows = self.rows.clone();

        #[allow(
            clippy::cast_precision_loss,
            reason = "row count is capped per section"
        )]
        let height = (rows.len().max(1) as f32)
            .mul_add(ROW_HEIGHT, 8.)
            .min(MENU_MAX_HEIGHT);

        if shown_selected.get() != self.selected {
            shown_selected.set(self.selected);
            keep_visible(&mut scroll, row_of_item(&rows, self.selected), height);
        }

        if rows.is_empty() {
            return frame(&colors, 44.)
                .child(
                    rect().padding((10., 12.)).child(
                        label()
                            .color(colors.muted)
                            .font_size(13.)
                            .text("No matches"),
                    ),
                )
                .into_element();
        }

        let selected_row = row_of_item(&rows, self.selected);
        let on_pick = self.on_pick.clone();
        let item_indices: Rc<Vec<Option<usize>>> = Rc::new({
            let mut next = 0;
            rows.iter()
                .map(|row| match row {
                    MenuRow::Item(_) => {
                        next += 1;
                        Some(next - 1)
                    }
                    MenuRow::Header(_) => None,
                })
                .collect()
        });
        let length = rows.len();
        let row_colors = colors.clone();
        let list = VirtualScrollView::new_controlled(
            move |virtual_item, ()| {
                let index = virtual_item.index;
                match &rows[index] {
                    MenuRow::Header(title) => rect()
                        .key(index)
                        .height(Size::px(ROW_HEIGHT))
                        .width(Size::fill())
                        .padding((16., 12., 4., 12.))
                        .child(
                            label()
                                .color(row_colors.muted)
                                .font_size(11.)
                                .font_weight(FontWeight::BOLD)
                                .text(title.to_uppercase()),
                        )
                        .into_element(),
                    MenuRow::Item(entry) => {
                        let item = item_indices[index].unwrap_or_default();
                        let on_pick = on_pick.clone();
                        let down_at = pressed_at.clone();
                        let press_at = pressed_at.clone();
                        menu_row(entry, index == selected_row, &row_colors)
                            .key(index)
                            .on_pointer_down(move |_| {
                                let (_, y): (i32, i32) = scroll.into();
                                down_at.set(y);
                            })
                            .on_press(move |event: Event<PressEventData>| {
                                event.stop_propagation();
                                let (_, y): (i32, i32) = scroll.into();
                                if press_at.get() == y {
                                    on_pick.call(item);
                                }
                            })
                            .into_element()
                    }
                }
            },
            scroll,
        )
        .length(length)
        .item_size(ROW_HEIGHT)
        .show_scrollbar(false)
        .width(Size::fill())
        .height(Size::fill());

        frame(&colors, height).child(list).into_element()
    }
}

/// Scroll so `row` is inside a list of the given `height`.
fn keep_visible(scroll: &mut ScrollController, row: usize, height: f32) {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_precision_loss,
        reason = "pixel offsets of a short list"
    )]
    let (top, bottom, visible) = (
        (row as f32 * ROW_HEIGHT) as i32,
        ((row + 1) as f32 * ROW_HEIGHT) as i32,
        (height - 8.) as i32,
    );
    let (_, current): (i32, i32) = (*scroll).into();
    let offset = -current;
    if top < offset {
        scroll.scroll_to_y(-top);
    } else if bottom > offset + visible {
        scroll.scroll_to_y(-(bottom - visible));
    }
}

fn frame(colors: &crate::EditorColors, height: f32) -> Rect {
    rect()
        .width(Size::px(MENU_WIDTH))
        .height(Size::px(height))
        .overflow(Overflow::Clip)
        .padding(4.)
        .background(colors.popup_background)
        .corner_radius(10.)
        .border(
            Border::new()
                .width(1.)
                .alignment(BorderAlignment::Inner)
                .fill(colors.border),
        )
        .shadow(
            Shadow::new()
                .y(6.)
                .blur(18.)
                .color(Color::from_argb(70, 0, 0, 0)),
        )
}

fn menu_row(entry: &Suggestion, selected: bool, colors: &crate::EditorColors) -> Rect {
    rect()
        .width(Size::fill())
        .height(Size::px(ROW_HEIGHT))
        .padding((4., 10.))
        .corner_radius(6.)
        .background(if selected {
            colors.popup_selected
        } else {
            Color::TRANSPARENT
        })
        .direction(Direction::Horizontal)
        .content(Content::flex())
        .cross_align(Alignment::center())
        .spacing(10.)
        .cursor(CursorIcon::Pointer)
        .maybe_child(entry.icon.clone().map(|icon| {
            rect()
                .width(Size::px(26.))
                .center()
                .child(label().color(colors.muted).font_size(13.).text(icon))
        }))
        .child(
            rect()
                .width(Size::flex(1.))
                .child(
                    label()
                        .color(colors.text)
                        .font_size(13.5)
                        .max_lines(1)
                        .text(entry.label.clone()),
                )
                .maybe_child(entry.detail.clone().map(|detail| {
                    label()
                        .color(colors.muted)
                        .font_size(11.5)
                        .max_lines(1)
                        .text(detail)
                })),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu() -> SlashMenu {
        let people = SuggestionGroup::new("People", |query| {
            ["Ada", "Alan"]
                .into_iter()
                .filter(|name| name.to_lowercase().contains(query))
                .map(|name| Suggestion::new(name, SuggestionAction::InsertText(name.into())))
                .collect()
        })
        .keyword("at");
        SlashMenu(Rc::new(vec![Rc::new(BlockSuggestions), Rc::new(people)]))
    }

    #[test]
    fn sections_get_headers_when_several_match() {
        let rows = menu().rows("a");
        assert!(rows.contains(&MenuRow::Header("People".into())));
        assert!(rows.contains(&MenuRow::Header("Blocks".into())));
    }

    #[test]
    fn single_section_has_no_header() {
        let rows = menu().rows("heading 2");
        assert_eq!(item_count(&rows), 1);
        assert!(!rows.iter().any(|row| matches!(row, MenuRow::Header(_))));
    }

    #[test]
    fn keyword_scopes_to_one_source() {
        let rows = menu().rows("at al");
        assert_eq!(item_count(&rows), 1);
        assert_eq!(item(&rows, 0).map(|item| item.label.as_str()), Some("Alan"));
    }
}
