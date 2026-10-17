//! Markdown note editor used by bookmark cards and modals.
//!
//! Built on [`freya_markdown_editor::MarkdownEditor`]; notein only adds
//! what is specific to it:
//!
//! - `notein://` links (see [`crate::deep_link`]) render as source chips
//!   that navigate on tap.
//! - Typing `[` opens the reference picker fed by plugin completions.
//! - Regular links open through the platform URL handler.

use std::borrow::Cow;

use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use notein_plugin::{ChipView, Completion, SourceRef};

use crate::components::theme::{
    BORDER, BORDER_FOCUS, PRIMARY, SURFACE_PRIMARY, SURFACE_SECONDARY, SURFACE_TERTIARY,
    TEXT_PLACEHOLDER, TEXT_PRIMARY, TEXT_SECONDARY,
};
use crate::deep_link::{self, DeepLink};
use crate::source_ref_nav;

const DEFAULT_MIN_HEIGHT: f32 = 72.0;
const CHIP_BG: Color = Color::from_rgb(44, 52, 70);
const CHIP_FG: Color = Color::from_rgb(200, 215, 240);
const CHIP_ICON_SIZE: f32 = 12.0;

/// Context installed by a bookmark popup / modal so chip taps can close
/// the host overlay before navigating elsewhere.
#[derive(Clone)]
pub struct ChipTapCloseCallback(pub std::rc::Rc<dyn Fn()>);

impl PartialEq for ChipTapCloseCallback {
    fn eq(&self, other: &Self) -> bool {
        std::rc::Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for ChipTapCloseCallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChipTapCloseCallback")
            .finish_non_exhaustive()
    }
}

/// Markdown body editor with notein reference chips.
#[derive(Clone, PartialEq)]
pub struct NoteEditor {
    value: Writable<String>,
    placeholder: Option<Cow<'static, str>>,
    min_height: f32,
    completions: Option<Callback<String, Vec<Completion>>>,
}

impl std::fmt::Debug for NoteEditor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoteEditor")
            .field("placeholder", &self.placeholder)
            .field("min_height", &self.min_height)
            .finish_non_exhaustive()
    }
}

impl NoteEditor {
    /// Plain text of a markdown note body, for previews and search.
    #[must_use]
    pub fn plain_text(body: &str) -> String {
        Document::parse(body).plain_text()
    }

    #[must_use]
    pub fn new(value: impl Into<Writable<String>>) -> Self {
        Self {
            value: value.into(),
            placeholder: None,
            min_height: DEFAULT_MIN_HEIGHT,
            completions: None,
        }
    }

    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<Cow<'static, str>>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    #[must_use]
    pub const fn min_height(mut self, height: f32) -> Self {
        self.min_height = height;
        self
    }

    /// Reference picker source, queried with the text typed after `[`.
    #[must_use]
    pub fn completions(mut self, provider: impl Into<Callback<String, Vec<Completion>>>) -> Self {
        self.completions = Some(provider.into());
        self
    }
}

impl Component for NoteEditor {
    fn render(&self) -> impl IntoElement {
        let controller = use_editor_controller();
        let focused = controller.selection().is_some();
        let completions = self.completions.clone();

        let mut editor = MarkdownEditor::new(self.value.clone())
            .controller(controller)
            .theme(note_theme())
            .slash_commands()
            .decorator(SchemeEmbed::new(deep_link::SCHEME, |embed| {
                ChipWidget {
                    src: SourceRef::from_link(embed.url),
                }
                .into_element()
            }))
            .on_link(|url| source_ref_nav::navigate(&SourceRef::from_link(url)))
            .width(Size::fill());
        if let Some(placeholder) = &self.placeholder {
            editor = editor.placeholder(placeholder.clone());
        }
        if let Some(provider) = completions {
            editor = editor.trigger(
                Trigger::new('[', move |query| {
                    provider
                        .call(query.to_owned())
                        .into_iter()
                        .map(suggestion)
                        .collect()
                })
                .anywhere(),
            );
        }

        rect()
            .width(Size::fill())
            .min_height(Size::px(self.min_height))
            .padding((8., 10.))
            .background(SURFACE_SECONDARY)
            .border(
                Border::new()
                    .width(1.)
                    .alignment(BorderAlignment::Inner)
                    .fill(if focused { BORDER_FOCUS } else { BORDER }),
            )
            .corner_radius(8.)
            .child(editor)
    }
}

fn suggestion(completion: Completion) -> Suggestion {
    let action = match &completion.source {
        SourceRef::External { url } => SuggestionAction::InsertLink {
            label: completion.label.clone(),
            url: url.clone(),
        },
        source => SuggestionAction::InsertEmbed {
            label: completion.label.clone(),
            url: source.to_url(),
        },
    };
    let entry = Suggestion::new(completion.label, action);
    match completion.subtitle {
        Some(subtitle) => entry.detail(subtitle),
        None => entry,
    }
}

