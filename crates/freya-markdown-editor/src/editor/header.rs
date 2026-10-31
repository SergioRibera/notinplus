//! Pressable chips in block headers.
//!
//! A chip names what a block is (a callout kind, a code language) and
//! opens a menu to change it, so nobody has to know the markdown syntax
//! behind it. Read only editors show the same chip without the menu.

use std::rc::Rc;
use std::time::Duration;

use freya::clipboard::Clipboard;
use freya::icons::lucide;
use freya::prelude::*;
use notein_i18n::tr;

use crate::controller::EditorController;
use crate::document::{BlockId, CalloutKind};
use crate::labels::Label;
use crate::popup::{Anchored, MENU_ROW, MenuItem, input_colors, menu_separator, popup_frame};
use crate::theme::EditorTheme;

/// Height of a header chip.
const CHIP: f32 = 28.0;

/// Lucide icon of a callout kind.
#[cfg_attr(feature = "sismo", sismo::function("markdown::callout_icon"))]
pub(crate) fn callout_icon(kind: CalloutKind) -> Bytes {
    match kind {
        CalloutKind::Note => lucide::info(),
        CalloutKind::Tip => lucide::lightbulb(),
        CalloutKind::Important => lucide::message_square_warning(),
        CalloutKind::Warning => lucide::triangle_alert(),
        CalloutKind::Caution => lucide::octagon_alert(),
    }
}

/// What pressing a [`HeaderChip`] opens.
#[derive(Clone, PartialEq)]
pub(crate) enum ChipMenu {
    /// Groups of entries, split by a line.
    Items(Vec<Vec<MenuItem>>),
    /// The language of a code block.
    Language {
        controller: EditorController,
        block: BlockId,
        current: String,
    },
}

/// Icon, caption and, when editable, a chevron that opens a [`ChipMenu`].
#[derive(Clone, PartialEq)]
pub(crate) struct HeaderChip {
    pub(crate) icon: Bytes,
    pub(crate) text: String,
    pub(crate) color: Color,
    pub(crate) theme: Rc<EditorTheme>,
    pub(crate) editable: bool,
    /// Open the menu as soon as the chip mounts, like right after typing
    /// a code fence.
    pub(crate) auto_open: bool,
    pub(crate) menu: ChipMenu,
}

impl Component for HeaderChip {
    #[cfg_attr(feature = "sismo", sismo::function("markdown::header_chip"))]
    fn render(&self) -> impl IntoElement {
        let mut open = use_state(|| self.auto_open && self.editable);
        // A finger opening the picker means looking through the list: its
        // search field waits for a tap instead of raising the keyboard.
        let mut by_touch = use_state(|| false);
        let mut hovered = use_state(|| false);
        let theme = &self.theme;
        let colors = &theme.colors;
        let editable = self.editable;
        let chip = rect()
            .height(Size::px(theme.px(CHIP)))
            .padding((0., theme.px(8.), 0., theme.px(6.)))
            .corner_radius(theme.px(6.))
            .horizontal()
            .spacing(theme.px(6.))
            .cross_align(Alignment::center())
            .background(if editable && (open() || hovered()) {
                self.color.with_a(36)
            } else {
                Color::TRANSPARENT
            })
            .maybe(editable, |chip| {
                chip.cursor(CursorIcon::Pointer)
                    .on_pointer_enter(move |event: Event<PointerEventData>| {
                        hovered.set(matches!(event.data(), PointerEventData::Mouse(_)));
                    })
                    .on_pointer_leave(move |_| hovered.set_if_modified(false))
                    .on_press(move |event: Event<PressEventData>| {
                        event.stop_propagation();
                        by_touch.set(matches!(event.data(), PressEventData::Touch(_)));
                        open.toggle();
                    })
            })
            .child(
                SvgViewer::new(self.icon.clone())
                    .color(self.color)
                    .width(Size::px(theme.px(15.)))
                    .height(Size::px(theme.px(15.))),
            )
            .child(
                label()
                    .color(self.color)
                    .font_size(theme.px(13.))
                    .font_weight(FontWeight::BOLD)
                    .max_lines(1)
                    .text(self.text.clone()),
            )
            .maybe_child(editable.then(|| {
                SvgViewer::new(lucide::chevron_down())
                    .color(self.color.with_a(170))
                    .width(Size::px(theme.px(13.)))
                    .height(Size::px(theme.px(13.)))
            }))
            .into_element();

        let close = EventHandler::new(move |()| open.set(false));
        let popup = (editable && open()).then(|| match &self.menu {
            ChipMenu::Items(groups) => {
                let mut rows = Vec::new();
                for (index, group) in groups.iter().enumerate() {
                    if index > 0 {
                        rows.push(menu_separator(colors));
                    }
                    for item in group {
                        let row = item.row(rows.len(), colors, Some(close.clone()));
                        rows.push(row);
                    }
                }
                popup_frame(colors)
                    .padding(4.)
                    .width(Size::px(220.))
                    .children(rows)
                    .into_element()
            }
            ChipMenu::Language {
                controller,
                block,
                current,
            } => LanguagePicker {
                controller: *controller,
                block: *block,
                current: current.clone(),
                theme: self.theme.clone(),
                focus_search: !by_touch(),
                on_done: close.clone(),
            }
            .into_element(),
        });
        Anchored::new(chip, popup)
            .on_dismiss(move || open.set(false))
            .into_element()
    }
}

