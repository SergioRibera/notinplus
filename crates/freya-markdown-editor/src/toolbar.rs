//! Ready made formatting toolbar driven by an [`EditorController`].

use std::time::Duration;

use freya::icons::lucide;
use freya::prelude::*;

use crate::controller::{ActiveFormat, EditorController};
use crate::document::{
    Align, BlockKind, BlockType, CalloutKind, HeadingLevel, Inline, MarkFamily, MarkKind, Rgba,
};
use crate::theme::EditorTheme;

const BUTTON: f32 = 30.0;
const ICON: f32 = 16.0;

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
}

impl KeyExt for MarkdownToolbar {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl LayoutExt for MarkdownToolbar {
    fn get_layout(&mut self) -> &mut LayoutData {
        &mut self.layout
    }
}

impl ContainerExt for MarkdownToolbar {}

impl Component for MarkdownToolbar {
    #[allow(clippy::too_many_lines, reason = "declarative layout of every group")]
    fn render(&self) -> impl IntoElement {
        let context_theme = use_try_consume::<EditorTheme>();
        let theme = self.theme.clone().or(context_theme).unwrap_or_default();
        let controller = self.controller;
        let format = controller.active_format();
        let mut menu = use_state(|| None::<Menu>);
        let link_draft = use_state(String::new);

        let colors = theme.colors.clone();
        let editing = !format.read_only;
        let rich = format.can_format();
        let toggle = |family: MarkFamily, kind: MarkKind, icon: Bytes, tip: &'static str| {
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

        let history = group([
            ToolButton::new(lucide::undo_2(), "Undo (Ctrl+Z)")
                .enabled(editing && format.can_undo)
                .on_press(move || controller.undo())
                .colors(&theme)
                .into_element(),
            ToolButton::new(lucide::redo_2(), "Redo (Ctrl+Y)")
                .enabled(editing && format.can_redo)
                .on_press(move || controller.redo())
                .colors(&theme)
                .into_element(),
        ]);

        let block_label = block_label(format.block_type);
        let block_picker = Dropdown {
            text: block_label.to_owned(),
            width: 132.,
            enabled: editing && format.target.is_some_and(|t| t.cell.is_none()),
            open: *menu.read() == Some(Menu::Block),
            on_toggle: EventHandler::new(move |()| toggle_menu(Menu::Block)),
            colors: colors.clone(),
            items: block_items(controller, menu, format.block_type),
        };

        let font_label = format
            .font_family()
            .map_or_else(|| "Default font".to_owned(), ToOwned::to_owned);
        let font_picker = Dropdown {
            text: font_label,
            width: 128.,
            enabled: rich,
            open: *menu.read() == Some(Menu::Font),
            on_toggle: EventHandler::new(move |()| toggle_menu(Menu::Font)),
            colors: colors.clone(),
            items: std::iter::once(MenuItem::new("Default font", move || {
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

        let size_label = format
            .font_size()
            .map_or_else(|| "Size".to_owned(), |size| size.to_string());
        let size_picker = Dropdown {
            text: size_label,
            width: 64.,
            enabled: rich,
            open: *menu.read() == Some(Menu::Size),
            on_toggle: EventHandler::new(move |()| toggle_menu(Menu::Size)),
            colors: colors.clone(),
            items: std::iter::once(MenuItem::new("Default", move || {
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

        let marks = group([
            toggle(
                MarkFamily::Strong,
                MarkKind::Strong,
                lucide::bold(),
                "Bold (Ctrl+B)",
            )
            .into_element(),
            toggle(
                MarkFamily::Emphasis,
                MarkKind::Emphasis,
                lucide::italic(),
                "Italic (Ctrl+I)",
            )
            .into_element(),
            toggle(
                MarkFamily::Underline,
                MarkKind::Underline,
                lucide::underline(),
                "Underline (Ctrl+U)",
            )
            .into_element(),
            toggle(
                MarkFamily::Strikethrough,
                MarkKind::Strikethrough,
                lucide::strikethrough(),
                "Strikethrough (Ctrl+Shift+X)",
            )
            .into_element(),
            toggle(
                MarkFamily::Code,
                MarkKind::Code,
                lucide::code(),
                "Inline code (Ctrl+E)",
            )
            .into_element(),
            toggle(
                MarkFamily::Highlight,
                MarkKind::Highlight,
                lucide::highlighter(),
                "Highlight (Ctrl+Shift+H)",
            )
            .into_element(),
            toggle(
                MarkFamily::Superscript,
                MarkKind::Superscript,
                lucide::superscript(),
                "Superscript (Ctrl+.)",
            )
            .into_element(),
            toggle(
                MarkFamily::Subscript,
                MarkKind::Subscript,
                lucide::subscript(),
                "Subscript (Ctrl+,)",
            )
            .into_element(),
        ]);

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
        let paint = group([
            anchored(
                ToolButton::new(lucide::baseline(), "Text color")
                    .enabled(rich)
                    .swatch(format.color().map(Color::from))
                    .on_press(move || toggle_menu(Menu::Color))
                    .colors(&theme)
                    .into_element(),
                color_menu,
            ),
            anchored(
                ToolButton::new(lucide::paint_bucket(), "Background color")
                    .enabled(rich)
                    .swatch(format.background().map(Color::from))
                    .on_press(move || toggle_menu(Menu::Background))
                    .colors(&theme)
                    .into_element(),
                background_menu,
            ),
            ToolButton::new(lucide::remove_formatting(), "Clear formatting (Ctrl+\\)")
                .enabled(rich && format.has_selection)
                .on_press(move || controller.clear_formatting())
                .colors(&theme)
                .into_element(),
        ]);

        let current_link = format.link().map(|link| link.url.clone());
        let link_menu = (*menu.read() == Some(Menu::Link))
            .then(|| link_popup(&theme, controller, menu, link_draft, current_link.clone()));
        let mut draft_seed = link_draft;
        let seed = current_link.clone().unwrap_or_default();
        let link = group([anchored(
            ToolButton::new(lucide::link(), "Link (Ctrl+K)")
                .active(current_link.is_some())
                .enabled(rich && (format.has_selection || current_link.is_some()))
                .on_press(move || {
                    draft_seed.set(seed.clone());
                    toggle_menu(Menu::Link);
                })
                .colors(&theme)
                .into_element(),
            link_menu,
        )]);

        let align = |value: Align, icon: Bytes, tip: &'static str| {
            ToolButton::new(icon, tip)
                .active(format.target.is_some() && format.align == value)
                .enabled(editing && format.target.is_some_and(|t| t.cell.is_none()))
                .on_press(move || controller.set_align(value))
                .colors(&theme)
                .into_element()
        };
        let alignment = group([
            align(Align::Left, lucide::text_align_start(), "Align left"),
            align(Align::Center, lucide::text_align_center(), "Center"),
            align(Align::Right, lucide::text_align_end(), "Align right"),
            align(Align::Justify, lucide::text_align_justify(), "Justify"),
        ]);

        let list = |block_type: BlockType, icon: Bytes, tip: &'static str| {
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
        let lists = group([
            list(BlockType::BulletList, lucide::list(), "Bulleted list"),
            list(
                BlockType::OrderedList,
                lucide::list_ordered(),
                "Numbered list",
            ),
            list(BlockType::TaskList, lucide::list_todo(), "To-do list"),
            ToolButton::new(lucide::list_indent_decrease(), "Outdent (Shift+Tab)")
                .enabled(editing && in_list)
                .on_press(move || controller.outdent())
                .colors(&theme)
                .into_element(),
            ToolButton::new(lucide::list_indent_increase(), "Indent (Tab)")
                .enabled(editing && in_list)
                .on_press(move || controller.indent())
                .colors(&theme)
                .into_element(),
        ]);

        let insert_picker = Dropdown {
            text: "Insert".to_owned(),
            width: 84.,
            enabled: editing,
            open: *menu.read() == Some(Menu::Insert),
            on_toggle: EventHandler::new(move |()| toggle_menu(Menu::Insert)),
            colors: colors.clone(),
            items: insert_items(controller, menu),
        };

        let mode = self.show_mode_toggle.then(|| {
            ToolButton::new(
                if editing {
                    lucide::eye()
                } else {
                    lucide::pencil()
                },
                if editing {
                    "Switch to reading mode"
                } else {
                    "Switch to editing mode"
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
            .content(Content::wrap_spacing(4.))
            .cross_align(Alignment::center())
            .spacing(4.)
            .child(history)
            .child(separator(&colors))
            .child(block_picker)
            .child(font_picker)
            .child(size_picker)
            .child(separator(&colors))
            .child(marks)
            .child(separator(&colors))
            .child(paint)
            .child(link)
            .child(separator(&colors))
            .child(alignment)
            .child(separator(&colors))
            .child(lists)
            .child(separator(&colors))
            .child(insert_picker)
            .maybe_child(mode.map(|mode| rect().margin((0., 0., 0., 6.)).child(mode)))
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

const fn block_label(block_type: Option<BlockType>) -> &'static str {
    match block_type {
        Some(BlockType::Heading(HeadingLevel::H1)) => "Heading 1",
        Some(BlockType::Heading(HeadingLevel::H2)) => "Heading 2",
        Some(BlockType::Heading(HeadingLevel::H3)) => "Heading 3",
        Some(BlockType::Heading(_)) => "Heading",
        Some(BlockType::Quote) => "Quote",
        Some(BlockType::Callout(_)) => "Callout",
        Some(BlockType::BulletList) => "Bulleted list",
        Some(BlockType::OrderedList) => "Numbered list",
        Some(BlockType::TaskList) => "To-do list",
        Some(BlockType::Code) => "Code",
        Some(BlockType::Math) => "Math",
        Some(BlockType::Term) => "Term",
        Some(BlockType::Definition) => "Definition",
        Some(BlockType::Paragraph) | None => "Text",
    }
}

fn block_items(
    controller: EditorController,
    menu: State<Option<Menu>>,
    current: Option<BlockType>,
) -> Vec<MenuItem> {
    let entries = [
        BlockType::Paragraph,
        BlockType::Heading(HeadingLevel::H1),
        BlockType::Heading(HeadingLevel::H2),
        BlockType::Heading(HeadingLevel::H3),
        BlockType::Quote,
        BlockType::Callout(CalloutKind::Note),
        BlockType::Callout(CalloutKind::Tip),
        BlockType::Callout(CalloutKind::Warning),
        BlockType::Code,
        BlockType::Math,
    ];
    entries
        .into_iter()
        .map(|block_type| {
            let text = match block_type {
                BlockType::Callout(kind) => format!("Callout · {}", kind.title()),
                other => block_label(Some(other)).to_owned(),
            };
            MenuItem::new(text, move || {
                controller.set_block_type(block_type);
                let mut menu = menu;
                menu.set(None);
            })
            .selected(current == Some(block_type))
        })
        .collect()
}

fn insert_items(controller: EditorController, menu: State<Option<Menu>>) -> Vec<MenuItem> {
    let close = move || {
        let mut menu = menu;
        menu.set(None);
    };
    vec![
        MenuItem::new("Table", move || {
            controller.insert_table(3, 3);
            close();
        }),
        MenuItem::new("Divider", move || {
            controller.insert_divider();
            close();
        }),
        MenuItem::new("Code block", move || {
            controller.insert_block(BlockKind::Code {
                language: String::new(),
                code: Inline::default(),
            });
            close();
        }),
        MenuItem::new("Math block", move || {
            controller.insert_block(BlockKind::Math(Inline::default()));
            close();
        }),
        MenuItem::new("Table of contents", move || {
            controller.insert_block(BlockKind::TableOfContents);
            close();
        }),
    ]
}

fn group<const N: usize>(children: [Element; N]) -> Element {
    rect()
        .horizontal()
        .cross_align(Alignment::center())
        .spacing(2.)
        .children(children)
        .into_element()
}

fn separator(colors: &crate::EditorColors) -> Element {
    rect()
        .width(Size::px(1.))
        .height(Size::px(18.))
        .margin((0., 4.))
        .background(colors.border)
        .into_element()
}

/// Child with an optional popup anchored right under it.
fn anchored(child: Element, popup: Option<Element>) -> Element {
    rect()
        .child(child)
        .maybe_child(popup.map(|popup| {
            rect().width(Size::px(0.)).height(Size::px(0.)).child(
                rect()
                    .layer(Layer::Overlay)
                    .position(Position::new_absolute().top(4.).left(0.))
                    .child(popup),
            )
        }))
        .into_element()
}

fn popup_frame(colors: &crate::EditorColors) -> Rect {
    rect()
        .padding(8.)
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
                .child(label().color(colors.muted).font_size(12.).text("Default")),
        )
        .into_element()
}

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
    let action = |text: &'static str, color: Color, on_press: Box<dyn Fn()>| {
        rect()
            .padding((5., 10.))
            .corner_radius(6.)
            .background(colors.popup_selected)
            .cursor(CursorIcon::Pointer)
            .on_press(move |_| on_press())
            .child(label().font_size(12.).color(color).text(text))
    };
    popup_frame(colors)
        .width(Size::px(280.))
        .spacing(8.)
        .child(
            Input::new(draft)
                .placeholder("https://")
                .auto_focus(true)
                .width(Size::fill())
                .on_submit(move |_| apply()),
        )
        .child(
            rect()
                .horizontal()
                .spacing(6.)
                .child(action("Apply", colors.accent, Box::new(apply)))
                .maybe_child(current.map(|_| {
                    action(
                        "Remove",
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
}

/// Square icon button with hover, active and disabled states.
#[derive(Clone, PartialEq)]
struct ToolButton {
    icon: Bytes,
    tip: &'static str,
    active: bool,
    enabled: bool,
    swatch: Option<Color>,
    on_press: Option<EventHandler<()>>,
    colors: Option<ToolButtonColors>,
}

impl ToolButton {
    const fn new(icon: Bytes, tip: &'static str) -> Self {
        Self {
            icon,
            tip,
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
        });
        self
    }
}

impl Component for ToolButton {
    fn render(&self) -> impl IntoElement {
        let mut hovered = use_state(|| false);
        let colors = self.colors.clone().unwrap_or(ToolButtonColors {
            text: Color::WHITE,
            muted: Color::GRAY,
            hover: Color::from_argb(40, 255, 255, 255),
            active: Color::from_argb(60, 110, 168, 254),
            accent: Color::from_rgb(110, 168, 254),
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
            .on_pointer_enter(move |_| hovered.set(true))
            .on_pointer_leave(move |_| hovered.set_if_modified(false))
            .on_press(move |event: Event<PressEventData>| {
                event.stop_propagation();
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
        TooltipContainer::new(Tooltip::new_text(self.tip))
            .delay(Duration::from_millis(450))
            .child(button)
    }
}

#[derive(Clone, PartialEq)]
struct MenuItem {
    text: String,
    selected: bool,
    on_press: EventHandler<()>,
}

impl MenuItem {
    fn new(text: impl Into<String>, on_press: impl FnMut() + 'static) -> Self {
        let mut on_press = on_press;
        Self {
            text: text.into(),
            selected: false,
            on_press: EventHandler::new(move |()| on_press()),
        }
    }

    const fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
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
    colors: crate::EditorColors,
    items: Vec<MenuItem>,
}

impl Component for Dropdown {
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
            .on_pointer_enter(move |_| hovered.set(true))
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
                        .font_size(12.5)
                        .color(foreground)
                        .max_lines(1)
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
                .children(self.items.iter().enumerate().map(|(index, item)| {
                    let on_press = item.on_press.clone();
                    rect()
                        .key(index)
                        .width(Size::fill())
                        .padding((6., 10.))
                        .corner_radius(6.)
                        .background(if item.selected {
                            colors.popup_selected
                        } else {
                            Color::TRANSPARENT
                        })
                        .cursor(CursorIcon::Pointer)
                        .on_press(move |event: Event<PressEventData>| {
                            event.stop_propagation();
                            on_press.call(());
                        })
                        .child(
                            label()
                                .font_size(13.)
                                .color(colors.text)
                                .text(item.text.clone()),
                        )
                        .into_element()
                }))
                .into_element()
        });
        anchored(button, popup)
    }
}

impl ActiveFormat {
    /// Convenience used by the toolbar labels.
    #[must_use]
    pub const fn block_label(&self) -> &'static str {
        block_label(self.block_type)
    }
}
