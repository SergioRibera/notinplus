//! Shared editor state and the command API used by toolbars, shortcuts and
//! integrators.

use std::ops::Range;
use std::time::{Duration, Instant};

use freya::prelude::*;

use crate::document::{
    Align, Atom, Block, BlockId, BlockKind, BlockType, Caret, CellPos, Document, Inline, Link,
    MarkFamily, MarkKind, Rgba, Table, TextRole, TextTarget, utf16_len,
};
use crate::editor::SuggestionAction;
use crate::markdown::{Dialect, ParseOptions};

const HISTORY_LIMIT: usize = 256;
const TYPING_GROUP: Duration = Duration::from_millis(900);

/// Selection inside a single text target, in UTF-16 code units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub target: TextTarget,
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    #[must_use]
    pub const fn caret(target: TextTarget, offset: usize) -> Self {
        Self {
            target,
            anchor: offset,
            head: offset,
        }
    }

    #[must_use]
    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    #[must_use]
    pub const fn is_collapsed(&self) -> bool {
        self.anchor == self.head
    }
}

impl From<Caret> for Selection {
    fn from(caret: Caret) -> Self {
        Self::caret(caret.target, caret.offset)
    }
}

/// Request for the editor owning `selection.target` to move its caret and
/// take focus. `seq` grows monotonically so each request applies once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CaretRequest {
    pub(crate) seq: u64,
    pub(crate) selection: Selection,
}

pub(crate) struct EditorView {
    pub(crate) document: Document,
    pub(crate) selection: Option<Selection>,
    pub(crate) caret: Option<CaretRequest>,
    pub(crate) read_only: bool,
}

/// Formatting at the current selection, for toolbars.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent flags a toolbar reads to enable its buttons"
)]
pub struct ActiveFormat {
    pub target: Option<TextTarget>,
    pub block_type: Option<BlockType>,
    pub role: Option<TextRole>,
    pub align: Align,
    pub in_table: bool,
    pub has_selection: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub read_only: bool,
    marks: Vec<MarkKind>,
}

impl ActiveFormat {
    #[must_use]
    pub fn has(&self, family: MarkFamily) -> bool {
        self.marks.iter().any(|mark| mark.family() == family)
    }

    #[must_use]
    pub fn mark(&self, family: MarkFamily) -> Option<&MarkKind> {
        self.marks.iter().find(|mark| mark.family() == family)
    }

    #[must_use]
    pub fn link(&self) -> Option<&Link> {
        match self.mark(MarkFamily::Link) {
            Some(MarkKind::Link(link)) => Some(link),
            _ => None,
        }
    }

    #[must_use]
    pub fn color(&self) -> Option<Rgba> {
        match self.mark(MarkFamily::Color) {
            Some(MarkKind::Color(color)) => Some(*color),
            _ => None,
        }
    }

    #[must_use]
    pub fn background(&self) -> Option<Rgba> {
        match self.mark(MarkFamily::Background) {
            Some(MarkKind::Background(color)) => Some(*color),
            _ => None,
        }
    }

    #[must_use]
    pub fn font_family(&self) -> Option<&str> {
        match self.mark(MarkFamily::FontFamily) {
            Some(MarkKind::FontFamily(family)) => Some(family),
            _ => None,
        }
    }

    #[must_use]
    pub fn font_size(&self) -> Option<u16> {
        match self.mark(MarkFamily::FontSize) {
            Some(MarkKind::FontSize(size)) => Some(*size),
            _ => None,
        }
    }

    /// Whether inline formatting commands make sense right now.
    #[must_use]
    pub fn can_format(&self) -> bool {
        !self.read_only && self.role == Some(TextRole::Rich)
    }
}

/// Formatting toggled with a collapsed caret, applied to the next insert.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pending {
    Add(MarkKind),
    Remove(MarkFamily),
}

#[derive(Clone)]
struct Snapshot {
    document: Document,
    selection: Option<Selection>,
}

#[derive(Default)]
struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    group: Option<(TextTarget, Instant)>,
}

pub(crate) struct EditorState {
    document: Document,
    revision: u64,
    selection: Option<Selection>,
    caret: Option<CaretRequest>,
    caret_seq: u64,
    pending: Vec<Pending>,
    history: History,
    read_only: bool,
    parse: ParseOptions,
    /// Rich copy of the last text copied from the editor, reused on paste
    /// when the system clipboard still holds the same plain text.
    clipboard: Option<(String, Inline)>,
}

