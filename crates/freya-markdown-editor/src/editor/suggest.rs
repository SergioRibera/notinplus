//! Character triggered suggestion menus (`/` commands, `[` references, `@`
//! mentions...).

use std::fmt;
use std::rc::Rc;

use freya::prelude::*;

use crate::document::{BlockKind, BlockType, CalloutKind, HeadingLevel, Inline, Table};
use crate::theme::EditorTheme;

/// What picking a suggestion does. The trigger char and the typed query are
/// removed first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuggestionAction {
    InsertText(String),
    InsertInline(Inline),
    InsertLink { label: String, url: String },
    InsertEmbed { label: String, url: String },
    SetBlockType(BlockType),
    InsertBlock(BlockKind),
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
}

type Provider = Rc<dyn Fn(&str) -> Vec<Suggestion>>;

/// Opens a suggestion menu when `symbol` is typed. The provider receives
/// the text typed after the symbol and returns the entries to show.
#[derive(Clone)]
pub struct Trigger {
    pub(crate) symbol: char,
    pub(crate) word_start: bool,
    pub(crate) provider: Provider,
}

impl fmt::Debug for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Trigger")
            .field("symbol", &self.symbol)
            .field("word_start", &self.word_start)
            .finish_non_exhaustive()
    }
}

impl Trigger {
    /// The menu only opens at the start of a word unless [`Self::anywhere`]
    /// is set.
    #[must_use]
    pub fn new(symbol: char, provider: impl Fn(&str) -> Vec<Suggestion> + 'static) -> Self {
        Self {
            symbol,
            word_start: true,
            provider: Rc::new(provider),
        }
    }

    /// Also open the menu in the middle of a word.
    #[must_use]
    pub const fn anywhere(mut self) -> Self {
        self.word_start = false;
        self
    }

    /// Built in `/` menu to turn the current block into another kind or
    /// insert dividers and tables.
    #[must_use]
    pub fn slash_commands() -> Self {
        Self::new('/', |query| {
            let query = query.to_lowercase();
            slash_entries()
                .into_iter()
                .filter(|entry| {
                    query.is_empty()
                        || entry.label.to_lowercase().contains(&query)
                        || entry
                            .detail
                            .as_ref()
                            .is_some_and(|detail| detail.to_lowercase().contains(&query))
                })
                .collect()
        })
    }
}

fn slash_entries() -> Vec<Suggestion> {
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

/// Set of triggers shared by every text block; compared by identity.
#[derive(Clone, Default)]
pub(crate) struct Triggers(pub(crate) Rc<Vec<Trigger>>);

impl PartialEq for Triggers {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Triggers {
    pub(crate) fn find(&self, symbol: char) -> Option<(usize, &Trigger)> {
        self.0
            .iter()
            .enumerate()
            .find(|(_, trigger)| trigger.symbol == symbol)
    }
}

/// An open menu inside one text block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OpenMenu {
    pub(crate) trigger: usize,
    /// UTF-16 offset of the trigger symbol.
    pub(crate) at: usize,
    pub(crate) selected: usize,
}

const MENU_LIMIT: usize = 40;
const MENU_WIDTH: f32 = 260.0;
const MENU_MAX_HEIGHT: f32 = 280.0;
const ROW_HEIGHT: f32 = 42.0;

pub(crate) fn query_entries(triggers: &Triggers, menu: OpenMenu, query: &str) -> Vec<Suggestion> {
    triggers
        .0
        .get(menu.trigger)
        .map_or_else(Vec::new, |trigger| {
            let mut entries = (trigger.provider)(query);
            entries.truncate(MENU_LIMIT);
            entries
        })
}

pub(crate) fn menu_popup(
    entries: &[Suggestion],
    selected: usize,
    top: f32,
    theme: &EditorTheme,
    on_pick: impl Fn(usize) + Clone + 'static,
) -> Element {
    let colors = &theme.colors;
    let rows: Vec<Element> = if entries.is_empty() {
        vec![
            rect()
                .padding((8., 12.))
                .child(
                    label()
                        .color(colors.muted)
                        .font_size(13.)
                        .text("No matches"),
                )
                .into_element(),
        ]
    } else {
        entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let on_pick = on_pick.clone();
                menu_row(entry, index == selected, theme, move || on_pick(index))
                    .key(index)
                    .into_element()
            })
            .collect()
    };

    #[allow(
        clippy::cast_precision_loss,
        reason = "entry count is capped by MENU_LIMIT"
    )]
    let height = (entries.len().max(1) as f32)
        .mul_add(ROW_HEIGHT, 10.)
        .min(MENU_MAX_HEIGHT);
    rect()
        .width(Size::fill())
        .height(Size::px(0.))
        .child(
            rect()
                .layer(Layer::Overlay)
                .position(Position::new_absolute().top(top).left(0.))
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
                .child(
                    ScrollView::new()
                        .direction(Direction::Vertical)
                        .show_scrollbar(false)
                        .width(Size::fill())
                        .height(Size::fill())
                        .children(rows),
                ),
        )
        .into_element()
}

fn menu_row(
    entry: &Suggestion,
    selected: bool,
    theme: &EditorTheme,
    on_pick: impl Fn() + 'static,
) -> Rect {
    let colors = &theme.colors;
    rect()
        .width(Size::fill())
        .padding((6., 10.))
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
        .on_pointer_down(move |event: Event<PointerEventData>| {
            event.stop_propagation();
            on_pick();
        })
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
