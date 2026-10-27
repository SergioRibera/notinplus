//! Rendering of a single block: gutter, container chrome and the text
//! editors it holds.
//!
//! Every block with a single text run renders through [`Frame`] so the
//! element tree keeps the same shape when the block changes kind. That lets
//! the [`TextBlock`] (and its focus and caret) survive conversions like
//! paragraph to heading or list item.

use freya::prelude::*;

use super::Shared;
use super::text::{TextBlock, TextLook, TextPlace};
use crate::controller::CaretRequest;
use crate::decorator::{CodeContext, TransclusionContext};
use crate::document::{
    Align, Block, BlockId, BlockKind, CalloutKind, CellPos, ColumnAlign, HeadingLevel, Inline,
    ListKind, Table, TextRole, TextTarget, Transclusion,
};
use crate::labels::Label;
use crate::theme::EditorTheme;

const GUTTER: f32 = 24.0;

/// Heading entry shown by a table of contents block.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TocEntry {
    pub(crate) block: BlockId,
    pub(crate) level: HeadingLevel,
    pub(crate) text: String,
}

#[derive(Clone, PartialEq)]
pub(crate) struct BlockView {
    pub(crate) block: Block,
    pub(crate) ordinal: Option<u32>,
    pub(crate) caret: Option<CaretRequest>,
    pub(crate) focused: Option<TextTarget>,
    pub(crate) toc: Vec<TocEntry>,
    pub(crate) placeholder: Option<String>,
    pub(crate) shared: Shared,
}

/// Chrome around a text run.
struct Frame {
    indent: f32,
    padding: (f32, f32, f32, f32),
    background: Color,
    bar: Option<Color>,
    radius: f32,
    gutter: Option<Element>,
    header: Option<Element>,
    text: TextBlock,
}

impl Frame {
    const fn new(text: TextBlock) -> Self {
        Self {
            indent: 0.,
            padding: (0., 0., 0., 0.),
            background: Color::TRANSPARENT,
            bar: None,
            radius: 0.,
            gutter: None,
            header: None,
            text,
        }
    }

    fn render(self, theme: &EditorTheme) -> Element {
        let gutter_width = if self.gutter.is_some() {
            theme.px(GUTTER)
        } else {
            0.
        };
        let bar = self.bar.unwrap_or(Color::TRANSPARENT);
        let bar_width = if self.bar.is_some() { theme.px(3.) } else { 0. };
        rect()
            .width(Size::fill())
            .padding((0., 0., 0., self.indent))
            .child(
                rect()
                    .width(Size::fill())
                    .padding(self.padding)
                    .background(self.background)
                    .corner_radius(self.radius)
                    .border(
                        Border::new()
                            .width(BorderWidth {
                                top: 0.,
                                right: 0.,
                                bottom: 0.,
                                left: bar_width,
                            })
                            .alignment(BorderAlignment::Inner)
                            .fill(bar),
                    )
                    .child(
                        rect()
                            .horizontal()
                            .content(Content::flex())
                            .width(Size::fill())
                            .child(
                                rect()
                                    .width(Size::px(gutter_width))
                                    .padding((theme.px(2.), 0., 0., 0.))
                                    .cross_align(Alignment::center())
                                    .maybe_child(self.gutter),
                            )
                            .child(
                                rect()
                                    .width(Size::flex(1.))
                                    .child(rect().width(Size::fill()).maybe_child(self.header))
                                    .child(self.text),
                            ),
                    ),
            )
            .into_element()
    }
}

impl BlockView {
    fn text(
        &self,
        target: TextTarget,
        content: &Inline,
        role: TextRole,
        place: TextPlace,
        look: TextLook,
    ) -> TextBlock {
        TextBlock {
            target,
            content: content.clone(),
            role,
            place,
            look,
            caret: self
                .caret
                .filter(|request| request.selection.target == target),
            placeholder: self.placeholder.clone(),
            shared: self.shared.clone(),
        }
    }

    fn block_text(
        &self,
        content: &Inline,
        role: TextRole,
        place: TextPlace,
        look: TextLook,
    ) -> TextBlock {
        self.text(TextTarget::from(self.block.id), content, role, place, look)
    }

    fn look(&self) -> TextLook {
        TextLook {
            align: text_align(self.block.align),
            ..TextLook::default()
        }
    }

