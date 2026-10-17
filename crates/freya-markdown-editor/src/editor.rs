//! The [`MarkdownEditor`] component.

mod block;
mod paint;
mod suggest;
mod text;

use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use freya::prelude::*;

pub use suggest::{Suggestion, SuggestionAction, Trigger};

use self::block::{BlockView, TocEntry};
use self::suggest::Triggers;
use crate::controller::{EditorController, EditorState};
use crate::decorator::{Decorator, Decorators};
use crate::document::{Abbreviation, BlockKind, Document, Inline};
use crate::markdown::{Dialect, Extensions, ParseOptions};
use crate::theme::EditorTheme;

/// Called with the URL of a pressed link.
#[derive(Clone, Default)]
pub(crate) struct LinkHandler(Option<LinkCallback>);

type LinkCallback = Rc<dyn Fn(&str)>;

impl PartialEq for LinkHandler {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

impl LinkHandler {
    pub(crate) fn call(&self, url: &str) {
        if let Some(handler) = &self.0 {
            handler(url);
        }
    }
}

/// Values every block needs, compared cheaply between renders.
#[derive(Clone, PartialEq)]
pub(crate) struct Shared {
    pub(crate) controller: EditorController,
    pub(crate) theme: Rc<EditorTheme>,
    pub(crate) decorators: Decorators,
    pub(crate) triggers: Triggers,
    pub(crate) on_link: LinkHandler,
    pub(crate) read_only: bool,
    pub(crate) abbreviations: Rc<Vec<Abbreviation>>,
}

/// Block based Markdown editor and viewer.
///
/// The markdown string in [`Self::new`] is the source of truth: it is parsed
/// on mount, re-parsed whenever it changes from outside, and rewritten (in
/// the configured [`Dialect`]) after every edit.
///
/// ```rust,no_run
/// # use freya::prelude::*;
/// # use freya_markdown_editor::prelude::*;
/// fn app() -> impl IntoElement {
///     let notes = use_state(|| String::from("# Hello\n\nStart **typing**"));
///     let controller = use_editor_controller();
///     rect()
///         .child(MarkdownToolbar::new(controller))
///         .child(
///             MarkdownEditor::new(notes)
///                 .controller(controller)
///                 .slash_commands()
///                 .placeholder("Write something…")
///                 .padding(16.)
///                 .expanded(),
///         )
/// }
/// ```
#[derive(Clone, PartialEq)]
pub struct MarkdownEditor {
    value: Option<Writable<String>>,
    controller: Option<EditorController>,
    read_only: Option<bool>,
    dialect: Dialect,
    extensions: Extensions,
    decorators: Decorators,
    theme: Option<EditorTheme>,
    placeholder: Option<Cow<'static, str>>,
    triggers: Triggers,
    on_link: LinkHandler,
    layout: LayoutData,
    text_style: TextStyleData,
    key: DiffKey,
}

impl fmt::Debug for MarkdownEditor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MarkdownEditor")
            .field("dialect", &self.dialect)
            .field("read_only", &self.read_only)
            .field("decorators", &self.decorators)
            .finish_non_exhaustive()
    }
}

impl Default for MarkdownEditor {
    /// An editor driven only by its [`EditorController`].
    fn default() -> Self {
        Self {
            value: None,
            controller: None,
            read_only: None,
            dialect: Dialect::Rich,
            extensions: Extensions::default(),
            decorators: Decorators::default(),
            theme: None,
            placeholder: None,
            triggers: Triggers::default(),
            on_link: LinkHandler::default(),
            layout: LayoutData::default(),
            text_style: TextStyleData::default(),
            key: DiffKey::None,
        }
    }
}

impl MarkdownEditor {
    /// Edit the markdown held by `value`.
    #[must_use]
    pub fn new(value: impl Into<Writable<String>>) -> Self {
        Self {
            value: Some(value.into()),
            ..Self::default()
        }
    }

    /// Share state with toolbars and buttons. Without one the editor owns
    /// a private controller.
    #[must_use]
    pub const fn controller(mut self, controller: EditorController) -> Self {
        self.controller = Some(controller);
        self
    }