impl EditorState {
    fn new(document: Document) -> Self {
        Self {
            document,
            revision: 0,
            selection: None,
            caret: None,
            caret_seq: 0,
            pending: Vec::new(),
            history: History::default(),
            read_only: false,
            parse: ParseOptions::default(),
            clipboard: None,
        }
    }

    pub(crate) const fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn markdown(&self, dialect: Dialect) -> String {
        self.document.to_markdown(dialect)
    }

    pub(crate) fn focused_target(&self) -> Option<TextTarget> {
        self.selection.map(|selection| selection.target)
    }

    pub(crate) const fn selection_is_none(&self) -> bool {
        self.selection.is_none()
    }

    fn record(&mut self, group: Option<TextTarget>) {
        let now = Instant::now();
        let grouped = matches!(
            (group, self.history.group),
            (Some(target), Some((last, at))) if target == last && now.duration_since(at) < TYPING_GROUP
        );
        if !grouped {
            self.history.undo.push(Snapshot {
                document: self.document.clone(),
                selection: self.selection,
            });
            if self.history.undo.len() > HISTORY_LIMIT {
                self.history.undo.remove(0);
            }
        }
        self.history.group = group.map(|target| (target, now));
        self.history.redo.clear();
    }

    const fn changed(&mut self) {
        self.revision += 1;
    }

    const fn request_caret(&mut self, selection: Selection) {
        self.caret_seq += 1;
        self.caret = Some(CaretRequest {
            seq: self.caret_seq,
            selection,
        });
        self.selection = Some(selection);
    }

    /// Re-issue the current selection so a remounted editor picks it up.
    const fn refresh_caret(&mut self) {
        if let Some(selection) = self.selection {
            self.request_caret(selection);
        }
    }

    fn rich_selection(&self) -> Option<Selection> {
        self.selection
            .filter(|selection| self.document.role(selection.target) == Some(TextRole::Rich))
    }

    /// The target to act on, defaulting to the end of the document.
    fn target_or_last(&self) -> Option<TextTarget> {
        self.selection
            .map(|selection| selection.target)
            .or_else(|| self.document.targets().last().copied())
    }

    fn apply_mark(&mut self, apply: impl FnOnce(&mut Inline, Range<usize>), pending: Pending) {
        let Some(selection) = self.rich_selection() else {
            return;
        };
        if selection.is_collapsed() {
            self.pending.retain(|existing| match (existing, &pending) {
                (Pending::Add(a), Pending::Add(b)) => a.family() != b.family(),
                (Pending::Add(a), Pending::Remove(family))
                | (Pending::Remove(family), Pending::Add(a)) => a.family() != *family,
                (Pending::Remove(a), Pending::Remove(b)) => a != b,
            });
            self.pending.push(pending);
            self.refresh_caret();
            return;
        }
        self.record(None);
        if let Some(inline) = self.document.inline_mut(selection.target) {
            apply(inline, selection.range());
        }
        self.changed();
        self.refresh_caret();
    }

    fn active_format(&self) -> ActiveFormat {
        let mut format = ActiveFormat {
            can_undo: !self.history.undo.is_empty(),
            can_redo: !self.history.redo.is_empty(),
            read_only: self.read_only,
            ..ActiveFormat::default()
        };
        let Some(selection) = self.selection else {
            return format;
        };
        let Some(block) = self.document.block(selection.target.block) else {
            return format;
        };
        format.target = Some(selection.target);
        format.block_type = block.kind.block_type();
        format.role = self.document.role(selection.target);
        format.align = block.align;
        format.in_table = selection.target.cell.is_some();
        format.has_selection = !selection.is_collapsed();

        let Some(inline) = self.document.inline(selection.target) else {
            return format;
        };
        let range = selection.range();
        let families = [
            MarkFamily::Strong,
            MarkFamily::Emphasis,
            MarkFamily::Underline,
            MarkFamily::Strikethrough,
            MarkFamily::Code,
            MarkFamily::Highlight,
            MarkFamily::Superscript,
            MarkFamily::Subscript,
            MarkFamily::Spoiler,
        ];
        format.marks = families
            .into_iter()
            .filter(|family| inline.covers(range.clone(), *family))
            .filter_map(|family| inline.find_mark(range.clone(), family).cloned())
            .collect();
        for family in [
            MarkFamily::Link,
            MarkFamily::Color,
            MarkFamily::Background,
            MarkFamily::FontFamily,
            MarkFamily::FontSize,
        ] {
            if let Some(kind) = inline.find_mark(range.clone(), family) {
                format.marks.push(kind.clone());
            }
        }
        if selection.is_collapsed() {
            for pending in &self.pending {
                match pending {
                    Pending::Add(kind) => {
                        format.marks.retain(|mark| mark.family() != kind.family());
                        format.marks.push(kind.clone());
                    }
                    Pending::Remove(family) => {
                        format.marks.retain(|mark| mark.family() != *family);
                    }
                }
            }
        }
        format
    }
}