    fn marker(&self, kind: ListKind, depth: u8) -> Element {
        let theme = &self.shared.theme;
        let colors = &theme.colors;
        match kind {
            ListKind::Bullet => label()
                .color(colors.muted)
                .font_size(theme.body_size)
                .text(["•", "◦", "▪"][usize::from(depth) % 3])
                .into_element(),
            ListKind::Ordered { .. } => label()
                .color(colors.muted)
                .font_size(theme.body_size)
                .text(format!("{}.", self.ordinal.unwrap_or(1)))
                .into_element(),
            ListKind::Task { checked } => {
                let controller = self.shared.controller;
                let id = self.block.id;
                rect()
                    .width(Size::px(theme.px(16.)))
                    .height(Size::px(theme.px(16.)))
                    .margin((theme.px(2.), 0., 0., 0.))
                    .corner_radius(theme.px(4.))
                    .center()
                    .border(
                        Border::new()
                            .width(theme.px(1.5))
                            .alignment(BorderAlignment::Inner)
                            .fill(if checked { colors.accent } else { colors.muted }),
                    )
                    .background(if checked {
                        colors.accent
                    } else {
                        Color::TRANSPARENT
                    })
                    .cursor(CursorIcon::Pointer)
                    .on_press(move |_| controller.toggle_task(id))
                    .maybe_child(checked.then(|| {
                        label()
                            .font_size(theme.px(11.))
                            .color(colors.surface)
                            .font_weight(FontWeight::BOLD)
                            .text("✓")
                    }))
                    .into_element()
            }
        }
    }

    #[allow(clippy::too_many_lines, reason = "one arm per text block kind")]
    fn framed(&self) -> Option<Frame> {
        let theme = &self.shared.theme;
        let colors = &theme.colors;
        let rich = |content: &Inline, look: TextLook| {
            self.block_text(content, TextRole::Rich, TextPlace::Block, look)
        };
        let plain = |content: &Inline, color| {
            self.block_text(
                content,
                TextRole::Plain,
                TextPlace::Block,
                TextLook {
                    monospace: true,
                    font_size: Some(theme.body_size * 0.92),
                    color: Some(color),
                    ..TextLook::default()
                },
            )
        };
        let panel = |text: TextBlock, title: Option<String>| Frame {
            padding: (theme.px(8.), theme.px(12.), theme.px(8.), theme.px(12.)),
            background: colors.code_background,
            radius: theme.px(8.),
            header: title.map(|title| caption(title, colors.muted, theme)),
            ..Frame::new(text)
        };
        Some(match &self.block.kind {
            BlockKind::Paragraph(content) => Frame::new(rich(content, self.look())),
            BlockKind::Heading { level, content, .. } => Frame {
                padding: (theme.px(heading_gap(*level)), 0., theme.px(2.), 0.),
                ..Frame::new(rich(
                    content,
                    TextLook {
                        font_size: Some(theme.heading_size(*level)),
                        weight: Some(FontWeight::BOLD),
                        ..self.look()
                    },
                ))
            },
            BlockKind::Quote(content) => Frame {
                padding: (theme.px(2.), theme.px(12.), theme.px(2.), theme.px(12.)),
                bar: Some(colors.quote_bar),
                ..Frame::new(rich(
                    content,
                    TextLook {
                        color: Some(colors.muted),
                        italic: true,
                        ..self.look()
                    },
                ))
            },
            BlockKind::Callout {
                kind,
                title,
                content,
            } => {
                let accent = colors.callout(*kind);
                let header = rect()
                    .horizontal()
                    .spacing(theme.px(6.))
                    .margin((0., 0., theme.px(4.), 0.))
                    .cross_align(Alignment::center())
                    .child(
                        label()
                            .color(accent)
                            .font_size(theme.px(13.))
                            .text(callout_icon(*kind)),
                    )
                    .child(
                        label()
                            .color(accent)
                            .font_size(theme.px(13.))
                            .font_weight(FontWeight::BOLD)
                            .text(
                                title
                                    .clone()
                                    .unwrap_or_else(|| theme.labels.callout(*kind).to_owned()),
                            ),
                    )
                    .into_element();
                Frame {
                    padding: (theme.px(8.), theme.px(12.), theme.px(8.), theme.px(12.)),
                    background: accent.with_a(22),
                    bar: Some(accent),
                    radius: theme.px(4.),
                    header: Some(header),
                    ..Frame::new(rich(content, self.look()))
                }
            }
            BlockKind::ListItem {
                kind,
                depth,
                content,
            } => {
                let checked = matches!(kind, ListKind::Task { checked: true });
                Frame {
                    indent: f32::from(*depth) * theme.px(GUTTER),
                    gutter: Some(self.marker(*kind, *depth)),
                    ..Frame::new(self.block_text(
                        content,
                        TextRole::Rich,
                        TextPlace::ListItem,
                        TextLook {
                            color: checked.then_some(colors.muted),
                            ..self.look()
                        },
                    ))
                }
            }
            BlockKind::Code { language, code } => panel(
                plain(code, colors.text),
                (!language.is_empty()).then(|| language.clone()),
            ),
            BlockKind::Math(tex) => panel(plain(tex, colors.text), Some("TeX".to_owned())),
            BlockKind::FrontMatter(raw) => panel(
                plain(raw, colors.muted),
                Some(theme.labels.get(Label::FrontMatter).to_owned()),
            ),
            BlockKind::Html(raw) => panel(plain(raw, colors.muted), Some("HTML".to_owned())),
            BlockKind::Footnote {
                label: name,
                content,
            } => Frame {
                gutter: Some(
                    label()
                        .color(colors.accent)
                        .font_size(theme.body_size * 0.8)
                        .text(format!("[{name}]"))
                        .into_element(),
                ),
                ..Frame::new(rich(
                    content,
                    TextLook {
                        font_size: Some(theme.body_size * 0.9),
                        color: Some(colors.muted),
                        ..self.look()
                    },
                ))
            },
            BlockKind::Term(content) => Frame::new(rich(
                content,
                TextLook {
                    weight: Some(FontWeight::BOLD),
                    ..self.look()
                },
            )),
            BlockKind::Definition(content) => Frame {
                indent: theme.px(GUTTER),
                ..Frame::new(rich(content, self.look()))
            },
            BlockKind::Divider
            | BlockKind::Table(_)
            | BlockKind::TableOfContents
            | BlockKind::Transclusion(_) => return None,
        })
    }

