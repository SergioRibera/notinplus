//! Ready made formatting toolbar driven by an [`EditorController`].

use std::time::Duration;

use freya::icons::lucide;
use freya::prelude::*;
use notein_i18n::tr;

use crate::controller::{EditorController, ImageRequest};
use crate::document::{
    Align, BlockKind, BlockType, CalloutKind, HeadingLevel, Inline, MarkFamily, MarkKind, Rgba,
};
use crate::editor::header::callout_icon;
use crate::labels::Label;
use crate::popup::{
    Anchored, MenuItem, POPUP_MARGIN, PopupArea, input_colors, popup_bounds, popup_frame,
};
use crate::theme::EditorTheme;

const BUTTON: f32 = 30.0;
const ICON: f32 = 16.0;
/// Room a tooltip needs under its button before it flips above.
const TOOLTIP_ROOM: f32 = 44.0;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Block,
    Font,
    Size,
    Color,
    Background,
    Link,
    Insert,
}

/// Word style formatting bar for a [`MarkdownEditor`](crate::MarkdownEditor).
///
/// Groups history, block type, fonts, inline marks, colors, links,
/// alignment, lists and inserts. Every button calls the public
/// [`EditorController`] API, so custom toolbars can be built the same way.
#[derive(Clone, PartialEq)]
pub struct MarkdownToolbar {
    controller: EditorController,
    theme: Option<EditorTheme>,
    show_mode_toggle: bool,
    wrap: bool,
    popup_area: Option<Area>,
    layout: LayoutData,
    key: DiffKey,
}

impl std::fmt::Debug for MarkdownToolbar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MarkdownToolbar").finish_non_exhaustive()
    }
}

impl MarkdownToolbar {
    #[must_use]
    pub fn new(controller: EditorController) -> Self {
        Self {
            controller,
            theme: None,
            show_mode_toggle: true,
            wrap: true,
            popup_area: None,
            layout: LayoutData::default(),
            key: DiffKey::None,
        }
    }

    #[must_use]
    pub fn theme(mut self, theme: EditorTheme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Show the read only / edit switch. On by default.
    #[must_use]
    pub const fn mode_toggle(mut self, show: bool) -> Self {
        self.show_mode_toggle = show;
        self
    }

    /// Wrap the groups onto more rows when they do not fit. On by default;
    /// turn it off when the bar sits in a horizontal scroll view, which
    /// hands its visible width down as the wrapping limit.
    #[must_use]
    pub const fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = wrap;
        self
    }

    /// Region (window coordinates) that menus, pickers and tooltips must
    /// stay inside. Defaults to the window; pass the part left visible when
    /// something covers it, like a soft keyboard drawn over the window.
    #[must_use]
    pub const fn popup_area(mut self, area: Area) -> Self {
        self.popup_area = Some(area);
        self
    }
}