/// Handle to the shared state of a [`MarkdownEditor`](crate::MarkdownEditor).
///
/// It is `Copy`: create it with [`use_editor_controller`], hand it to the
/// editor with [`MarkdownEditor::controller`](crate::MarkdownEditor::controller)
/// and to any toolbar or button that needs to drive the editor.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct EditorController {
    state: State<EditorState>,
}

impl std::fmt::Debug for EditorController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorController").finish_non_exhaustive()
    }
}

/// Create a controller owned by the calling component.
#[must_use]
pub fn use_editor_controller() -> EditorController {
    use_hook(|| EditorController::create(Document::default()))
}

/// Create a controller seeded with a markdown string.
pub fn use_editor_controller_with(markdown: impl FnOnce() -> String) -> EditorController {
    use_hook(|| EditorController::create(Document::parse(&markdown())))
}

impl EditorController {
    /// Create a controller outside of a hook, e.g. in a root context.
    #[must_use]
    pub fn create(document: Document) -> Self {
        Self {
            state: State::create(EditorState::new(document)),
        }
    }

    fn update<R>(&self, update: impl FnOnce(&mut EditorState) -> R) -> R {
        let mut state = self.state;
        let mut guard = state.write();
        update(&mut guard)
    }

    fn read<R>(&self, read: impl FnOnce(&EditorState) -> R) -> R {
        read(&self.state.read())
    }

    pub(crate) fn peek<R>(&self, read: impl FnOnce(&EditorState) -> R) -> R {
        read(&self.state.peek())
    }

    // Queries. Reading through these subscribes the calling component.

    /// Clone of the current document.
    #[must_use]
    pub fn document(&self) -> Document {
        self.read(|state| state.document.clone())
    }

    /// Run `read` against the current document without cloning it.
    pub fn with_document<R>(&self, read: impl FnOnce(&Document) -> R) -> R {
        self.read(|state| read(&state.document))
    }

    #[must_use]
    pub fn markdown(&self, dialect: Dialect) -> String {
        self.read(|state| state.document.to_markdown(dialect))
    }

