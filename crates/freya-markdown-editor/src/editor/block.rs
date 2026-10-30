//! Rendering of a single block: gutter, container chrome and the text
//! editors it holds.
//!
//! Every block with a single text run renders through [`Frame`] so the
//! element tree keeps the same shape when the block changes kind. That lets
//! the [`TextBlock`] (and its focus and caret) survive conversions like
//! paragraph to heading or list item.

use freya::prelude::*;

use freya::icons::lucide;

use super::Shared;
use super::header::{ChipMenu, CopyButton, HeaderChip, RenameButton, callout_icon, language_name};
use super::properties::PropertiesPanel;
use super::text::{TextBlock, TextLook, TextPlace};
use crate::controller::CaretRequest;
use crate::decorator::{CodeContext, MathContext, TransclusionContext};
use crate::document::{
    Align, Block, BlockId, BlockKind, BlockType, CalloutKind, CellPos, ColumnAlign, HeadingLevel,
    Inline, ListKind, Properties, Table, TextRole, TextTarget, Transclusion,
};
use crate::labels::Label;
use crate::popup::MenuItem;
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

/// What a block remembers between renders.
#[derive(Clone, Copy)]
struct View {
    /// Opens `<details>` and property lists.
    unfolded: State<bool>,
    /// Shows front matter as YAML.
    as_text: State<bool>,
    /// Width of the block, for drawings that must fit in it.
    width: f32,
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
    /// Keeps the text mounted (and its caret) but takes no room.
    collapsed: bool,
    /// Under the text, like the drawing of a formula.
    footer: Option<Element>,
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
            collapsed: false,
            footer: None,
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
                                    // Over the text, which keeps its paragraph laid out
                                    // even while folded away and could take their presses.
                                    .child(
                                        rect()
                                            .width(Size::fill())
                                            .layer(Layer::Relative(1))
                                            .maybe_child(self.header),
                                    )
                                    .child(
                                        rect()
                                            .width(Size::fill())
                                            .maybe(self.collapsed, |text| {
                                                text.height(Size::px(0.)).overflow(Overflow::Clip)
                                            })
                                            .child(self.text),
                                    )
                                    .child(
                                        rect()
                                            .width(Size::fill())
                                            .layer(Layer::Relative(1))
                                            .maybe_child(self.footer),
                                    ),
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

    const fn editable(&self) -> bool {
        !self.shared.read_only
    }

    /// Callout kinds a block can turn into, as menu entries.
    fn callout_items(&self, current: Option<CalloutKind>) -> Vec<MenuItem> {
        let controller = self.shared.controller;
        let block = self.block.id;
        let theme = &self.shared.theme;
        CalloutKind::ALL
            .into_iter()
            .map(|kind| {
                MenuItem::new(theme.labels.callout(kind), move || {
                    if current.is_some() {
                        controller.set_callout_kind(block, kind);
                    } else {
                        controller.set_type_of(block, BlockType::Callout(kind));
                    }
                })
                .icon(callout_icon(kind), theme.colors.callout(kind))
                .selected(current == Some(kind))
            })
            .collect()
    }

    /// Kind and title of a callout; pressing it changes the kind or turns
    /// the callout back into a plain quote.
    fn callout_chip(&self, kind: CalloutKind, title: Option<&str>) -> Element {
        let theme = &self.shared.theme;
        let controller = self.shared.controller;
        let block = self.block.id;
        let plain = MenuItem::new(theme.labels.get(Label::PlainQuote), move || {
            controller.set_type_of(block, BlockType::Quote);
        })
        .icon(lucide::quote(), theme.colors.muted);
        HeaderChip {
            icon: callout_icon(kind),
            text: title.map_or_else(|| theme.labels.callout(kind).to_owned(), str::to_owned),
            color: theme.colors.callout(kind),
            theme: theme.clone(),
            editable: self.editable(),
            auto_open: false,
            menu: ChipMenu::Items(vec![self.callout_items(Some(kind)), vec![plain]]),
        }
        .into_element()
    }

    /// Header of a plain quote while editing, turning it into a callout. In
    /// the flow like a callout's, since the text would take the presses of
    /// anything floating over its line on touch screens.
    fn quote_chip(&self) -> Element {
        let theme = &self.shared.theme;
        HeaderChip {
            icon: lucide::quote(),
            text: theme.labels.get(Label::Quote).to_owned(),
            color: theme.colors.muted,
            theme: theme.clone(),
            editable: true,
            auto_open: false,
            menu: ChipMenu::Items(vec![self.callout_items(None)]),
        }
        .into_element()
    }

