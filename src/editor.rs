//! The markdown editor as Notein uses it everywhere: notes, canvas text
//! boxes, annotations.
//!
//! On top of [`freya_markdown_editor`] it adds reference chips for
//! `notein://` links, a `/ref` section fed by search, links that open
//! through the shell, the theme of the active palette and the user's
//! markdown-marks setting.

mod labels;

use std::borrow::Cow;
use std::rc::Rc;

use freya::prelude::*;
use freya_markdown_editor::prelude::*;
use notein_plugin::bookmark::SourceRef;
use notein_plugin::services::Placement;
use notein_ui_kit::theme::{Palette, palette};
use notein_ui_kit::{RefChip, tokens};

use crate::deep_link;
use crate::references;
use crate::state::LibraryView;

/// Editor theme for the active palette.
#[must_use]
pub fn theme(p: &Palette) -> EditorTheme {
    let mut theme = if p.dark {
        EditorTheme::dark()
    } else {
        EditorTheme::light()
    };
    theme.body_size = tokens::text::BODY;
    theme.heading_sizes = [22.0, 19.0, 16.5, 15.0, 14.0, 13.5];
    theme.labels = labels::labels();
    let colors = &mut theme.colors;
    colors.text = p.t1;
    colors.muted = p.t2;
    colors.placeholder = p.t3;
    colors.accent = p.accent;
    colors.caret = p.accent;
    colors.selection = p.accent_soft;
    colors.surface = p.s2;
    colors.border = p.line;
    colors.code_background = p.s3;
    colors.code_text = p.t1;
    colors.quote_bar = p.line_strong;
    colors.highlight = p.highlighters[0].with_a(110);
    colors.popup_background = p.s2;
    colors.popup_selected = p.s4;
    theme
}

/// Whether editors show `**markdown**` marks near the caret.
#[must_use]
pub fn show_marks() -> bool {
    crate::settings::SettingsStore::try_get().is_some_and(|s| s.read().notes.show_markdown_marks)
}

/// A configured [`MarkdownEditor`] over `value`.
pub fn editor(value: impl Into<Writable<String>>, controller: EditorController) -> MarkdownEditor {
    configure(MarkdownEditor::new(value).controller(controller))
}

/// Set `editor` up the way every editor in Notein is.
pub fn configure(editor: MarkdownEditor) -> MarkdownEditor {
    editor
        .theme(theme(&palette()))
        .syntax_reveal(show_marks())
        .decorator(SchemeEmbed::new(deep_link::SCHEME, |embed| {
            Chip {
                target: SourceRef::from_link(embed.url),
                zoom: embed.theme.zoom,
            }
            .into_element()
        }))
        .on_link(|url| {
            crate::plugins::navigate::open(&SourceRef::from_link(url), Placement::Preferred);
        })
        .on_input_context(crate::soft_keyboard::input_context)
        .suggestions(
            SuggestionGroup::new("Referencias", move |query| {
                let data = LibraryView::get().data();
                references::suggestions(query, data.as_deref())
                    .into_iter()
                    .map(suggestion)
                    .collect()
            })
            .keyword("ref"),
        )
        .slash_menu()
}

fn suggestion(found: references::Suggestion) -> Suggestion {
    let label = found.label.clone();
    let url = found.target.to_url();
    let action = match &found.target {
        SourceRef::Plugin {
            plugin_id,
            item,
            anchor,
            ..
        } => {
            let (kind, doc, anchor) = (
                plugin_id.clone(),
                notein_plugin::DocId(*item),
                anchor.clone(),
            );
            SuggestionAction::Run(SuggestionRun::new(move |controller| {
                if let Some(kind) = crate::plugins::kind(&kind) {
                    kind.cited(doc, &anchor);
                }
                controller.insert_embed(&label, &url);
            }))
        }
        SourceRef::External { url } => SuggestionAction::InsertLink {
            label: found.label.clone(),
            url: url.clone(),
        },
        SourceRef::Canvas { .. } => SuggestionAction::InsertEmbed {
            label: found.label.clone(),
            url,
        },
    };
    Suggestion::new(found.label, action).detail(found.subtitle)
}

/// Close the surface the editor sits in before a chip navigates away
/// (popovers and cards install it).
#[derive(Clone)]
pub struct CloseOnNavigate(pub Rc<dyn Fn()>);

impl PartialEq for CloseOnNavigate {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for CloseOnNavigate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloseOnNavigate").finish_non_exhaustive()
    }
}

/// Inline chip for a `notein://` reference.
#[derive(Clone, Debug, PartialEq)]
struct Chip {
    target: SourceRef,
    zoom: f32,
}

impl Component for Chip {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let data = LibraryView::get().data();
        let shown = references::display(&self.target, data.as_deref());
        let target = self.target.clone();
        rect()
            .scale((self.zoom, self.zoom))
            .on_pointer_down(move |event: Event<PointerEventData>| {
                event.stop_propagation();
                let in_app = !matches!(target, SourceRef::External { .. });
                crate::plugins::navigate::open(&target, Placement::Preferred);
                if in_app && let Some(close) = try_consume_context::<CloseOnNavigate>() {
                    (close.0)();
                }
            })
            .child(
                RefChip::new(shown.label, shown.icon, shown.color.resolve(&p)).broken(shown.broken),
            )
    }
}

/// A bordered editor field (annotation body, bookmark card).
#[derive(Clone, PartialEq)]
pub struct EditorField {
    value: Writable<String>,
    placeholder: Option<Cow<'static, str>>,
    min_height: f32,
    auto_focus: bool,
}

impl std::fmt::Debug for EditorField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorField").finish_non_exhaustive()
    }
}

impl EditorField {
    #[must_use]
    pub fn new(value: impl Into<Writable<String>>) -> Self {
        Self {
            value: value.into(),
            placeholder: None,
            min_height: 96.0,
            auto_focus: false,
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

    #[must_use]
    pub const fn auto_focus(mut self) -> Self {
        self.auto_focus = true;
        self
    }
}

impl Component for EditorField {
    fn render(&self) -> impl IntoElement {
        let p = palette();
        let controller = use_editor_controller();
        let auto_focus = self.auto_focus;
        use_hook(move || {
            if auto_focus {
                controller.focus_end();
            }
        });
        let focused = controller.has_focus();
        let mut field = editor(self.value.clone(), controller).width(Size::fill());
        if let Some(placeholder) = &self.placeholder {
            field = field.placeholder(placeholder.clone());
        }
        rect()
            .width(Size::fill())
            .min_height(Size::px(self.min_height))
            .padding((10.0, 12.0))
            .background(p.s3)
            .corner_radius(tokens::radius::MD)
            .maybe(focused, |r| {
                r.border(
                    Border::new()
                        .fill(p.accent)
                        .width(2.0)
                        .alignment(BorderAlignment::Inner),
                )
            })
            .child(field)
    }
}