    /// Read only replacement for code and math blocks from decorators.
    fn decorated(&self) -> Option<Element> {
        if !self.shared.read_only {
            return None;
        }
        let (language, code) = match &self.block.kind {
            BlockKind::Code { language, code } => (language.as_str(), code.text()),
            BlockKind::Math(tex) => ("math", tex.text()),
            _ => return None,
        };
        self.shared.decorators.render_code_block(&CodeContext {
            language,
            code,
            theme: &self.shared.theme,
        })
    }

    fn table(&self, table: &Table) -> Element {
        let theme = &self.shared.theme;
        let colors = &theme.colors;
        let controller = self.shared.controller;
        let in_table = self
            .focused
            .is_some_and(|target| target.block == self.block.id && target.cell.is_some());
        let rows = table.rows().iter().enumerate().map(|(row, cells)| {
            let header = row == 0;
            rect()
                .key(row)
                .horizontal()
                .content(Content::flex())
                .width(Size::fill())
                .background(if header {
                    colors.code_background
                } else {
                    Color::TRANSPARENT
                })
                .children(cells.iter().enumerate().map(|(column, cell)| {
                    let target = TextTarget {
                        block: self.block.id,
                        cell: Some(CellPos { row, column }),
                    };
                    let align = match table.columns().get(column) {
                        Some(ColumnAlign::Center) => TextAlign::Center,
                        Some(ColumnAlign::Right) => TextAlign::Right,
                        _ => TextAlign::Left,
                    };
                    rect()
                        .key(column)
                        .width(Size::flex(1.))
                        .padding((theme.px(6.), theme.px(8.)))
                        .border(
                            Border::new()
                                .width(theme.px(0.5))
                                .alignment(BorderAlignment::Center)
                                .fill(colors.border),
                        )
                        .child(TextBlock {
                            placeholder: None,
                            ..self.text(
                                target,
                                cell,
                                TextRole::Rich,
                                TextPlace::Cell,
                                TextLook {
                                    weight: header.then_some(FontWeight::BOLD),
                                    align,
                                    ..TextLook::default()
                                },
                            )
                        })
                        .into_element()
                }))
                .into_element()
        });
        let tools = (in_table && !self.shared.read_only).then(|| {
            let button = |text: &'static str, action: fn(crate::EditorController)| {
                rect()
                    .padding((3., 8.))
                    .corner_radius(6.)
                    .background(colors.code_background)
                    .cursor(CursorIcon::Pointer)
                    .on_press(move |_| action(controller))
                    .child(label().font_size(11.5).color(colors.muted).text(text))
            };
            rect()
                .horizontal()
                .spacing(6.)
                .child(button("+ Row", |c| c.table_insert_row()))
                .child(button("+ Column", |c| c.table_insert_column()))
                .child(button("− Row", |c| c.table_remove_row()))
                .child(button("− Column", |c| c.table_remove_column()))
        });
        rect()
            .width(Size::fill())
            .spacing(theme.px(6.))
            .child(
                rect()
                    .width(Size::fill())
                    .corner_radius(theme.px(6.))
                    .overflow(Overflow::Clip)
                    .border(
                        Border::new()
                            .width(1.)
                            .alignment(BorderAlignment::Inner)
                            .fill(colors.border),
                    )
                    .children(rows),
            )
            .maybe_child(tools)
            .into_element()
    }

    /// A transcluded quote, as a decorator shows it or as last seen.
    /// Pressing it follows its source.
    fn transclusion(&self, transclusion: &Transclusion) -> Element {
        let theme = &self.shared.theme;
        let shown = self
            .shared
            .decorators
            .render_transclusion(&TransclusionContext {
                kind: &transclusion.kind,
                url: &transclusion.url,
                label: &transclusion.label,
                fallback: &transclusion.fallback,
                read_only: self.shared.read_only,
                theme,
            });
        if let Some(shown) = shown {
            return shown;
        }
        let colors = &theme.colors;
        let on_link = self.shared.on_link.clone();
        let url = transclusion.url.clone();
        let lines = transclusion
            .fallback
            .split("\n\n")
            .filter(|paragraph| !paragraph.trim().is_empty())
            .map(|paragraph| {
                label()
                    .width(Size::fill())
                    .color(colors.muted)
                    .font_size(theme.body_size)
                    .font_slant(FontSlant::Italic)
                    .text(paragraph.to_owned())
                    .into_element()
            });
        rect()
            .width(Size::fill())
            .padding((theme.px(2.), theme.px(12.)))
            .spacing(theme.px(6.))
            .cursor(CursorIcon::Pointer)
            .border(
                Border::new()
                    .width(BorderWidth {
                        top: 0.,
                        right: 0.,
                        bottom: 0.,
                        left: theme.px(3.),
                    })
                    .alignment(BorderAlignment::Inner)
                    .fill(colors.quote_bar),
            )
            .on_press(move |_| on_link.call(&url))
            .children(lines)
            .child(
                label()
                    .color(colors.accent)
                    .font_size(theme.px(13.))
                    .text(format!("— {}", transclusion.label)),
            )
            .into_element()
    }

    fn table_of_contents(&self) -> Element {
        let theme = &self.shared.theme;
        let colors = &theme.colors;
        let controller = self.shared.controller;
        let min_level = self
            .toc
            .iter()
            .map(|entry| entry.level.get())
            .min()
            .unwrap_or(1);
        let entries: Vec<Element> = if self.toc.is_empty() {
            vec![
                label()
                    .color(colors.muted)
                    .font_size(theme.px(13.))
                    .text(theme.labels.get(Label::EmptyContents))
                    .into_element(),
            ]
        } else {
            self.toc
                .iter()
                .map(|entry| {
                    let block = entry.block;
                    rect()
                        .key(entry.block)
                        .padding((
                            theme.px(2.),
                            0.,
                            theme.px(2.),
                            theme.px(f32::from(entry.level.get() - min_level) * 16.),
                        ))
                        .cursor(CursorIcon::Pointer)
                        .on_press(move |_| controller.focus(block, 0))
                        .child(
                            label()
                                .color(colors.accent)
                                .font_size(theme.px(14.))
                                .text(entry.text.clone()),
                        )
                        .into_element()
                })
                .collect()
        };
        rect()
            .width(Size::fill())
            .padding((theme.px(8.), theme.px(12.)))
            .corner_radius(theme.px(8.))
            .border(
                Border::new()
                    .width(1.)
                    .alignment(BorderAlignment::Inner)
                    .fill(colors.border),
            )
            .child(
                label()
                    .color(colors.muted)
                    .font_size(theme.px(11.))
                    .font_weight(FontWeight::BOLD)
                    .text("CONTENTS"),
            )
            .children(entries)
            .into_element()
    }
}