    /// How a block with a drawing (a formula, a diagram) shows: the drawing
    /// alone until someone edits it, then its source with the drawing kept
    /// up to date under it. Returns whether the source is folded away and
    /// the footer holding the drawing.
    fn drawing(
        &self,
        drawn: Option<Element>,
        empty: bool,
        unfolded: State<bool>,
    ) -> (bool, Option<Element>) {
        let Some(drawn) = drawn else {
            return (false, None);
        };
        let theme = &self.shared.theme;
        let editing = self.editable() && (self.focused.is_some() || empty || unfolded());
        let controller = self.shared.controller;
        let keyboard = self.shared.request_keyboard.clone();
        let block = self.block.id;
        let footer = rect()
            .width(Size::fill())
            .cross_align(Alignment::center())
            .maybe(editing, |footer| {
                footer
                    .margin((theme.px(8.), 0., 0., 0.))
                    .padding((theme.px(10.), 0., theme.px(2.), 0.))
                    .border(
                        Border::new()
                            .width(BorderWidth {
                                top: 1.,
                                right: 0.,
                                bottom: 0.,
                                left: 0.,
                            })
                            .alignment(BorderAlignment::Inner)
                            .fill(theme.colors.border),
                    )
            })
            .maybe(!editing && self.editable(), |footer| {
                footer
                    .padding((theme.px(4.), 0.))
                    .cursor(CursorIcon::Pointer)
                    .on_press(move |event: Event<PressEventData>| {
                        event.stop_propagation();
                        controller.focus(block, usize::MAX);
                        keyboard.call();
                    })
            })
            .child(drawn)
            .into_element();
        (!editing, Some(footer))
    }

    /// Chip of a formula block, with templates for people who do not know
    /// TeX.
    fn math_header(&self, toggle: Option<Element>) -> Option<Element> {
        if !self.editable() {
            return None;
        }
        let theme = &self.shared.theme;
        let controller = self.shared.controller;
        let block = self.block.id;
        let templates = [
            (Label::Fraction, r"\frac{a}{b}"),
            (Label::SquareRoot, r"\sqrt{x}"),
            (Label::Power, "x^{2}"),
            (Label::MathIndex, "x_{i}"),
            (Label::Sum, r"\sum_{i=1}^{n} "),
            (Label::Integral, r"\int_{a}^{b} f(x)\,dx"),
            (
                Label::Matrix,
                r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
            ),
        ]
        .into_iter()
        .map(|(name, tex)| {
            MenuItem::new(theme.labels.get(name), move || {
                controller.insert_text_in(block, tex);
            })
            .detail(tex)
        })
        .collect();
        Some(
            rect()
                .margin((theme.px(-4.), 0., theme.px(4.), theme.px(-6.)))
                .width(Size::fill())
                .horizontal()
                .content(Content::flex())
                .cross_align(Alignment::center())
                .child(rect().width(Size::flex(1.)).child(HeaderChip {
                    icon: lucide::sigma(),
                    text: theme.labels.get(Label::Formula).to_owned(),
                    color: theme.colors.muted,
                    theme: theme.clone(),
                    editable: true,
                    auto_open: false,
                    menu: ChipMenu::Items(vec![templates]),
                }))
                .maybe_child(toggle)
                .into_element(),
        )
    }