fn note_theme() -> EditorTheme {
    let mut theme = EditorTheme::dark();
    theme.body_size = 13.5;
    theme.heading_sizes = [22.0, 19.0, 16.5, 15.0, 14.0, 13.5];
    let colors = &mut theme.colors;
    colors.text = TEXT_PRIMARY;
    colors.muted = TEXT_SECONDARY;
    colors.placeholder = TEXT_PLACEHOLDER;
    colors.accent = PRIMARY;
    colors.caret = TEXT_PRIMARY;
    colors.surface = SURFACE_SECONDARY;
    colors.border = BORDER;
    colors.code_background = SURFACE_TERTIARY;
    colors.popup_background = SURFACE_PRIMARY;
    colors.popup_selected = SURFACE_TERTIARY;
    theme
}

/// Inline chip for a `notein://` reference. Plugins may brand it through
/// [`notein_plugin::Source::render_chip`]; tapping navigates to the
/// reference.
#[derive(Clone, PartialEq)]
struct ChipWidget {
    src: SourceRef,
}

impl Component for ChipWidget {
    fn render(&self) -> impl IntoElement {
        let mut hover = use_state(|| false);
        let view = resolve_chip_view(&self.src);
        let base = view.tint.map_or(CHIP_BG, |rgba| {
            Color::from_argb(rgba[3], rgba[0], rgba[1], rgba[2])
        });
        let background = if hover() { lighten(base, 0.12) } else { base };

        let icon = {
            use freya::icons::lucide;
            match &self.src {
                SourceRef::Plugin { plugin_id, .. }
                    if plugin_id == crate::pdf_viewer::PDF_PLUGIN_ID =>
                {
                    Some(lucide::book_text())
                }
                SourceRef::Plugin { plugin_id, .. } if plugin_id == "io.notein.bible" => {
                    Some(lucide::scroll_text())
                }
                SourceRef::External { .. } => Some(lucide::link_2()),
                _ => None,
            }
        };
        let svg = icon.map(|bytes| {
            SvgViewer::new(bytes)
                .color(CHIP_FG)
                .width(Size::px(CHIP_ICON_SIZE))
                .height(Size::px(CHIP_ICON_SIZE))
        });
        let emoji = svg
            .is_none()
            .then(|| view.icon.clone())
            .flatten()
            .map(|icon| label().font_size(11.5).color(CHIP_FG).text(icon));

        let target = self.src.clone();
        // Inline children need explicit sizes, and `on_pointer_down`
        // fires reliably on a pill this small where `on_press` may not.
        rect()
            .height(Size::px(20.))
            .padding((2., 8.))
            .background(background)
            .corner_radius(10.)
            .border(
                Border::new()
                    .width(1.)
                    .alignment(BorderAlignment::Inner)
                    .fill(SURFACE_TERTIARY),
            )
            .cursor(CursorIcon::Pointer)
            .direction(Direction::Horizontal)
            .cross_align(Alignment::center())
            .spacing(6.0)
            .on_pointer_enter(move |_| hover.set(true))
            .on_pointer_leave(move |_| hover.set_if_modified(false))
            .on_pointer_down(move |event: Event<PointerEventData>| {
                event.stop_propagation();
                let in_app = !matches!(target, SourceRef::External { .. });
                source_ref_nav::navigate(&target);
                if in_app && let Some(close) = try_consume_context::<ChipTapCloseCallback>() {
                    (close.0)();
                }
            })
            .maybe_child(svg)
            .maybe_child(emoji)
            .child(label().font_size(11.5).color(CHIP_FG).text(view.label))
    }
}

fn resolve_chip_view(src: &SourceRef) -> ChipView {
    if let SourceRef::Plugin {
        plugin_id, anchor, ..
    } = src
    {
        let registry = crate::plugins::registry();
        let guard = crate::plugins::lock(&registry);
        if let Some(plugin) = guard.get_source(plugin_id)
            && let Some(view) = plugin.render_chip(anchor)
        {
            return view;
        }
    }
    ChipView {
        label: source_ref_nav::chip_label(src),
        icon: None,
        tint: None,
        tooltip: None,
    }
}

/// Blend `color` toward white by `t` (0 = original, 1 = white).
fn lighten(color: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let mix = |channel: u8| {
        let base = f32::from(channel);
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "value is clamped to the u8 range"
        )]
        let mixed = (base + (255.0 - base) * t).round().clamp(0.0, 255.0) as u8;
        mixed
    };
    Color::from_argb(color.a(), mix(color.r()), mix(color.g()), mix(color.b()))
}