/// A language a code block may be written in.
struct Language {
    id: &'static str,
    name: &'static str,
    aliases: &'static [&'static str],
}

const fn language(
    id: &'static str,
    name: &'static str,
    aliases: &'static [&'static str],
) -> Language {
    Language { id, name, aliases }
}

/// Languages offered by the picker, most common first. Any other info
/// string can still be typed.
const LANGUAGES: &[Language] = &[
    language("rust", "Rust", &["rs"]),
    language("python", "Python", &["py"]),
    language("javascript", "JavaScript", &["js", "jsx"]),
    language("typescript", "TypeScript", &["ts", "tsx"]),
    language("json", "JSON", &[]),
    language("bash", "Bash", &["sh", "shell", "zsh", "console"]),
    language("html", "HTML", &[]),
    language("css", "CSS", &["scss"]),
    language("sql", "SQL", &[]),
    language("java", "Java", &[]),
    language("kotlin", "Kotlin", &["kt"]),
    language("swift", "Swift", &[]),
    language("c", "C", &["h"]),
    language("cpp", "C++", &["c++", "hpp", "cc"]),
    language("csharp", "C#", &["cs", "c#"]),
    language("go", "Go", &["golang"]),
    language("php", "PHP", &[]),
    language("ruby", "Ruby", &["rb"]),
    language("dart", "Dart", &[]),
    language("lua", "Lua", &[]),
    language("nix", "Nix", &[]),
    language("yaml", "YAML", &["yml"]),
    language("toml", "TOML", &[]),
    language("xml", "XML", &["svg"]),
    language("markdown", "Markdown", &["md"]),
    language("diff", "Diff", &["patch"]),
    language("mermaid", "Mermaid", &[]),
    language("dot", "Graphviz", &["graphviz"]),
];

impl Language {
    fn find(id: &str) -> Option<&'static Self> {
        let id = id.trim();
        LANGUAGES.iter().find(|language| {
            language.id.eq_ignore_ascii_case(id)
                || language.name.eq_ignore_ascii_case(id)
                || language
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(id))
        })
    }

    fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        self.id.contains(&query)
            || self.name.to_lowercase().contains(&query)
            || self.aliases.iter().any(|alias| alias.contains(&query))
    }
}

/// Name a code block's chip shows for `id`: the known name, the raw info
/// string otherwise.
pub(crate) fn language_name(id: &str) -> Option<&str> {
    if id.trim().is_empty() {
        return None;
    }
    Some(Language::find(id).map_or(id, |language| language.name))
}