    /// Force read only (`true`) or edit mode (`false`). When unset the
    /// controller's mode is used, so it can be toggled at runtime with
    /// [`EditorController::set_read_only`].
    #[must_use]
    pub const fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = Some(read_only);
        self
    }

    /// Dialect used when writing the value back. Defaults to
    /// [`Dialect::Rich`].
    #[must_use]
    pub const fn dialect(mut self, dialect: Dialect) -> Self {
        self.dialect = dialect;
        self
    }

    #[must_use]
    pub const fn extensions(mut self, extensions: Extensions) -> Self {
        self.extensions = extensions;
        self
    }

    #[must_use]
    pub fn decorator(mut self, decorator: impl Decorator + 'static) -> Self {
        self.decorators = self.decorators.with(decorator);
        self
    }

    #[must_use]
    pub fn decorators(mut self, decorators: Decorators) -> Self {
        self.decorators = decorators;
        self
    }

    #[must_use]
    pub fn theme(mut self, theme: EditorTheme) -> Self {
        self.theme = Some(theme);
        self
    }

    /// Hint shown while the document is empty.
    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<Cow<'static, str>>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    #[must_use]
    pub fn trigger(mut self, trigger: Trigger) -> Self {
        Rc::make_mut(&mut self.triggers.0).push(trigger);
        self
    }

    /// Enable the built in `/` block menu.
    #[must_use]
    pub fn slash_commands(self) -> Self {
        self.trigger(Trigger::slash_commands())
    }

    /// Called when a link is pressed: always in read only mode, with Ctrl
    /// (or Cmd) held while editing.
    #[must_use]
    pub fn on_link(mut self, on_link: impl Fn(&str) + 'static) -> Self {
        self.on_link = LinkHandler(Some(Rc::new(on_link)));
        self
    }
}