impl Component for BlockView {
    #[allow(clippy::too_many_lines, reason = "one match over the block kinds")]
    fn render(&self) -> impl IntoElement {
        let theme = self.shared.theme.clone();
        let fallback = || match &self.block.kind {
            BlockKind::Table(table) => self.table(table),
            BlockKind::TableOfContents => self.table_of_contents(),
            BlockKind::Transclusion(transclusion) => self.transclusion(transclusion),
            _ => rect()
                .width(Size::fill())
                .padding((theme.px(10.), 0.))
                .child(
                    rect()
                        .width(Size::fill())
                        .height(Size::px(theme.px(1.)))
                        .background(theme.colors.border),
                )
                .into_element(),
        };
        let mut hovered = use_state(|| false);
        let mut drop_target = use_state(|| false);
        let body = self
            .decorated()
            .or_else(|| self.framed().map(|frame| frame.render(&theme)))
            .unwrap_or_else(fallback);
        let block = rect()
            .width(Size::fill())
            .padding((theme.block_spacing / 2., 0.))
            .child(body);
        if !self.shared.reorderable || self.shared.read_only {
            return block.into_element();
        }

        let colors = &theme.colors;
        let controller = self.shared.controller;
        let id = self.block.id;
        let preview = self
            .block
            .kind
            .content()
            .map(Inline::plain_text)
            .unwrap_or_default()
            .chars()
            .take(40)
            .collect::<String>();
        let handle = DragZone::new(id)
            .drag_element(
                rect()
                    .padding((6., 10.))
                    .corner_radius(8.)
                    .background(colors.popup_background)
                    .border(
                        Border::new()
                            .width(1.)
                            .alignment(BorderAlignment::Inner)
                            .fill(colors.border),
                    )
                    .child(label().color(colors.text).font_size(12.).max_lines(1).text(
                        if preview.is_empty() {
                            theme.labels.get(Label::Block).to_owned()
                        } else {
                            preview
                        },
                    )),
            )
            .child(
                rect()
                    .width(Size::px(HANDLE))
                    .padding((theme.block_spacing / 2. + 3., 0., 0., 0.))
                    .cursor(CursorIcon::Grab)
                    .child(
                        SvgViewer::new(freya::icons::lucide::grip_vertical())
                            .color(if hovered() {
                                colors.muted
                            } else {
                                colors.muted.with_a(70)
                            })
                            .width(Size::px(14.))
                            .height(Size::px(16.)),
                    ),
            );
        DropZone::new(move |dragged: BlockId| controller.move_block_onto(dragged, id))
            .on_drag_over(move |over| drop_target.set(over))
            .width(Size::fill())
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(2.))
                    .background(if drop_target() {
                        colors.accent
                    } else {
                        Color::TRANSPARENT
                    }),
            )
            .child(
                rect()
                    .horizontal()
                    .content(Content::flex())
                    .width(Size::fill())
                    .on_pointer_enter(move |_| hovered.set(true))
                    .on_pointer_leave(move |_| hovered.set_if_modified(false))
                    .child(
                        // A scroll view would claim the touch as a scroll.
                        rect()
                            .on_pointer_down(|event: Event<PointerEventData>| {
                                event.stop_propagation();
                            })
                            .child(handle),
                    )
                    .child(rect().width(Size::flex(1.)).child(block)),
            )
            .into_element()
    }
}

/// Width of the drag handle column shown when blocks can be reordered.
const HANDLE: f32 = 20.0;

const fn callout_icon(kind: CalloutKind) -> &'static str {
    match kind {
        CalloutKind::Note => "ℹ",
        CalloutKind::Tip => "✦",
        CalloutKind::Important => "❗",
        CalloutKind::Warning => "⚠",
        CalloutKind::Caution => "⛔",
    }
}

const fn text_align(align: Align) -> TextAlign {
    match align {
        Align::Left => TextAlign::Left,
        Align::Center => TextAlign::Center,
        Align::Right => TextAlign::Right,
        Align::Justify => TextAlign::Justify,
    }
}

const fn heading_gap(level: HeadingLevel) -> f32 {
    match level {
        HeadingLevel::H1 => 12.,
        HeadingLevel::H2 => 10.,
        HeadingLevel::H3 => 8.,
        _ => 6.,
    }
}

fn caption(text: String, color: Color, theme: &EditorTheme) -> Element {
    label()
        .color(color)
        .font_size(theme.px(11.))
        .margin((0., 0., theme.px(4.), 0.))
        .text(text)
        .into_element()
}