/// Info string stored for what the user typed in the picker: the canonical
/// id of a known language, else the text itself without spaces.
fn language_id(typed: &str) -> String {
    Language::find(typed).map_or_else(
        || typed.split_whitespace().collect::<Vec<_>>().join("-"),
        |language| language.id.to_owned(),
    )
}

/// Search field and list to pick (or type) a code block's language.
#[derive(Clone, PartialEq)]
struct LanguagePicker {
    controller: EditorController,
    block: BlockId,
    current: String,
    theme: Rc<EditorTheme>,
    focus_search: bool,
    on_done: EventHandler<()>,
}

impl Component for LanguagePicker {
    #[cfg_attr(feature = "sismo", sismo::function("markdown::language_picker"))]
    fn render(&self) -> impl IntoElement {
        let draft = use_state(String::new);
        let colors = &self.theme.colors;
        let labels = &self.theme.labels;
        let (controller, block) = (self.controller, self.block);
        let pick = {
            let on_done = self.on_done.clone();
            move |id: String| {
                controller.set_code_language(block, &id);
                controller.focus(block, 0);
                on_done.call(());
            }
        };

        let query = draft.read().trim().to_owned();
        let current = Language::find(&self.current).map_or(self.current.as_str(), |l| l.id);
        let matches: Vec<&'static Language> = LANGUAGES
            .iter()
            .filter(|language| query.is_empty() || language.matches(&query))
            .collect();
        // What Enter picks: the language typed, else the first match, else
        // the text itself.
        let best = (!query.is_empty())
            .then(|| Language::find(&query).or_else(|| matches.first().copied()))
            .flatten();
        let mut items = Vec::new();
        if query.is_empty() {
            let pick = pick.clone();
            items.push(
                MenuItem::new(labels.get(Label::PlainText), move || pick(String::new()))
                    .selected(current.is_empty()),
            );
        }
        items.extend(matches.iter().copied().map(|language| {
            let pick = pick.clone();
            let highlighted = best.map_or(language.id == current, |best| best.id == language.id);
            MenuItem::new(language.name, move || pick(language.id.to_owned())).selected(highlighted)
        }));
        if !query.is_empty() && Language::find(&query).is_none() {
            let pick = pick.clone();
            let typed = language_id(&query);
            items.push(
                MenuItem::new(
                    tr!("md-use-language", "language" => typed.clone()),
                    move || pick(typed.clone()),
                )
                .icon(lucide::corner_down_left(), colors.accent),
            );
        }

        let submit = {
            let pick = pick.clone();
            let best = best.map(|language| language.id);
            move |typed: String| {
                let typed = typed.trim();
                if !typed.is_empty() {
                    pick(best.map_or_else(|| language_id(typed), str::to_owned));
                }
            }
        };
        #[allow(clippy::cast_precision_loss, reason = "a handful of rows")]
        let list_height = (items.len() as f32 * MENU_ROW).min(MENU_ROW * 6.5);
        popup_frame(colors)
            .padding(6.)
            .width(Size::px(240.))
            .spacing(6.)
            .child(
                Input::new(draft)
                    .placeholder(labels.get(Label::SearchLanguage).to_owned())
                    .theme_colors(input_colors(colors))
                    .auto_focus(self.focus_search)
                    .width(Size::fill())
                    .on_submit(submit),
            )
            .child(
                ScrollView::new()
                    .width(Size::fill())
                    .height(Size::px(list_height))
                    .children(
                        items
                            .iter()
                            .enumerate()
                            .map(|(index, item)| item.row(index, colors, None)),
                    ),
            )
    }
}

/// Pencil that opens a field to rename what it sits next to.
#[derive(Clone, PartialEq)]
pub(crate) struct RenameButton {
    pub(crate) current: String,
    pub(crate) placeholder: String,
    pub(crate) hint: Option<String>,
    pub(crate) color: Color,
    pub(crate) theme: Rc<EditorTheme>,
    pub(crate) on_rename: EventHandler<String>,
}