impl KeyExt for MarkdownToolbar {
    fn key_mut(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl LayoutExt for MarkdownToolbar {
    fn layout_mut(&mut self) -> &mut LayoutData {
        &mut self.layout
    }
}

impl ContainerExt for MarkdownToolbar {}

impl Component for MarkdownToolbar {
    #[cfg_attr(feature = "sismo", sismo::function("markdown::toolbar"))]
    #[allow(clippy::too_many_lines, reason = "declarative layout of every group")]
    fn render(&self) -> impl IntoElement {
        let context_theme = use_try_consume::<EditorTheme>();
        let theme = self.theme.clone().or(context_theme).unwrap_or_default();
        let controller = self.controller;
        provide_context(PopupArea(self.popup_area));
        let format = controller.active_format();
        let mut menu = use_state(|| None::<Menu>);
        let link_draft = use_state(String::new);

        let colors = theme.colors.clone();
        let text = theme.labels.clone();
        let editing = !format.read_only;
        let rich = format.can_format();
        let toggle = |family: MarkFamily, kind: MarkKind, icon: Bytes, tip: &str| {
            ToolButton::new(icon, tip)
                .active(format.has(family))
                .enabled(rich)
                .on_press(move || controller.toggle_mark(kind.clone()))
                .colors(&theme)
        };

        let mut toggle_menu = move |which: Menu| {
            let next = (*menu.peek() != Some(which)).then_some(which);
            menu.set(next);
        };
        let mut close_menu = move |which: Menu| {
            if *menu.peek() == Some(which) {
                menu.set(None);
            }
        };

        let history = [
            ToolButton::new(lucide::undo_2(), text.get(Label::Undo))
                .enabled(editing && format.can_undo)
                .on_press(move || controller.undo())
                .colors(&theme)
                .into_element(),
            ToolButton::new(lucide::redo_2(), text.get(Label::Redo))
                .enabled(editing && format.can_redo)
                .on_press(move || controller.redo())
                .colors(&theme)
                .into_element(),
        ];

        let block_picker = Dropdown {
            text: text.block(format.block_type).to_owned(),
            width: 132.,
            enabled: editing && format.target.is_some_and(|t| t.cell.is_none()),
            open: *menu.read() == Some(Menu::Block),
            on_toggle: EventHandler::new(move |()| toggle_menu(Menu::Block)),
            on_close: EventHandler::new(move |()| close_menu(Menu::Block)),
            colors: colors.clone(),
            items: block_items(controller, menu, format.block_type, &theme),
        };

        let font_label = format.font_family().map_or_else(
            || text.get(Label::DefaultFont).to_owned(),
            ToOwned::to_owned,
        );
        let font_picker = Dropdown {
            text: font_label,
            width: 144.,
            enabled: rich,
            open: *menu.read() == Some(Menu::Font),
            on_toggle: EventHandler::new(move |()| toggle_menu(Menu::Font)),
            on_close: EventHandler::new(move |()| close_menu(Menu::Font)),
            colors: colors.clone(),
            items: std::iter::once(MenuItem::new(text.get(Label::DefaultFont), move || {
                controller.set_font_family(None);
                let mut menu = menu;
                menu.set(None);
            }))
            .chain(theme.fonts.iter().map(|family| {
                let family = family.to_string();
                let selected = format.font_family() == Some(family.as_str());
                let chosen = family.clone();
                MenuItem::new(family, move || {
                    controller.set_font_family(Some(chosen.clone()));
                    let mut menu = menu;
                    menu.set(None);
                })
                .selected(selected)
            }))
            .collect(),
        };

        let size_label = format.font_size().map_or_else(
            || text.get(Label::FontSize).to_owned(),
            |size| size.to_string(),
        );
        let size_picker = Dropdown {
            text: size_label,
            width: 76.,
            enabled: rich,
            open: *menu.read() == Some(Menu::Size),
            on_toggle: EventHandler::new(move |()| toggle_menu(Menu::Size)),
            on_close: EventHandler::new(move |()| close_menu(Menu::Size)),
            colors: colors.clone(),
            items: std::iter::once(MenuItem::new(text.get(Label::DefaultSize), move || {
                controller.set_font_size(None);
                let mut menu = menu;
                menu.set(None);
            }))
            .chain(theme.font_sizes.iter().map(|size| {
                let size = *size;
                MenuItem::new(size.to_string(), move || {
                    controller.set_font_size(Some(size));
                    let mut menu = menu;
                    menu.set(None);
                })
                .selected(format.font_size() == Some(size))
            }))
            .collect(),
        };

        let marks = [
            toggle(
                MarkFamily::Strong,
                MarkKind::Strong,
                lucide::bold(),
                text.get(Label::Bold),
            )
            .into_element(),
            toggle(
                MarkFamily::Emphasis,
                MarkKind::Emphasis,
                lucide::italic(),
                text.get(Label::Italic),
            )
            .into_element(),
            toggle(
                MarkFamily::Underline,
                MarkKind::Underline,
                lucide::underline(),
                text.get(Label::Underline),
            )
            .into_element(),
            toggle(
                MarkFamily::Strikethrough,
                MarkKind::Strikethrough,
                lucide::strikethrough(),
                text.get(Label::Strikethrough),
            )
            .into_element(),
            toggle(
                MarkFamily::Code,
                MarkKind::Code,
                lucide::code(),
                text.get(Label::InlineCode),
            )
            .into_element(),
            toggle(
                MarkFamily::Highlight,
                MarkKind::Highlight,
                lucide::highlighter(),
                text.get(Label::Highlight),
            )
            .into_element(),
            toggle(
                MarkFamily::Superscript,
                MarkKind::Superscript,
                lucide::superscript(),
                text.get(Label::Superscript),
            )
            .into_element(),
            toggle(
                MarkFamily::Subscript,
                MarkKind::Subscript,
                lucide::subscript(),
                text.get(Label::Subscript),
            )
            .into_element(),
        ];

        let color_menu = (*menu.read() == Some(Menu::Color)).then(|| {
            palette_popup(&theme, format.color(), move |color| {
                controller.set_color(color);
                let mut menu = menu;
                menu.set(None);
            })
        });
        let background_menu = (*menu.read() == Some(Menu::Background)).then(|| {
            palette_popup(&theme, format.background(), move |color| {
                controller.set_background(color);
                let mut menu = menu;
                menu.set(None);
            })
        });
        let paint = [
            anchored(
                ToolButton::new(lucide::baseline(), text.get(Label::TextColor))
                    .enabled(rich)
                    .swatch(format.color().map(Color::from))
                    .on_press(move || toggle_menu(Menu::Color))
                    .colors(&theme)
                    .into_element(),
                color_menu,
            ),
            anchored(
                ToolButton::new(lucide::paint_bucket(), text.get(Label::BackgroundColor))
                    .enabled(rich)
                    .swatch(format.background().map(Color::from))
                    .on_press(move || toggle_menu(Menu::Background))
                    .colors(&theme)
                    .into_element(),
                background_menu,
            ),
            ToolButton::new(
                lucide::remove_formatting(),
                text.get(Label::ClearFormatting),
            )
            .enabled(rich && format.has_selection)
            .on_press(move || controller.clear_formatting())
            .colors(&theme)
            .into_element(),
        ];

        let current_link = format.link().map(|link| link.url.clone());
        let link_menu = (*menu.read() == Some(Menu::Link))
            .then(|| link_popup(&theme, controller, menu, link_draft, current_link.clone()));
        let mut draft_seed = link_draft;
        let seed = current_link.clone().unwrap_or_default();
        let link = [anchored(
            ToolButton::new(lucide::link(), text.get(Label::Link))
                .active(current_link.is_some())
                .enabled(rich && (format.has_selection || current_link.is_some()))
                .on_press(move || {
                    draft_seed.set(seed.clone());
                    toggle_menu(Menu::Link);
                })
                .colors(&theme)
                .into_element(),
            link_menu,
        )];

        let align = |value: Align, icon: Bytes, tip: &str| {
            ToolButton::new(icon, tip)
                .active(format.target.is_some() && format.align == value)
                .enabled(editing && format.target.is_some_and(|t| t.cell.is_none()))
                .on_press(move || controller.set_align(value))
                .colors(&theme)
                .into_element()
        };
        let alignment = [
            align(
                Align::Left,
                lucide::text_align_start(),
                text.get(Label::AlignLeft),
            ),
            align(
                Align::Center,
                lucide::text_align_center(),
                text.get(Label::AlignCenter),
            ),
            align(
                Align::Right,
                lucide::text_align_end(),
                text.get(Label::AlignRight),
            ),
            align(
                Align::Justify,
                lucide::text_align_justify(),
                text.get(Label::Justify),
            ),
        ];

        let list = |block_type: BlockType, icon: Bytes, tip: &str| {
            let active = format.block_type == Some(block_type);
            ToolButton::new(icon, tip)
                .active(active)
                .enabled(editing && format.target.is_some_and(|t| t.cell.is_none()))
                .on_press(move || {
                    controller.set_block_type(if active {
                        BlockType::Paragraph
                    } else {
                        block_type
                    });
                })
                .colors(&theme)
                .into_element()
        };
        let in_list = matches!(
            format.block_type,
            Some(BlockType::BulletList | BlockType::OrderedList | BlockType::TaskList)
        );
        let lists = [
            list(
                BlockType::BulletList,
                lucide::list(),
                text.get(Label::BulletedList),
            ),
            list(
                BlockType::OrderedList,
                lucide::list_ordered(),
                text.get(Label::NumberedList),
            ),
            list(
                BlockType::TaskList,
                lucide::list_todo(),
                text.get(Label::TodoList),
            ),
            ToolButton::new(lucide::list_indent_decrease(), text.get(Label::Outdent))
                .enabled(editing && in_list)
                .on_press(move || controller.outdent())
                .colors(&theme)
                .into_element(),
            ToolButton::new(lucide::list_indent_increase(), text.get(Label::Indent))
                .enabled(editing && in_list)
                .on_press(move || controller.indent())
                .colors(&theme)
                .into_element(),
        ];

        let insert_picker = Dropdown {
            text: text.get(Label::Insert).to_owned(),
            width: 84.,
            enabled: editing,
            open: *menu.read() == Some(Menu::Insert),
            on_toggle: EventHandler::new(move |()| toggle_menu(Menu::Insert)),
            on_close: EventHandler::new(move |()| close_menu(Menu::Insert)),
            colors: colors.clone(),
            items: insert_items(controller, menu, &theme),
        };

        let mode = self.show_mode_toggle.then(|| {
            ToolButton::new(
                if editing {
                    lucide::eye()
                } else {
                    lucide::pencil()
                },
                if editing {
                    text.get(Label::ReadingMode)
                } else {
                    text.get(Label::EditingMode)
                },
            )
            .active(!editing)
            .on_press(move || controller.toggle_read_only())
            .colors(&theme)
            .into_element()
        });

        rect()
            .layout(self.layout.clone())
            .horizontal()
            .maybe(self.wrap, |bar| bar.content(Content::wrap_spacing(4.)))
            .cross_align(Alignment::center())
            .spacing(2.)
            .children(history)
            .child(separator(&colors))
            .child(block_picker)
            .child(font_picker)
            .child(size_picker)
            .child(separator(&colors))
            .children(marks)
            .child(separator(&colors))
            .children(paint)
            .children(link)
            .child(separator(&colors))
            .children(alignment)
            .child(separator(&colors))
            .children(lists)
            .child(separator(&colors))
            .child(insert_picker)
            .maybe_child(mode.map(|mode| rect().margin((0., 0., 0., 6.)).child(mode)))
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

#[cfg_attr(feature = "sismo", sismo::function("markdown::toolbar_block_items"))]
fn block_items(
    controller: EditorController,
    menu: State<Option<Menu>>,
    current: Option<BlockType>,
    theme: &EditorTheme,
) -> Vec<MenuItem> {
    let text = &theme.labels;
    let entries = [
        BlockType::Paragraph,
        BlockType::Heading(HeadingLevel::H1),
        BlockType::Heading(HeadingLevel::H2),
        BlockType::Heading(HeadingLevel::H3),
        BlockType::Quote,
        BlockType::Callout(CalloutKind::Note),
        BlockType::Callout(CalloutKind::Tip),
        BlockType::Callout(CalloutKind::Important),
        BlockType::Callout(CalloutKind::Warning),
        BlockType::Callout(CalloutKind::Caution),
        BlockType::Code,
        BlockType::Math,
    ];
    entries
        .into_iter()
        .map(|block_type| {
            let caption = match block_type {
                BlockType::Callout(kind) => {
                    tr!("md-insert-callout", "kind" => text.callout(kind))
                }
                other => text.block(Some(other)).to_owned(),
            };
            MenuItem::new(caption, move || {
                controller.set_block_type(block_type);
                let mut menu = menu;
                menu.set(None);
            })
            .icon(
                block_icon(block_type),
                match block_type {
                    BlockType::Callout(kind) => theme.colors.callout(kind),
                    _ => theme.colors.muted,
                },
            )
            .selected(current == Some(block_type))
        })
        .collect()
}

#[cfg_attr(feature = "sismo", sismo::function("markdown::toolbar_insert_items"))]
fn insert_items(
    controller: EditorController,
    menu: State<Option<Menu>>,
    theme: &EditorTheme,
) -> Vec<MenuItem> {
    let text = &theme.labels;
    let icon = theme.colors.muted;
    let text_diagram = text.get(Label::DiagramTemplate).to_owned();
    let close = move || {
        let mut menu = menu;
        menu.set(None);
    };
    let image = controller.can_pick_image().then(|| {
        MenuItem::new(text.get(Label::Image), move || {
            close();
            controller.request_image(ImageRequest::Insert);
        })
        .icon(lucide::image_plus(), icon)
    });
    image
        .into_iter()
        .chain([
            MenuItem::new(text.get(Label::Table), move || {
                controller.insert_table(3, 3);
                close();
            })
            .icon(lucide::table(), icon),
            MenuItem::new(text.get(Label::Divider), move || {
                controller.insert_divider();
                close();
            })
            .icon(lucide::minus(), icon),
            MenuItem::new(text.get(Label::CodeBlock), move || {
                controller.insert_block(BlockKind::Code {
                    language: String::new(),
                    code: Inline::default(),
                });
                close();
            })
            .icon(lucide::code_xml(), icon),
            MenuItem::new(text.get(Label::Diagram), move || {
                controller.insert_block(BlockKind::Code {
                    language: "mermaid".to_owned(),
                    code: Inline::from(text_diagram.as_str()),
                });
                close();
            })
            .icon(lucide::workflow(), icon),
            MenuItem::new(text.get(Label::MathBlock), move || {
                controller.insert_block(BlockKind::Math(Inline::default()));
                close();
            })
            .icon(lucide::sigma(), icon),
            MenuItem::new(text.get(Label::TableOfContents), move || {
                controller.insert_block(BlockKind::TableOfContents);
                close();
            })
            .icon(lucide::list_tree(), icon),
        ])
        .collect()
}

fn separator(colors: &crate::EditorColors) -> Element {
    rect()
        .width(Size::px(1.))
        .height(Size::px(18.))
        .margin((0., 4.))
        .background(colors.border)
        .into_element()
}

#[cfg_attr(feature = "sismo", sismo::function("markdown::toolbar_palette"))]
fn palette_popup(
    theme: &EditorTheme,
    current: Option<Rgba>,
    pick: impl Fn(Option<Rgba>) + Clone + 'static,
) -> Element {
    let colors = &theme.colors;
    let reset = pick.clone();
    let swatches = theme.palette.iter().map(|color| {
        let color = *color;
        let pick = pick.clone();
        let selected = current == Some(color);
        rect()
            .key(color.to_hex())
            .width(Size::px(22.))
            .height(Size::px(22.))
            .corner_radius(11.)
            .background(Color::from(color))
            .border(
                Border::new()
                    .width(if selected { 2. } else { 1. })
                    .alignment(BorderAlignment::Outer)
                    .fill(if selected { colors.text } else { colors.border }),
            )
            .cursor(CursorIcon::Pointer)
            .on_press(move |_| pick(Some(color)))
    });
    popup_frame(colors)
        .width(Size::px(150.))
        .spacing(8.)
        .child(
            rect()
                .horizontal()
                .content(Content::wrap_spacing(6.))
                .spacing(6.)
                .children(swatches),
        )
        .child(
            rect()
                .padding((4., 6.))
                .corner_radius(6.)
                .cursor(CursorIcon::Pointer)
                .on_press(move |_| reset(None))
                .child(
                    label()
                        .color(colors.muted)
                        .font_size(12.)
                        .text(theme.labels.get(Label::DefaultColor).to_owned()),
                ),
        )
        .into_element()
}

#[cfg_attr(feature = "sismo", sismo::function("markdown::toolbar_link_popup"))]
fn link_popup(
    theme: &EditorTheme,
    controller: EditorController,
    menu: State<Option<Menu>>,
    draft: State<String>,
    current: Option<String>,
) -> Element {
    let colors = &theme.colors;
    let close = move || {
        let mut menu = menu;
        menu.set(None);
    };
    let apply = move || {
        let url = draft.peek().trim().to_owned();
        controller.set_link((!url.is_empty()).then_some(url));
        close();
    };
    let action = |text: &str, color: Color, on_press: Box<dyn Fn()>| {
        rect()
            .padding((5., 10.))
            .corner_radius(6.)
            .background(colors.popup_selected)
            .cursor(CursorIcon::Pointer)
            .on_press(move |_| on_press())
            .child(label().font_size(12.).color(color).text(text.to_owned()))
    };
    popup_frame(colors)
        .width(Size::px(280.))
        .spacing(8.)
        .child(
            Input::new(draft)
                .placeholder("https://")
                .theme_colors(input_colors(colors))
                .auto_focus(true)
                .width(Size::fill())
                .on_submit(move |_| apply()),
        )
        .child(
            rect()
                .horizontal()
                .spacing(6.)
                .child(action(
                    theme.labels.get(Label::ApplyLink),
                    colors.accent,
                    Box::new(apply),
                ))
                .maybe_child(current.map(|_| {
                    action(
                        theme.labels.get(Label::RemoveLink),
                        colors.caution,
                        Box::new(move || {
                            controller.set_link(None);
                            close();
                        }),
                    )
                })),
        )
        .into_element()
}

#[derive(Clone, PartialEq)]
struct ToolButtonColors {
    text: Color,
    muted: Color,
    hover: Color,
    active: Color,
    accent: Color,
    tip: Color,
    tip_text: Color,
    tip_border: Color,
}

/// Tooltips paint on the top overlay level: above every menu, popup and
/// dialog, since they never take input.
const TOOLTIP_LAYER: Layer = Layer::OverlayLevel(16);
/// How long the mouse rests on a button before its tooltip shows.
const TOOLTIP_DELAY: Duration = Duration::from_millis(450);
/// Gap between a button and its tooltip.
const TOOLTIP_GAP: f32 = 6.;

/// The tooltip of one [`ToolButton`]: shown after the mouse rests on it,
/// under it or over it near the bottom of the popup bounds, and slid
/// sideways to stay inside them.
#[derive(Clone, Copy, PartialEq)]
struct Tip {
    shown: State<bool>,
    task: State<Option<TaskHandle>>,
    anchor: State<Area>,
    size: State<Area>,
}

impl Tip {
    fn use_tip() -> Self {
        Self {
            shown: use_state(|| false),
            task: use_state(|| None),
            anchor: use_state(Area::default),
            size: use_state(Area::default),
        }
    }

    /// Touch never shows a tooltip: a finger does not rest, and the press
    /// already does what the button says.
    fn enter(mut self, event: &Event<PointerEventData>) {
        if !matches!(event.data(), PointerEventData::Mouse(_)) {
            return;
        }
        self.cancel();
        let mut shown = self.shown;
        self.task.set(Some(spawn(async move {
            timer(TOOLTIP_DELAY).await;
            shown.set_if_modified(true);
        })));
    }

    fn leave(mut self) {
        self.cancel();
        self.shown.set_if_modified(false);
    }

    fn cancel(&mut self) {
        if let Some(task) = self.task.write().take() {
            task.cancel();
        }
    }

    fn track(mut self, event: &Event<SizedEventData>) {
        self.anchor.set_if_modified(event.area);
    }

    fn render(self, text: String, colors: &ToolButtonColors) -> Option<Element> {
        if !*self.shown.read() {
            return None;
        }
        let mut size = self.size;
        let anchor = *self.anchor.peek();
        let measured = *size.read();
        let bounds = popup_bounds();
        let over = anchor.max_y() + TOOLTIP_ROOM > bounds.max_y();
        let centred = anchor.center().x - measured.width() / 2.;
        let (min_left, max_left) = (
            bounds.min_x() + POPUP_MARGIN,
            (bounds.max_x() - measured.width() - POPUP_MARGIN).max(bounds.min_x() + POPUP_MARGIN),
        );
        let left = centred.clamp(min_left, max_left) - anchor.min_x();
        let position = if over {
            Position::new_absolute().bottom(anchor.height() + TOOLTIP_GAP)
        } else {
            Position::new_absolute().top(anchor.height() + TOOLTIP_GAP)
        };
        Some(
            rect()
                .position(position.left(left))
                .layer(TOOLTIP_LAYER)
                .interactive(Interactive::No)
                .on_sized(move |event: Event<SizedEventData>| size.set_if_modified(event.area))
                .padding((5., 9.))
                .corner_radius(6.)
                .background(colors.tip)
                .border(
                    Border::new()
                        .width(1.)
                        .alignment(BorderAlignment::Inner)
                        .fill(colors.tip_border),
                )
                .child(
                    label()
                        .text(text)
                        .font_size(12.)
                        .color(colors.tip_text)
                        .max_lines(1),
                )
                .into_element(),
        )
    }
}

/// Square icon button with hover, active and disabled states.
#[derive(Clone, PartialEq)]
struct ToolButton {
    icon: Bytes,
    tip: String,
    active: bool,
    enabled: bool,
    swatch: Option<Color>,
    on_press: Option<EventHandler<()>>,
    colors: Option<ToolButtonColors>,
}

impl ToolButton {
    fn new(icon: Bytes, tip: impl Into<String>) -> Self {
        Self {
            icon,
            tip: tip.into(),
            active: false,
            enabled: true,
            swatch: None,
            on_press: None,
            colors: None,
        }
    }

    const fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    const fn swatch(mut self, swatch: Option<Color>) -> Self {
        self.swatch = swatch;
        self
    }

    fn on_press(mut self, on_press: impl FnMut() + 'static) -> Self {
        let mut on_press = on_press;
        self.on_press = Some(EventHandler::new(move |()| on_press()));
        self
    }

    fn colors(mut self, theme: &EditorTheme) -> Self {
        let colors = &theme.colors;
        self.colors = Some(ToolButtonColors {
            text: colors.text,
            muted: colors.muted,
            hover: colors.popup_selected,
            active: colors.accent.with_a(46),
            accent: colors.accent,
            tip: colors.popup_background,
            tip_text: colors.text,
            tip_border: colors.border,
        });
        self
    }
}

impl Component for ToolButton {
    #[cfg_attr(feature = "sismo", sismo::function("markdown::tool_button"))]
    fn render(&self) -> impl IntoElement {
        let mut hovered = use_state(|| false);
        let tip = Tip::use_tip();
        let colors = self.colors.clone().unwrap_or(ToolButtonColors {
            text: Color::WHITE,
            muted: Color::GRAY,
            hover: Color::from_argb(40, 255, 255, 255),
            active: Color::from_argb(60, 110, 168, 254),
            accent: Color::from_rgb(110, 168, 254),
            tip: Color::from_rgb(32, 32, 36),
            tip_text: Color::WHITE,
            tip_border: Color::from_argb(40, 255, 255, 255),
        });
        let enabled = self.enabled;
        let background = if self.active {
            colors.active
        } else if hovered() && enabled {
            colors.hover
        } else {
            Color::TRANSPARENT
        };
        let foreground = if !enabled {
            colors.muted.with_a(110)
        } else if self.active {
            colors.accent
        } else {
            colors.text
        };
        let on_press = self.on_press.clone();
        let button = rect()
            .width(Size::px(BUTTON))
            .height(Size::px(BUTTON))
            .corner_radius(7.)
            .center()
            .background(background)
            .cursor(if enabled {
                CursorIcon::Pointer
            } else {
                CursorIcon::NotAllowed
            })
            .on_pointer_enter(move |event: Event<PointerEventData>| {
                hovered.set(matches!(event.data(), PointerEventData::Mouse(_)));
                tip.enter(&event);
            })
            .on_pointer_leave(move |_| {
                hovered.set_if_modified(false);
                tip.leave();
            })
            .on_press(move |event: Event<PressEventData>| {
                event.stop_propagation();
                tip.leave();
                if enabled && let Some(handler) = &on_press {
                    handler.call(());
                }
            })
            .child(
                SvgViewer::new(self.icon.clone())
                    .color(foreground)
                    .width(Size::px(ICON))
                    .height(Size::px(ICON)),
            )
            .maybe_child(self.swatch.map(|swatch| {
                rect()
                    .width(Size::px(14.))
                    .height(Size::px(3.))
                    .margin((2., 0., 0., 0.))
                    .corner_radius(2.)
                    .background(swatch)
            }));
        rect()
            .on_sized(move |event: Event<SizedEventData>| tip.track(&event))
            .child(button)
            .maybe_child(tip.render(self.tip.clone(), &colors))
    }
}

/// Text button that opens a list of [`MenuItem`]s.
#[derive(Clone, PartialEq)]
struct Dropdown {
    text: String,
    width: f32,
    enabled: bool,
    open: bool,
    on_toggle: EventHandler<()>,
    on_close: EventHandler<()>,
    colors: crate::EditorColors,
    items: Vec<MenuItem>,
}

impl Component for Dropdown {
    #[cfg_attr(feature = "sismo", sismo::function("markdown::toolbar_dropdown"))]
    fn render(&self) -> impl IntoElement {
        let mut hovered = use_state(|| false);
        let colors = &self.colors;
        let enabled = self.enabled;
        let on_toggle = self.on_toggle.clone();
        let background = if self.open || (hovered() && enabled) {
            colors.popup_selected
        } else {
            Color::TRANSPARENT
        };
        let foreground = if enabled {
            colors.text
        } else {
            colors.muted.with_a(110)
        };
        let button = rect()
            .width(Size::px(self.width))
            .height(Size::px(BUTTON))
            .padding((0., 6., 0., 8.))
            .corner_radius(7.)
            .background(background)
            .border(
                Border::new()
                    .width(1.)
                    .alignment(BorderAlignment::Inner)
                    .fill(colors.border),
            )
            .horizontal()
            .content(Content::flex())
            .cross_align(Alignment::center())
            .cursor(if enabled {
                CursorIcon::Pointer
            } else {
                CursorIcon::NotAllowed
            })
            .on_pointer_enter(move |event: Event<PointerEventData>| {
                hovered.set(matches!(event.data(), PointerEventData::Mouse(_)));
            })
            .on_pointer_leave(move |_| hovered.set_if_modified(false))
            .on_press(move |event: Event<PressEventData>| {
                event.stop_propagation();
                if enabled {
                    on_toggle.call(());
                }
            })
            .child(
                rect().width(Size::flex(1.)).child(
                    label()
                        .width(Size::fill())
                        .font_size(12.5)
                        .color(foreground)
                        .max_lines(1)
                        .text_overflow(TextOverflow::Ellipsis)
                        .text(self.text.clone()),
                ),
            )
            .child(
                SvgViewer::new(lucide::chevron_down())
                    .color(colors.muted)
                    .width(Size::px(14.))
                    .height(Size::px(14.)),
            )
            .into_element();

        let popup = (self.open && enabled).then(|| {
            popup_frame(colors)
                .padding(4.)
                .width(Size::px(self.width.max(150.)))
                .children(
                    self.items
                        .iter()
                        .enumerate()
                        .map(|(index, item)| item.row(index, colors, None)),
                )
                .into_element()
        });
        let on_close = self.on_close.clone();
        Anchored::new(button, popup)
            .on_dismiss(move || on_close.call(()))
            .into_element()
    }
}

/// Child with an optional popup. See [`Anchored`].
fn anchored(child: Element, popup: Option<Element>) -> Element {
    Anchored::new(child, popup).into_element()
}

fn block_icon(block_type: BlockType) -> Bytes {
    match block_type {
        BlockType::Paragraph => lucide::pilcrow(),
        BlockType::Heading(HeadingLevel::H1) => lucide::heading_1(),
        BlockType::Heading(HeadingLevel::H2) => lucide::heading_2(),
        BlockType::Heading(_) => lucide::heading_3(),
        BlockType::Quote => lucide::quote(),
        BlockType::Callout(kind) => callout_icon(kind),
        BlockType::BulletList => lucide::list(),
        BlockType::OrderedList => lucide::list_ordered(),
        BlockType::TaskList => lucide::list_checks(),
        BlockType::Code => lucide::code_xml(),
        BlockType::Math => lucide::sigma(),
        BlockType::Term | BlockType::Definition => lucide::text_quote(),
    }
}