impl KeyExt for MarkdownEditor {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl LayoutExt for MarkdownEditor {
    fn get_layout(&mut self) -> &mut LayoutData {
        &mut self.layout
    }
}

impl ContainerExt for MarkdownEditor {}

impl TextStyleExt for MarkdownEditor {
    fn get_text_style_data(&mut self) -> &mut TextStyleData {
        &mut self.text_style
    }
}

#[derive(Default)]
struct Sync {
    text: Option<String>,
    revision: u64,
}

impl Component for MarkdownEditor {
    fn render(&self) -> impl IntoElement {
        let context_theme = use_try_consume::<EditorTheme>();
        let theme = Rc::new(self.theme.clone().or(context_theme).unwrap_or_default());
        let parse = ParseOptions {
            extensions: self.extensions,
            embed: self.decorators.embed_filter(),
        };

        let initial = self
            .value
            .as_ref()
            .map(|value| value.peek().clone())
            .unwrap_or_default();
        let initial_parse = parse.clone();
        let owned = use_hook(move || {
            EditorController::create(Document::parse_with(&initial, &initial_parse))
        });
        let controller = self.controller.unwrap_or(owned);
        use_hook(move || controller.set_parse_options(parse));
        if let Some(read_only) = self.read_only {
            controller.set_read_only(read_only);
        }

        use_value_sync(self.value.clone(), controller, self.dialect);

        let view = controller.view();
        let document = &view.document;
        let read_only = view.read_only;
        let shared = Shared {
            controller,
            theme: theme.clone(),
            decorators: self.decorators.clone(),
            triggers: self.triggers.clone(),
            on_link: self.on_link.clone(),
            read_only,
            abbreviations: Rc::new(document.abbreviations.clone()),
        };

        let empty = matches!(
            document.blocks(),
            [only] if matches!(&only.kind, BlockKind::Paragraph(content) if content.is_empty())
        );
        let placeholder = empty
            .then(|| self.placeholder.as_ref().map(ToString::to_string))
            .flatten();
        let toc = toc_entries(document);
        let focused = view.selection.map(|selection| selection.target);

        let blocks = document.blocks().iter().map(|block| {
            let toc = if matches!(block.kind, BlockKind::TableOfContents) {
                toc.clone()
            } else {
                Vec::new()
            };
            rect()
                .key(block.id)
                .width(Size::fill())
                .child(BlockView {
                    block: block.clone(),
                    ordinal: document.ordinal(block.id),
                    caret: view
                        .caret
                        .filter(|request| request.selection.target.block == block.id),
                    focused: focused.filter(|target| target.block == block.id),
                    toc,
                    placeholder: placeholder.clone(),
                    shared: shared.clone(),
                })
                .into_element()
        });

        let mut text_style = TextStyleData {
            color: Some(theme.colors.text.into()),
            font_size: Some(theme.body_size.into()),
            ..TextStyleData::default()
        };
        overlay_text_style(&mut text_style, &self.text_style);

        rect()
            .layout(self.layout.clone())
            .text_style(text_style)
            .on_press(move |_| {
                if !read_only && controller.peek(super::controller::EditorState::selection_is_none)
                {
                    controller.focus_end();
                }
            })
            .children(blocks)
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

fn toc_entries(document: &Document) -> Vec<TocEntry> {
    if !document
        .blocks()
        .iter()
        .any(|block| matches!(block.kind, BlockKind::TableOfContents))
    {
        return Vec::new();
    }
    document
        .headings()
        .map(|(block, level, content)| TocEntry {
            block,
            level,
            text: Inline::plain_text(content),
        })
        .collect()
}

fn overlay_text_style(base: &mut TextStyleData, overlay: &TextStyleData) {
    base.color = overlay.color.clone().or_else(|| base.color.take());
    base.font_size = overlay.font_size.or(base.font_size);
    if !overlay.font_families.is_empty() {
        base.font_families.clone_from(&overlay.font_families);
    }
    base.text_align = overlay.text_align.or(base.text_align);
    base.text_height = overlay.text_height.or(base.text_height);
    base.text_overflow = overlay
        .text_overflow
        .clone()
        .or_else(|| base.text_overflow.take());
    base.text_shadows
        .extend(overlay.text_shadows.iter().copied());
    base.text_decoration = overlay.text_decoration.or(base.text_decoration);
    base.font_slant = overlay.font_slant.or(base.font_slant);
    base.font_weight = overlay.font_weight.or(base.font_weight);
    base.font_width = overlay.font_width.or(base.font_width);
    base.letter_spacing = overlay.letter_spacing.or(base.letter_spacing);
    base.font_features
        .extend(overlay.font_features.iter().cloned());
}

/// Keep `value` and the controller document in sync in both directions:
/// outside writes are parsed, edits are written back in `dialect`.
fn use_value_sync(value: Option<Writable<String>>, controller: EditorController, dialect: Dialect) {
    let sync = use_hook(|| {
        Rc::new(RefCell::new(Sync {
            revision: controller.peek(EditorState::revision),
            ..Sync::default()
        }))
    });
    let dialect = use_reactive(&dialect);

    let incoming = value.clone();
    let incoming_sync = sync.clone();
    use_side_effect(move || {
        let Some(value) = &incoming else {
            return;
        };
        let text = value.read().clone();
        let mut sync = incoming_sync.borrow_mut();
        if sync.text.as_deref() == Some(text.as_str()) {
            return;
        }
        let fresh =
            sync.text.is_none() && controller.peek(|state| state.markdown(*dialect.peek())) == text;
        sync.text = Some(text.clone());
        if !fresh {
            controller.load_markdown(&text);
        }
        sync.revision = controller.peek(EditorState::revision);
    });

    use_side_effect(move || {
        let revision = controller.revision();
        let Some(mut value) = value.clone() else {
            return;
        };
        let mut sync = sync.borrow_mut();
        if revision == sync.revision {
            return;
        }
        sync.revision = revision;
        let markdown = controller.peek(|state| state.markdown(*dialect.peek()));
        if sync.text.as_deref() != Some(markdown.as_str()) {
            sync.text = Some(markdown.clone());
            *value.write() = markdown;
        }
    });
}