    /// Shows the source of a block that only shows its drawing, with the
    /// caret at its end. The source stays open even if the focus does not
    /// arrive, so it can always be tapped into.
    fn source_opener(&self, unfolded: State<bool>) -> impl Fn() + 'static {
        let controller = self.shared.controller;
        let keyboard = self.shared.request_keyboard.clone();
        let block = self.block.id;
        move || {
            let mut unfolded = unfolded;
            unfolded.set(true);
            controller.focus(block, usize::MAX);
            keyboard.call();
        }
    }

    /// Edit while only the drawing of a block shows (`folded`), Done while
    /// its source does.
    fn source_toggle(&self, folded: bool, unfolded: State<bool>) -> Element {
        let theme = &self.shared.theme;
        let color = theme.colors.muted;
        let controller = self.shared.controller;
        let block = self.block.id;
        let focused = self.focused.is_some();
        let open = self.source_opener(unfolded);
        let (icon, caption) = if folded {
            (lucide::pencil(), Label::EditSource)
        } else {
            (lucide::check(), Label::DoneEditing)
        };
        rect()
            .height(Size::px(theme.px(28.)))
            .padding((0., theme.px(8.)))
            .corner_radius(theme.px(6.))
            .horizontal()
            .spacing(theme.px(6.))
            .cross_align(Alignment::center())
            .cursor(CursorIcon::Pointer)
            .on_press(move |event: Event<PressEventData>| {
                event.stop_propagation();
                if folded {
                    open();
                    return;
                }
                let mut unfolded = unfolded;
                unfolded.set(false);
                if focused {
                    controller.focus_beside(block);
                }
            })
            .child(
                SvgViewer::new(icon)
                    .color(color)
                    .width(Size::px(theme.px(14.)))
                    .height(Size::px(theme.px(14.))),
            )
            .child(
                label()
                    .color(color)
                    .font_size(theme.px(12.))
                    .text(theme.labels.get(caption).to_owned()),
            )
            .into_element()
    }

    /// Folding header of raw HTML.
    fn html_header(&self, open: bool, unfolded: State<bool>) -> Element {
        let theme = &self.shared.theme;
        let color = theme.colors.muted;
        let mut unfolded = unfolded;
        rect()
            .height(Size::px(theme.px(28.)))
            .horizontal()
            .spacing(theme.px(6.))
            .cross_align(Alignment::center())
            .cursor(CursorIcon::Pointer)
            .on_press(move |event: Event<PressEventData>| {
                event.stop_propagation();
                unfolded.toggle();
            })
            .child(
                SvgViewer::new(lucide::file_code())
                    .color(color)
                    .width(Size::px(theme.px(13.)))
                    .height(Size::px(theme.px(13.))),
            )
            .child(
                label()
                    .color(color)
                    .font_size(theme.px(11.5))
                    .font_weight(FontWeight::BOLD)
                    .text("HTML"),
            )
            .child(
                SvgViewer::new(if open {
                    lucide::chevron_down()
                } else {
                    lucide::chevron_right()
                })
                .color(color)
                .width(Size::px(theme.px(12.)))
                .height(Size::px(theme.px(12.))),
            )
            .into_element()
    }

    /// Caption telling a comment apart from the text around it.
    fn comment_header(&self) -> Element {
        let theme = &self.shared.theme;
        let color = theme.colors.muted;
        rect()
            .horizontal()
            .spacing(theme.px(6.))
            .margin((0., 0., theme.px(4.), 0.))
            .cross_align(Alignment::center())
            .child(
                SvgViewer::new(lucide::message_square_dashed())
                    .color(color)
                    .width(Size::px(theme.px(13.)))
                    .height(Size::px(theme.px(13.))),
            )
            .child(
                label()
                    .color(color)
                    .font_size(theme.px(11.5))
                    .font_weight(FontWeight::BOLD)
                    .text(theme.labels.get(Label::Comment).to_owned()),
            )
            .child(
                label()
                    .color(color.with_a(170))
                    .font_size(theme.px(11.5))
                    .text(format!("· {}", theme.labels.get(Label::CommentHint))),
            )
            .into_element()
    }

    /// Chevron and summary of a `<details>` block. Pressing either folds
    /// it; folding is a view state, the file keeps how it starts.
    fn details_header(&self, summary: &str, open: bool, unfolded: State<bool>) -> Element {
        let theme = &self.shared.theme;
        let colors = &theme.colors;
        let controller = self.shared.controller;
        let block = self.block.id;
        let focused = self.focused.is_some();
        let mut unfolded = unfolded;
        let shown = if summary.is_empty() {
            theme.labels.get(Label::Collapsible).to_owned()
        } else {
            summary.to_owned()
        };
        rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::flex())
            .cross_align(Alignment::center())
            .margin((0., theme.px(-6.), theme.px(2.), theme.px(-4.)))
            .child(
                rect()
                    .width(Size::flex(1.))
                    .height(Size::px(theme.px(32.)))
                    .horizontal()
                    .content(Content::flex())
                    .spacing(theme.px(6.))
                    .cross_align(Alignment::center())
                    .cursor(CursorIcon::Pointer)
                    .on_press(move |event: Event<PressEventData>| {
                        event.stop_propagation();
                        if !open {
                            unfolded.set(true);
                            return;
                        }
                        unfolded.set(false);
                        // The caret would stay in hidden text: it moves next to it.
                        if focused {
                            controller.focus_beside(block);
                        }
                    })
                    .child(
                        SvgViewer::new(if open {
                            lucide::chevron_down()
                        } else {
                            lucide::chevron_right()
                        })
                        .color(colors.muted)
                        .width(Size::px(theme.px(16.)))
                        .height(Size::px(theme.px(16.))),
                    )
                    .child(
                        label()
                            .width(Size::flex(1.))
                            .max_lines(1)
                            .text_overflow(TextOverflow::Ellipsis)
                            .color(if summary.is_empty() {
                                colors.muted
                            } else {
                                colors.text
                            })
                            .font_size(theme.body_size)
                            .font_weight(FontWeight::BOLD)
                            .text(shown),
                    ),
            )
            .maybe_child(self.editable().then(|| RenameButton {
                current: summary.to_owned(),
                placeholder: theme.labels.get(Label::DetailsSummary).to_owned(),
                hint: None,
                color: colors.muted,
                theme: theme.clone(),
                on_rename: EventHandler::new(move |summary: String| {
                    controller.set_details_summary(block, &summary);
                }),
            }))
            .into_element()
    }

    /// Language chip and copy button of a code block, after `toggle` when it
    /// draws something, see [`Self::source_toggle`].
    fn code_header(
        &self,
        language: &str,
        code: &Inline,
        toggle: Option<Element>,
    ) -> Option<Element> {
        let theme = &self.shared.theme;
        let colors = &theme.colors;
        let editable = self.editable();
        let name = language_name(language);
        if !editable && name.is_none() {
            return None;
        }
        let diagram = matches!(
            language.trim().to_ascii_lowercase().as_str(),
            "mermaid" | "dot" | "graphviz"
        );
        let chip = HeaderChip {
            icon: if diagram {
                lucide::workflow()
            } else {
                lucide::code_xml()
            },
            text: name
                .unwrap_or_else(|| theme.labels.get(Label::PlainText))
                .to_owned(),
            color: colors.muted,
            theme: theme.clone(),
            editable,
            auto_open: language.is_empty() && code.is_empty() && self.focused.is_some(),
            menu: ChipMenu::Language {
                controller: self.shared.controller,
                block: self.block.id,
                current: language.to_owned(),
            },
        };
        Some(
            rect()
                .width(Size::fill())
                .horizontal()
                .content(Content::flex())
                .cross_align(Alignment::center())
                .margin((theme.px(-4.), theme.px(-6.), theme.px(4.), theme.px(-6.)))
                .child(rect().width(Size::flex(1.)).child(chip))
                .maybe_child(toggle.filter(|_| editable))
                .child(CopyButton {
                    text: code.plain_text(),
                    theme: theme.clone(),
                })
                .into_element(),
        )
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
    fn framed(&self, view: View) -> Option<Frame> {
        let View {
            unfolded,
            as_text,
            width,
        } = view;
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
                header: self.editable().then(|| {
                    rect()
                        .margin((0., 0., theme.px(2.), theme.px(-6.)))
                        .child(self.quote_chip())
                        .into_element()
                }),
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
                    // The chip's own padding lines its icon up with the text.
                    .margin((theme.px(-2.), 0., theme.px(2.), theme.px(-6.)))
                    .horizontal()
                    .cross_align(Alignment::center())
                    .child(self.callout_chip(*kind, title.as_deref()))
                    .maybe_child((self.editable() && self.focused.is_some()).then(|| {
                        let controller = self.shared.controller;
                        let block = self.block.id;
                        RenameButton {
                            current: title.clone().unwrap_or_default(),
                            placeholder: theme.labels.callout(*kind).to_owned(),
                            hint: Some(theme.labels.get(Label::RenameHint).to_owned()),
                            color: accent,
                            theme: theme.clone(),
                            on_rename: EventHandler::new(move |title: String| {
                                controller.set_callout_title(block, &title);
                            }),
                        }
                    }))
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
            BlockKind::Code { language, code } => {
                let drawn = self.shared.decorators.render_preview(&CodeContext {
                    language,
                    code: code.text(),
                    max_width: width - theme.px(24.),
                    theme,
                });
                let (collapsed, footer) = self.drawing(drawn, code.is_empty(), unfolded);
                let toggle = (footer.is_some() && !code.is_empty())
                    .then(|| self.source_toggle(collapsed, unfolded));
                Frame {
                    header: self.code_header(language, code, toggle),
                    collapsed,
                    footer,
                    ..panel(plain(code, colors.text), None)
                }
            }
            BlockKind::Math(tex) => {
                let drawn = self.shared.decorators.render_math(&MathContext {
                    tex: tex.text(),
                    display: true,
                    font_size: theme.body_size,
                    max_width: width - theme.px(24.),
                    theme,
                });
                let (collapsed, footer) = self.drawing(drawn, tex.is_empty(), unfolded);
                let toggle = (footer.is_some() && !tex.is_empty())
                    .then(|| self.source_toggle(collapsed, unfolded));
                Frame {
                    header: self.math_header(toggle),
                    collapsed,
                    footer,
                    ..panel(plain(tex, colors.text), None)
                }
            }
            BlockKind::FrontMatter(raw) => {
                let properties = Properties::parse(raw.text());
                // Rich YAML only reads as text.
                let listed = properties.is_some() && !as_text();
                Frame {
                    header: Some(
                        PropertiesPanel {
                            block: self.block.id,
                            properties,
                            controller: self.shared.controller,
                            theme: theme.clone(),
                            editable: self.editable(),
                            unfolded,
                            as_text,
                        }
                        .into_element(),
                    ),
                    collapsed: listed,
                    ..panel(plain(raw, colors.muted), None)
                }
            }
            BlockKind::Html(raw) => {
                // Markup the editor cannot show stays out of the way until asked for.
                let open = unfolded() || self.focused.is_some();
                Frame {
                    padding: (theme.px(2.), theme.px(10.), theme.px(4.), theme.px(10.)),
                    header: Some(self.html_header(open, unfolded)),
                    collapsed: !open,
                    ..panel(plain(raw, colors.muted), None)
                }
            }
            BlockKind::Comment(note) => Frame {
                padding: (theme.px(6.), theme.px(10.), theme.px(6.), theme.px(10.)),
                background: colors.muted.with_a(18),
                radius: theme.px(6.),
                header: Some(self.comment_header()),
                ..Frame::new(self.block_text(
                    note,
                    TextRole::Plain,
                    TextPlace::Block,
                    TextLook {
                        font_size: Some(theme.body_size * 0.92),
                        color: Some(colors.muted),
                        ..TextLook::default()
                    },
                ))
            },
            BlockKind::Details {
                summary, content, ..
            } => {
                // Focus opens it, so the caret is never inside hidden text.
                let open = unfolded() || self.focused.is_some();
                Frame {
                    padding: (theme.px(4.), theme.px(10.), theme.px(6.), theme.px(10.)),
                    background: colors.code_background.with_a(120),
                    radius: theme.px(8.),
                    header: Some(self.details_header(summary, open, unfolded)),
                    collapsed: !open,
                    ..Frame::new(rich(content, self.look()))
                }
            }
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
    fn decorated(&self, width: f32) -> Option<Element> {
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
            max_width: width,
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
            let button = |text: Label, action: fn(crate::EditorController)| {
                rect()
                    .padding((3., 8.))
                    .corner_radius(6.)
                    .background(colors.code_background)
                    .cursor(CursorIcon::Pointer)
                    .on_press(move |_| action(controller))
                    .child(
                        label()
                            .font_size(11.5)
                            .color(colors.muted)
                            .text(theme.labels.get(text).to_owned()),
                    )
            };
            rect()
                .horizontal()
                .spacing(6.)
                .child(button(Label::TableAddRow, |c| c.table_insert_row()))
                .child(button(Label::TableAddColumn, |c| c.table_insert_column()))
                .child(button(Label::TableRemoveRow, |c| c.table_remove_row()))
                .child(button(Label::TableRemoveColumn, |c| {
                    c.table_remove_column();
                }))
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
                    .text(theme.labels.get(Label::EmptyContents).to_owned())
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
                    .text(theme.labels.get(Label::Contents).to_owned()),
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
        let unfolded =
            use_state(|| matches!(self.block.kind, BlockKind::Details { open: true, .. }));
        let as_text = use_state(|| false);
        // Only the width, so scrolling (which moves the area) re-renders nothing.
        let mut width = use_state(|| 0f32);
        if self.shared.read_only && matches!(self.block.kind, BlockKind::Comment(_)) {
            return rect().into_element();
        }
        let view = View {
            unfolded,
            as_text,
            width: width(),
        };
        let body = self
            .decorated(view.width)
            .or_else(|| self.framed(view).map(|frame| frame.render(&theme)))
            .unwrap_or_else(fallback);
        let block = rect()
            .width(Size::fill())
            .on_sized(move |event: Event<SizedEventData>| {
                width.set_if_modified(event.area.width());
            })
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