impl Component for RenameButton {
    #[cfg_attr(feature = "sismo", sismo::function("markdown::rename_button"))]
    fn render(&self) -> impl IntoElement {
        let mut open = use_state(|| false);
        let mut draft = use_state(String::new);
        let theme = &self.theme;
        let colors = &theme.colors;
        let current = self.current.clone();
        let button = rect()
            .width(Size::px(theme.px(CHIP)))
            .height(Size::px(theme.px(CHIP)))
            .corner_radius(theme.px(6.))
            .center()
            .cursor(CursorIcon::Pointer)
            .background(if open() {
                self.color.with_a(36)
            } else {
                Color::TRANSPARENT
            })
            .on_press(move |event: Event<PressEventData>| {
                event.stop_propagation();
                draft.set(current.clone());
                open.toggle();
            })
            .child(
                SvgViewer::new(lucide::pencil())
                    .color(self.color.with_a(170))
                    .width(Size::px(theme.px(13.)))
                    .height(Size::px(theme.px(13.))),
            )
            .into_element();
        let popup = open().then(|| {
            let on_rename = self.on_rename.clone();
            popup_frame(colors)
                .padding(6.)
                .width(Size::px(260.))
                .spacing(6.)
                .child(
                    Input::new(draft)
                        .placeholder(self.placeholder.clone())
                        .theme_colors(input_colors(colors))
                        .auto_focus(true)
                        .width(Size::fill())
                        .on_submit(move |text: String| {
                            on_rename.call(text);
                            open.set(false);
                        }),
                )
                .maybe_child(
                    self.hint
                        .clone()
                        .map(|hint| label().font_size(11.5).color(colors.muted).text(hint)),
                )
                .into_element()
        });
        Anchored::new(button, popup)
            .on_dismiss(move || open.set(false))
            .into_element()
    }
}

/// Copies a code block to the clipboard and confirms it for a moment.
#[derive(Clone, PartialEq)]
pub(crate) struct CopyButton {
    pub(crate) text: String,
    pub(crate) theme: Rc<EditorTheme>,
}

impl Component for CopyButton {
    #[cfg_attr(feature = "sismo", sismo::function("markdown::copy_button"))]
    fn render(&self) -> impl IntoElement {
        let mut copied = use_state(|| false);
        let theme = &self.theme;
        let colors = &theme.colors;
        let text = self.text.clone();
        let (icon, caption, color) = if copied() {
            (lucide::check(), Label::Copied, colors.tip)
        } else {
            (lucide::copy(), Label::CopyCode, colors.muted)
        };
        rect()
            .height(Size::px(theme.px(CHIP)))
            .padding((0., theme.px(8.)))
            .corner_radius(theme.px(6.))
            .horizontal()
            .spacing(theme.px(6.))
            .cross_align(Alignment::center())
            .cursor(CursorIcon::Pointer)
            .on_press(move |event: Event<PressEventData>| {
                event.stop_propagation();
                if Clipboard::set(text.clone()).is_ok() {
                    copied.set(true);
                    spawn(async move {
                        timer(Duration::from_millis(1500)).await;
                        copied.set_if_modified(false);
                    });
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
    }
}

#[cfg(test)]
mod tests {
    use super::{language_id, language_name};

    #[test]
    fn aliases_resolve_to_the_canonical_id() {
        assert_eq!(language_id("JS"), "javascript");
        assert_eq!(language_id("c++"), "cpp");
        assert_eq!(language_id("Rust"), "rust");
    }

    #[test]
    fn unknown_languages_are_kept_without_spaces() {
        assert_eq!(language_id("my lang"), "my-lang");
        assert_eq!(language_name("zig"), Some("zig"));
        assert_eq!(language_name("ts"), Some("TypeScript"));
        assert_eq!(language_name(" "), None);
    }
}