    /// Grows by one on every document change.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.read(|state| state.revision)
    }

    #[must_use]
    pub fn selection(&self) -> Option<Selection> {
        self.read(|state| state.selection)
    }

    #[must_use]
    pub fn active_format(&self) -> ActiveFormat {
        self.read(EditorState::active_format)
    }

    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.read(|state| state.read_only)
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.read(|state| !state.history.undo.is_empty())
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.read(|state| !state.history.redo.is_empty())
    }

    // Content.

    /// Replace the document with parsed markdown, dropping history.
    pub fn load_markdown(&self, markdown: &str) {
        self.update(|state| {
            state.document = Document::parse_with(markdown, &state.parse);
            state.history = History::default();
            state.selection = None;
            state.pending.clear();
            state.changed();
        });
    }

    /// Replace the document, keeping it undoable.
    pub fn set_document(&self, document: Document) {
        self.update(|state| {
            state.record(None);
            state.document = document;
            state.selection = None;
            state.changed();
        });
    }

    /// Options used by [`Self::load_markdown`] and markdown paste.
    pub fn set_parse_options(&self, options: ParseOptions) {
        self.update(|state| state.parse = options);
    }

    pub fn set_read_only(&self, read_only: bool) {
        if self.peek(|state| state.read_only) != read_only {
            self.update(|state| state.read_only = read_only);
        }
    }

    pub fn toggle_read_only(&self) {
        self.update(|state| state.read_only = !state.read_only);
    }

    // Inline formatting.

    /// Toggle a mark over the selection (or for the next typed text).
    pub fn toggle_mark(&self, kind: MarkKind) {
        let active = self.peek(|state| state.active_format().has(kind.family()));
        if active {
            self.clear_mark(kind.family());
        } else {
            self.set_mark(kind);
        }
    }

    pub fn set_mark(&self, kind: MarkKind) {
        self.update(|state| {
            let pending = Pending::Add(kind.clone());
            state.apply_mark(|inline, range| inline.add_mark(range, kind), pending);
        });
    }

    pub fn clear_mark(&self, family: MarkFamily) {
        self.update(|state| {
            state.apply_mark(
                |inline, range| inline.remove_family(range, family),
                Pending::Remove(family),
            );
        });
    }

    /// Remove every mark from the selection.
    pub fn clear_formatting(&self) {
        self.update(|state| {
            let Some(selection) = state.rich_selection() else {
                return;
            };
            state.pending.clear();
            if selection.is_collapsed() {
                return;
            }
            state.record(None);
            if let Some(inline) = state.document.inline_mut(selection.target) {
                let families: Vec<MarkFamily> = inline
                    .marks()
                    .iter()
                    .map(|mark| mark.kind.family())
                    .collect();
                for family in families {
                    inline.remove_family(selection.range(), family);
                }
            }
            state.changed();
            state.refresh_caret();
        });
    }

    pub fn set_color(&self, color: Option<Rgba>) {
        match color {
            Some(color) => self.set_mark(MarkKind::Color(color)),
            None => self.clear_mark(MarkFamily::Color),
        }
    }

    pub fn set_background(&self, color: Option<Rgba>) {
        match color {
            Some(color) => self.set_mark(MarkKind::Background(color)),
            None => self.clear_mark(MarkFamily::Background),
        }
    }

    pub fn set_font_family(&self, family: Option<String>) {
        match family {
            Some(family) => self.set_mark(MarkKind::FontFamily(family)),
            None => self.clear_mark(MarkFamily::FontFamily),
        }
    }

    pub fn set_font_size(&self, size: Option<u16>) {
        match size {
            Some(size) => self.set_mark(MarkKind::FontSize(size)),
            None => self.clear_mark(MarkFamily::FontSize),
        }
    }

    /// Link the selection to `url`, or unlink it with `None`. With a
    /// collapsed caret inside a link the whole link is updated.
    pub fn set_link(&self, url: Option<String>) {
        self.update(|state| {
            let Some(selection) = state.rich_selection() else {
                return;
            };
            let Some(inline) = state.document.inline(selection.target) else {
                return;
            };
            let range = if selection.is_collapsed() {
                inline
                    .mark_at(selection.head.saturating_sub(1), MarkFamily::Link)
                    .or_else(|| inline.mark_at(selection.head, MarkFamily::Link))
                    .map(|mark| mark.range.clone())
            } else {
                Some(selection.range())
            };
            let Some(range) = range else {
                return;
            };
            state.record(None);
            if let Some(inline) = state.document.inline_mut(selection.target) {
                match url {
                    Some(url) => inline.add_mark(range, MarkKind::Link(Link::new(url))),
                    None => inline.remove_family(range, MarkFamily::Link),
                }
            }
            state.changed();
            state.refresh_caret();
        });
    }

    // Insertion.

    /// Insert plain text at the caret, replacing the selection.
    pub fn insert_text(&self, text: &str) {
        self.insert_inline(Inline::from(text));
    }

    /// Insert `label` linked to `url`.
    pub fn insert_link(&self, label: &str, url: &str) {
        let mut inline = Inline::from(label);
        inline.add_mark(0..inline.len(), MarkKind::Link(Link::new(url)));
        self.insert_inline(inline);
    }

    /// Insert an atomic embed (a link claimed by a decorator).
    pub fn insert_embed(&self, label: &str, url: &str) {
        self.insert_atom(Atom::Embed {
            url: url.to_owned(),
            label: label.to_owned(),
            title: None,
        });
    }

    pub fn insert_atom(&self, atom: Atom) {
        let mut inline = Inline::default();
        inline.insert_atom(0, atom);
        self.insert_inline(inline);
    }

    /// Insert rich inline content at the caret.
    pub fn insert_inline(&self, content: Inline) {
        self.update(|state| {
            let Some(target) = state.target_or_last() else {
                return;
            };
            let selection = state
                .selection
                .filter(|selection| selection.target == target)
                .unwrap_or_else(|| {
                    let end = state.document.inline(target).map_or(0, Inline::len);
                    Selection::caret(target, end)
                });
            let plain = state.document.role(target) == Some(TextRole::Plain);
            state.record(None);
            let Some(inline) = state.document.inline_mut(target) else {
                return;
            };
            let range = selection.range();
            inline.replace(range.clone(), "");
            let length = content.len();
            if plain {
                inline.replace(range.start..range.start, &content.plain_text());
            } else {
                inline.insert_inline(range.start, content);
            }
            state.changed();
            state.request_caret(Selection::caret(target, range.start + length));
        });
    }

    /// Paste markdown at the caret: one paragraph is inserted inline, more
    /// blocks split the current one around them.
    pub fn insert_markdown(&self, markdown: &str) {
        self.update(|state| {
            let parsed = Document::parse_with(markdown, &state.parse);
            let Some(selection) = state.selection else {
                return;
            };
            let target = selection.target;
            let blocks = parsed.blocks();
            let single = match blocks {
                [only] => match &only.kind {
                    BlockKind::Paragraph(content) => Some(content.clone()),
                    _ => None,
                },
                _ => None,
            };
            let rich = state.document.role(target) == Some(TextRole::Rich);
            if let Some(content) = single.filter(|_| rich) {
                state.record(None);
                if let Some(inline) = state.document.inline_mut(target) {
                    let range = selection.range();
                    inline.replace(range.clone(), "");
                    let end = range.start + content.len();
                    inline.insert_inline(range.start, content);
                    state.changed();
                    state.request_caret(Selection::caret(target, end));
                }
                return;
            }
            if target.cell.is_some() || !rich {
                return;
            }
            state.record(None);
            let range = selection.range();
            if let Some(inline) = state.document.inline_mut(target) {
                inline.replace(range.clone(), "");
            }
            let tail = state
                .document
                .split(target, range.start)
                .map(|caret| caret.target.block);
            let mut anchor = target.block;
            for block in blocks {
                anchor = state.document.insert_after(
                    anchor,
                    Block {
                        id: BlockId::fresh(),
                        ..block.clone()
                    },
                );
            }
            let caret = tail.map_or_else(
                || Selection::caret(TextTarget::from(anchor), 0),
                |tail| Selection::caret(TextTarget::from(tail), 0),
            );
            state.document.ensure_editable();
            state.changed();
            state.request_caret(caret);
        });
    }

    // Blocks.

    pub fn set_block_type(&self, block_type: BlockType) {
        self.update(|state| {
            let Some(selection) = state.selection.filter(|s| s.target.cell.is_none()) else {
                return;
            };
            let mut document = state.document.clone();
            if document.set_type(selection.target.block, block_type) {
                state.record(None);
                state.document = document;
                state.changed();
                state.refresh_caret();
            }
        });
    }

    pub fn set_align(&self, align: Align) {
        self.update(|state| {
            let Some(selection) = state.selection else {
                return;
            };
            state.record(None);
            if let Some(block) = state.document.block_mut(selection.target.block) {
                block.align = align;
            }
            state.changed();
            state.refresh_caret();
        });
    }

    pub fn indent(&self) {
        self.shift_depth(1);
    }

    pub fn outdent(&self) {
        self.shift_depth(-1);
    }

    fn shift_depth(&self, delta: i8) {
        self.update(|state| {
            let Some(selection) = state.selection else {
                return;
            };
            let mut document = state.document.clone();
            if document.indent(selection.target.block, delta) {
                state.record(None);
                state.document = document;
                state.changed();
                state.refresh_caret();
            }
        });
    }

    pub fn toggle_task(&self, block: BlockId) {
        self.update(|state| {
            if state.read_only {
                return;
            }
            state.record(None);
            if state.document.toggle_task(block) {
                state.changed();
            }
        });
    }

    pub fn set_code_language(&self, block: BlockId, language: &str) {
        self.update(|state| {
            state.record(None);
            if let Some(BlockKind::Code {
                language: current, ..
            }) = state.document.block_mut(block).map(|block| &mut block.kind)
            {
                language.clone_into(current);
                state.changed();
            }
        });
    }

    /// Insert a block after the current one (replacing it when it is an
    /// empty paragraph) and move the caret into it.
    pub fn insert_block(&self, kind: BlockKind) {
        self.update(|state| {
            state.record(None);
            let current = state.selection.map(|selection| selection.target.block);
            let replace = current.and_then(|id| {
                state
                    .document
                    .block(id)
                    .filter(|block| {
                        matches!(&block.kind, BlockKind::Paragraph(content) if content.is_empty())
                    })
                    .map(|block| block.id)
            });
            let block = Block::from(kind);
            let id = match (replace, current) {
                (Some(id), _) => {
                    if let Some(slot) = state.document.block_mut(id) {
                        slot.kind = block.kind;
                    }
                    id
                }
                (None, Some(anchor)) => state.document.insert_after(anchor, block),
                (None, None) => {
                    let end = state.document.blocks().len();
                    state.document.insert(end, block)
                }
            };
            state.document.ensure_editable();
            let caret = first_target_from(&state.document, id);
            state.changed();
            if let Some(target) = caret {
                state.request_caret(Selection::caret(target, 0));
            }
        });
    }

    pub fn insert_table(&self, columns: usize, rows: usize) {
        self.insert_block(BlockKind::Table(Table::new(columns, rows)));
    }

    pub fn insert_divider(&self) {
        self.insert_block(BlockKind::Divider);
    }

    pub fn remove_block(&self, block: BlockId) {
        self.update(|state| {
            state.record(None);
            let next = state
                .document
                .index_of(block)
                .and_then(|index| state.document.blocks().get(index + 1))
                .map(|next| next.id);
            state.document.remove(block);
            state.changed();
            let target = next
                .and_then(|next| first_target_from(&state.document, next))
                .or_else(|| state.document.targets().last().copied());
            if let Some(target) = target {
                state.request_caret(Selection::caret(target, 0));
            }
        });
    }

    pub fn move_block(&self, block: BlockId, delta: isize) {
        self.update(|state| {
            let mut document = state.document.clone();
            if document.move_block(block, delta) {
                state.record(None);
                state.document = document;
                state.changed();
                state.refresh_caret();
            }
        });
    }

    pub fn table_insert_row(&self) {
        self.edit_table(|table, cell| {
            table.insert_row(cell.row + 1);
            Some(CellPos {
                row: cell.row + 1,
                column: cell.column,
            })
        });
    }

    pub fn table_insert_column(&self) {
        self.edit_table(|table, cell| {
            table.insert_column(cell.column + 1);
            Some(CellPos {
                row: cell.row,
                column: cell.column + 1,
            })
        });
    }

    pub fn table_remove_row(&self) {
        self.edit_table(|table, cell| {
            table.remove_row(cell.row).then(|| CellPos {
                row: cell.row.min(table.rows().len() - 1),
                column: cell.column,
            })
        });
    }

    pub fn table_remove_column(&self) {
        self.edit_table(|table, cell| {
            table.remove_column(cell.column).then(|| CellPos {
                row: cell.row,
                column: cell.column.min(table.columns().len() - 1),
            })
        });
    }

    fn edit_table(&self, edit: impl FnOnce(&mut Table, CellPos) -> Option<CellPos>) {
        self.update(|state| {
            let Some(selection) = state.selection else {
                return;
            };
            let Some(cell) = selection.target.cell else {
                return;
            };
            let mut document = state.document.clone();
            let Some(BlockKind::Table(table)) = document
                .block_mut(selection.target.block)
                .map(|block| &mut block.kind)
            else {
                return;
            };
            let Some(next) = edit(table, cell) else {
                return;
            };
            state.record(None);
            state.document = document;
            state.changed();
            state.request_caret(Selection::caret(
                TextTarget {
                    block: selection.target.block,
                    cell: Some(next),
                },
                0,
            ));
        });
    }

    // History.

    pub fn undo(&self) {
        self.update(|state| {
            let Some(snapshot) = state.history.undo.pop() else {
                return;
            };
            state.history.redo.push(Snapshot {
                document: std::mem::replace(&mut state.document, snapshot.document),
                selection: state.selection,
            });
            state.history.group = None;
            state.pending.clear();
            state.changed();
            match snapshot.selection {
                Some(selection) => state.request_caret(clamp(&state.document, selection)),
                None => state.selection = None,
            }
        });
    }

    pub fn redo(&self) {
        self.update(|state| {
            let Some(snapshot) = state.history.redo.pop() else {
                return;
            };
            state.history.undo.push(Snapshot {
                document: std::mem::replace(&mut state.document, snapshot.document),
                selection: state.selection,
            });
            state.history.group = None;
            state.changed();
            match snapshot.selection {
                Some(selection) => state.request_caret(clamp(&state.document, selection)),
                None => state.selection = None,
            }
        });
    }

    // Focus.

    /// Move the caret to `offset` inside `target` and focus it.
    pub fn focus(&self, target: impl Into<TextTarget>, offset: usize) {
        let target = target.into();
        self.update(|state| {
            state.request_caret(clamp(&state.document, Selection::caret(target, offset)));
        });
    }

    pub fn focus_start(&self) {
        if let Some(target) = self.peek(|state| state.document.targets().first().copied()) {
            self.focus(target, 0);
        }
    }

    pub fn focus_end(&self) {
        let end = self.peek(|state| {
            let target = state.document.targets().last().copied()?;
            Some((target, state.document.inline(target).map_or(0, Inline::len)))
        });
        if let Some((target, offset)) = end {
            self.focus(target, offset);
        }
    }

    // Internal hooks used by the text editors.

    /// Everything the editor root needs for one render, subscribing to it.
    pub(crate) fn view(&self) -> EditorView {
        self.read(|state| EditorView {
            document: state.document.clone(),
            selection: state.selection,
            caret: state.caret,
            read_only: state.read_only,
        })
    }

    pub(crate) fn set_selection(&self, selection: Selection) {
        let current = self.peek(|state| state.selection);
        if current == Some(selection) {
            return;
        }
        self.update(|state| {
            let moved = state.selection.is_none_or(|previous| {
                previous.target != selection.target || previous.head != selection.head
            });
            if moved {
                state.pending.clear();
                state.history.group = None;
            }
            state.selection = Some(selection);
        });
    }

    /// Apply the new rope text of `target`. Returns whether the model text
    /// now differs from what the caller typed (autoformat or shortcuts), so
    /// the caller must resync from the model.
    pub(crate) fn edit_text(
        &self,
        target: TextTarget,
        text: &str,
        hint: usize,
        selection: Selection,
    ) -> bool {
        self.update(|state| {
            let Some(current) = state.document.inline(target) else {
                return false;
            };
            if current.text() == text {
                state.selection = Some(selection);
                return false;
            }
            state.record(Some(target));
            let role = state.document.role(target);
            let pending = std::mem::take(&mut state.pending);
            let Some(inline) = state.document.inline_mut(target) else {
                return false;
            };
            let Some(change) = inline.apply_text(text, hint) else {
                return false;
            };
            let inserted_len = utf16_len(&change.inserted);
            let inserted = change.range.start..change.range.start + inserted_len;
            let mut selection = selection;
            let mut resync = false;

            if role == Some(TextRole::Rich) {
                if !inserted.is_empty() {
                    for pending in pending {
                        match pending {
                            Pending::Add(kind) => inline.add_mark(inserted.clone(), kind),
                            Pending::Remove(family) => {
                                inline.remove_family(inserted.clone(), family);
                            }
                        }
                    }
                }
                if inserted_len == 1
                    && let Some((cursor, family)) = inline.autoformat(inserted.end)
                {
                    selection = Selection::caret(target, cursor);
                    state.pending.push(Pending::Remove(family));
                    resync = true;
                }
            }

            let shortcut =
                (role == Some(TextRole::Rich) && target.cell.is_none() && inserted_len > 0)
                    .then(|| {
                        state
                            .document
                            .apply_block_shortcut(target.block, selection.head)
                    })
                    .flatten();
            state.changed();
            if let Some(caret) = shortcut {
                state.request_caret(caret.into());
                return true;
            }
            if resync {
                state.request_caret(selection);
            } else {
                state.selection = Some(selection);
            }
            resync
        })
    }

    /// Enter: split the block, or open a code block on `` ```lang ``.
    pub(crate) fn split(&self, target: TextTarget, selection: Range<usize>) {
        self.update(|state| {
            state.record(None);
            if let Some(inline) = state.document.inline_mut(target) {
                inline.replace(selection.clone(), "");
            }
            let fence = state
                .document
                .block(target.block)
                .and_then(|block| match &block.kind {
                    BlockKind::Paragraph(content) => content
                        .text()
                        .strip_prefix("```")
                        .filter(|language| !language.contains(char::is_whitespace))
                        .map(str::to_owned),
                    _ => None,
                });
            if let Some(language) = fence {
                if let Some(block) = state.document.block_mut(target.block) {
                    block.kind = BlockKind::Code {
                        language,
                        code: Inline::default(),
                    };
                }
                state.changed();
                state.request_caret(Selection::caret(target, 0));
                return;
            }
            let caret = state.document.split(target, selection.start);
            state.changed();
            if let Some(caret) = caret {
                state.request_caret(caret.into());
            }
        });
    }

    pub(crate) fn join_backward(&self, target: TextTarget) -> bool {
        self.structural(|document| document.join_backward(target))
    }

    pub(crate) fn join_forward(&self, target: TextTarget) -> bool {
        self.structural(|document| document.join_forward(target))
    }

    fn structural(&self, edit: impl FnOnce(&mut Document) -> Option<Caret>) -> bool {
        self.update(|state| {
            let mut document = state.document.clone();
            let Some(caret) = edit(&mut document) else {
                return false;
            };
            state.record(None);
            state.document = document;
            state.changed();
            state.request_caret(caret.into());
            true
        })
    }

    /// Move the caret to the previous (`forward == false`) or next target.
    pub(crate) fn step(&self, target: TextTarget, forward: bool) -> bool {
        let destination = self.peek(|state| {
            let next = if forward {
                state.document.next_target(target)
            } else {
                state.document.previous_target(target)
            }?;
            let offset = if forward {
                0
            } else {
                state.document.inline(next).map_or(0, Inline::len)
            };
            Some(Selection::caret(next, offset))
        });
        if let Some(selection) = destination {
            self.update(|state| state.request_caret(selection));
        }
        destination.is_some()
    }

    /// Tab inside a table: next cell, appending a row past the last one.
    pub(crate) fn next_cell(&self, target: TextTarget, backwards: bool) {
        let Some(cell) = target.cell else {
            return;
        };
        let shape = self.peek(
            |state| match state.document.block(target.block).map(|b| &b.kind) {
                Some(BlockKind::Table(table)) => Some((table.rows().len(), table.columns().len())),
                _ => None,
            },
        );
        let Some((rows, columns)) = shape else {
            return;
        };
        let index = cell.row * columns + cell.column;
        let next = if backwards {
            index.checked_sub(1)
        } else {
            Some(index + 1)
        };
        let Some(next) = next else {
            return;
        };
        if next >= rows * columns {
            self.table_insert_row();
            return;
        }
        let next = CellPos {
            row: next / columns,
            column: next % columns,
        };
        self.focus(
            TextTarget {
                block: target.block,
                cell: Some(next),
            },
            0,
        );
    }

    /// Remove the trigger text in `range` and run the picked action.
    pub(crate) fn commit_suggestion(
        &self,
        target: TextTarget,
        range: Range<usize>,
        action: SuggestionAction,
    ) {
        self.update(|state| {
            state.record(None);
            if let Some(inline) = state.document.inline_mut(target) {
                inline.replace(range.clone(), "");
            }
            state.changed();
            state.request_caret(Selection::caret(target, range.start));
        });
        match action {
            SuggestionAction::InsertText(text) => self.insert_text(&text),
            SuggestionAction::InsertInline(inline) => self.insert_inline(inline),
            SuggestionAction::InsertLink { label, url } => self.insert_link(&label, &url),
            SuggestionAction::InsertEmbed { label, url } => self.insert_embed(&label, &url),
            SuggestionAction::SetBlockType(block_type) => self.set_block_type(block_type),
            SuggestionAction::InsertBlock(kind) => self.insert_block(kind),
        }
    }

    /// Ctrl+Enter: open an empty paragraph after the block of `target`.
    pub(crate) fn exit_block(&self, target: TextTarget) {
        self.update(|state| {
            state.record(None);
            let id = state
                .document
                .insert_after(target.block, Block::paragraph(""));
            state.changed();
            state.request_caret(Selection::caret(TextTarget::from(id), 0));
        });
    }

    pub(crate) fn remember_copy(&self, target: TextTarget, range: Range<usize>) {
        self.update(|state| {
            state.clipboard = state.document.inline(target).map(|inline| {
                let slice = inline.slice(range);
                (slice.plain_text(), slice)
            });
        });
    }

    /// Rich content for `plain` if it was copied from this editor.
    pub(crate) fn recall_copy(&self, plain: &str) -> Option<Inline> {
        self.peek(|state| {
            state
                .clipboard
                .as_ref()
                .filter(|(text, _)| text == plain)
                .map(|(_, inline)| inline.clone())
        })
    }
}

fn first_target_from(document: &Document, id: BlockId) -> Option<TextTarget> {
    let index = document.index_of(id)?;
    document.blocks()[index..]
        .iter()
        .find_map(|block| match &block.kind {
            BlockKind::Table(_) => Some(TextTarget {
                block: block.id,
                cell: Some(CellPos { row: 0, column: 0 }),
            }),
            kind if kind.content().is_some() => Some(TextTarget::from(block.id)),
            _ => None,
        })
}

fn clamp(document: &Document, selection: Selection) -> Selection {
    let len = document.inline(selection.target).map_or(0, Inline::len);
    Selection {
        target: selection.target,
        anchor: selection.anchor.min(len),
        head: selection.head.min(len),
    }
}
