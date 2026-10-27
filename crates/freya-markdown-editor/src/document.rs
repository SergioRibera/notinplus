//! Editable document model: an ordered list of flat blocks, Notion style.
//!
//! Nested markdown containers are flattened on parse: list items carry a
//! `depth`, quotes and callouts hold a single inline run. Every editable run
//! of text is an [`Inline`] addressed by a [`TextTarget`].

mod color;
mod inline;
mod table;

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

pub use color::{ParseColorError, Rgba};
pub use inline::{
    Atom, Inline, Link, Mark, MarkFamily, MarkKind, OBJECT_REPLACEMENT, TextChange, byte_index,
    diff, utf16_len,
};
pub use table::{ColumnAlign, Table};

/// Stable identity of a block for the lifetime of the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(u64);

impl BlockId {
    #[must_use]
    pub fn fresh() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

impl Align {
    #[must_use]
    pub const fn as_css(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
            Self::Justify => "justify",
        }
    }

    #[must_use]
    pub fn from_css(value: &str) -> Option<Self> {
        Some(match value.trim().to_ascii_lowercase().as_str() {
            "left" => Self::Left,
            "center" => Self::Center,
            "right" => Self::Right,
            "justify" => Self::Justify,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HeadingLevel {
    H1 = 1,
    H2,
    H3,
    H4,
    H5,
    H6,
}

impl HeadingLevel {
    #[must_use]
    pub const fn get(self) -> u8 {
        self as u8
    }
}

impl TryFrom<u8> for HeadingLevel {
    type Error = u8;

    fn try_from(level: u8) -> Result<Self, Self::Error> {
        Ok(match level {
            1 => Self::H1,
            2 => Self::H2,
            3 => Self::H3,
            4 => Self::H4,
            5 => Self::H5,
            6 => Self::H6,
            other => return Err(other),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CalloutKind {
    #[default]
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl CalloutKind {
    pub const ALL: [Self; 5] = [
        Self::Note,
        Self::Tip,
        Self::Important,
        Self::Warning,
        Self::Caution,
    ];

    #[must_use]
    pub const fn marker(self) -> &'static str {
        match self {
            Self::Note => "NOTE",
            Self::Tip => "TIP",
            Self::Important => "IMPORTANT",
            Self::Warning => "WARNING",
            Self::Caution => "CAUTION",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ListKind {
    Bullet,
    /// `start` is only honored on the first item of a run.
    Ordered {
        start: u32,
    },
    Task {
        checked: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BlockKind {
    Paragraph(Inline),
    Heading {
        level: HeadingLevel,
        content: Inline,
        anchor: Option<String>,
    },
    Quote(Inline),
    Callout {
        kind: CalloutKind,
        title: Option<String>,
        content: Inline,
    },
    ListItem {
        kind: ListKind,
        depth: u8,
        content: Inline,
    },
    Code {
        language: String,
        code: Inline,
    },
    Math(Inline),
    Divider,
    Table(Table),
    Footnote {
        label: String,
        content: Inline,
    },
    /// Description list term.
    Term(Inline),
    /// Description list details, attached to the closest previous term.
    Definition(Inline),
    FrontMatter(Inline),
    TableOfContents,
    Html(Inline),
    Transclusion(Transclusion),
}

/// A quote whose content lives elsewhere and is shown as it is now.
///
/// In markdown it is a block quote opened by `[!<kind>]` (a kind with a
/// colon, so it never reads as an alert), followed by the content as it
/// was last seen and closed by a `— [label](url)` line naming the source.
/// Portable markdown keeps the plain quote and drops the opening line.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Transclusion {
    /// What the opening marker names, such as `app:live`.
    pub kind: String,
    pub url: String,
    pub label: String,
    /// Markdown of the content as last seen, paragraphs split by a blank
    /// line.
    pub fallback: String,
}

/// How the text of a block may be edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextRole {
    /// Inline formatting allowed.
    Rich,
    /// Source like text (code, math, raw html): no marks, monospace.
    Plain,
}

/// Block conversions offered by toolbars and slash menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockType {
    Paragraph,
    Heading(HeadingLevel),
    Quote,
    Callout(CalloutKind),
    BulletList,
    OrderedList,
    TaskList,
    Code,
    Math,
    Term,
    Definition,
}

impl BlockKind {
    #[must_use]
    pub const fn content(&self) -> Option<&Inline> {
        match self {
            Self::Paragraph(content)
            | Self::Quote(content)
            | Self::Math(content)
            | Self::Term(content)
            | Self::Definition(content)
            | Self::FrontMatter(content)
            | Self::Html(content)
            | Self::Heading { content, .. }
            | Self::Callout { content, .. }
            | Self::ListItem { content, .. }
            | Self::Footnote { content, .. }
            | Self::Code { code: content, .. } => Some(content),
            Self::Divider | Self::Table(_) | Self::TableOfContents | Self::Transclusion(_) => None,
        }
    }

    pub const fn content_mut(&mut self) -> Option<&mut Inline> {
        match self {
            Self::Paragraph(content)
            | Self::Quote(content)
            | Self::Math(content)
            | Self::Term(content)
            | Self::Definition(content)
            | Self::FrontMatter(content)
            | Self::Html(content)
            | Self::Heading { content, .. }
            | Self::Callout { content, .. }
            | Self::ListItem { content, .. }
            | Self::Footnote { content, .. }
            | Self::Code { code: content, .. } => Some(content),
            Self::Divider | Self::Table(_) | Self::TableOfContents | Self::Transclusion(_) => None,
        }
    }

    #[must_use]
    pub const fn role(&self) -> Option<TextRole> {
        match self {
            Self::Code { .. } | Self::Math(_) | Self::FrontMatter(_) | Self::Html(_) => {
                Some(TextRole::Plain)
            }
            Self::Divider | Self::TableOfContents | Self::Transclusion(_) => None,
            _ => Some(TextRole::Rich),
        }
    }

    #[must_use]
    pub const fn block_type(&self) -> Option<BlockType> {
        Some(match self {
            Self::Paragraph(_) => BlockType::Paragraph,
            Self::Heading { level, .. } => BlockType::Heading(*level),
            Self::Quote(_) => BlockType::Quote,
            Self::Callout { kind, .. } => BlockType::Callout(*kind),
            Self::ListItem { kind, .. } => match kind {
                ListKind::Bullet => BlockType::BulletList,
                ListKind::Ordered { .. } => BlockType::OrderedList,
                ListKind::Task { .. } => BlockType::TaskList,
            },
            Self::Code { .. } => BlockType::Code,
            Self::Math(_) => BlockType::Math,
            Self::Term(_) => BlockType::Term,
            Self::Definition(_) => BlockType::Definition,
            _ => return None,
        })
    }

    fn from_type(block_type: BlockType, content: Inline, depth: u8) -> Self {
        match block_type {
            BlockType::Paragraph => Self::Paragraph(content),
            BlockType::Heading(level) => Self::Heading {
                level,
                content,
                anchor: None,
            },
            BlockType::Quote => Self::Quote(content),
            BlockType::Callout(kind) => Self::Callout {
                kind,
                title: None,
                content,
            },
            BlockType::BulletList => Self::ListItem {
                kind: ListKind::Bullet,
                depth,
                content,
            },
            BlockType::OrderedList => Self::ListItem {
                kind: ListKind::Ordered { start: 1 },
                depth,
                content,
            },
            BlockType::TaskList => Self::ListItem {
                kind: ListKind::Task { checked: false },
                depth,
                content,
            },
            BlockType::Code => Self::Code {
                language: String::new(),
                code: Inline::from(content.plain_text()),
            },
            BlockType::Math => Self::Math(Inline::from(content.plain_text())),
            BlockType::Term => Self::Term(content),
            BlockType::Definition => Self::Definition(content),
        }
    }

    /// Kind of the block created when pressing Enter at the end of `self`.
    fn continuation(&self) -> Self {
        let empty = Inline::default();
        match self {
            Self::ListItem { kind, depth, .. } => Self::ListItem {
                kind: match kind {
                    ListKind::Task { .. } => ListKind::Task { checked: false },
                    other => *other,
                },
                depth: *depth,
                content: empty,
            },
            Self::Term(_) | Self::Definition(_) => Self::Definition(empty),
            _ => Self::Paragraph(empty),
        }
    }

    const fn is_paragraph(&self) -> bool {
        matches!(self, Self::Paragraph(_))
    }
}

/// A heading as listed by [`Document::outline`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadingRef {
    pub block: BlockId,
    pub level: HeadingLevel,
    /// Plain text of the heading.
    pub text: String,
    /// Unique anchor inside the document.
    pub slug: String,
    /// Whether the anchor is fixed (`{#anchor}`) rather than derived from
    /// the text.
    pub pinned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Block {
    pub id: BlockId,
    pub kind: BlockKind,
    pub align: Align,
}

impl From<BlockKind> for Block {
    fn from(kind: BlockKind) -> Self {
        Self {
            id: BlockId::fresh(),
            kind,
            align: Align::Left,
        }
    }
}

impl Block {
    #[must_use]
    pub fn paragraph(content: impl Into<Inline>) -> Self {
        BlockKind::Paragraph(content.into()).into()
    }

    #[must_use]
    pub const fn with_align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellPos {
    pub row: usize,
    pub column: usize,
}

/// Address of an editable inline run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextTarget {
    pub block: BlockId,
    pub cell: Option<CellPos>,
}

impl From<BlockId> for TextTarget {
    fn from(block: BlockId) -> Self {
        Self { block, cell: None }
    }
}

/// Where the caret should land after a structural edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caret {
    pub target: TextTarget,
    /// UTF-16 offset inside the target.
    pub offset: usize,
}

impl Caret {
    #[must_use]
    pub fn new(target: impl Into<TextTarget>, offset: usize) -> Self {
        Self {
            target: target.into(),
            offset,
        }
    }
}

/// `*[term]: expansion` definition (PHP Markdown Extra abbreviations).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Abbreviation {
    pub term: String,
    pub expansion: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Document {
    blocks: Vec<Block>,
    pub abbreviations: Vec<Abbreviation>,
}

impl Default for Document {
    fn default() -> Self {
        Self::from(vec![Block::paragraph("")])
    }
}

impl From<Vec<Block>> for Document {
    fn from(blocks: Vec<Block>) -> Self {
        let mut document = Self {
            blocks,
            abbreviations: Vec::new(),
        };
        document.ensure_editable();
        document
    }
}

impl Document {
    #[must_use]
    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    #[must_use]
    pub fn index_of(&self, id: BlockId) -> Option<usize> {
        self.blocks.iter().position(|block| block.id == id)
    }

    #[must_use]
    pub fn block(&self, id: BlockId) -> Option<&Block> {
        self.blocks.iter().find(|block| block.id == id)
    }

    pub fn block_mut(&mut self, id: BlockId) -> Option<&mut Block> {
        self.blocks.iter_mut().find(|block| block.id == id)
    }

    #[must_use]
    pub fn inline(&self, target: TextTarget) -> Option<&Inline> {
        let block = self.block(target.block)?;
        match (&block.kind, target.cell) {
            (BlockKind::Table(table), Some(cell)) => table.cell(cell.row, cell.column),
            (kind, None) => kind.content(),
            _ => None,
        }
    }

    pub fn inline_mut(&mut self, target: TextTarget) -> Option<&mut Inline> {
        let block = self.block_mut(target.block)?;
        match (&mut block.kind, target.cell) {
            (BlockKind::Table(table), Some(cell)) => table.cell_mut(cell.row, cell.column),
            (kind, None) => kind.content_mut(),
            _ => None,
        }
    }

    /// Text role of `target`, `None` when it does not exist.
    #[must_use]
    pub fn role(&self, target: TextTarget) -> Option<TextRole> {
        let block = self.block(target.block)?;
        match (&block.kind, target.cell) {
            (BlockKind::Table(_), Some(_)) => Some(TextRole::Rich),
            (kind, None) => kind.role(),
            _ => None,
        }
    }

    /// Keep a paragraph to type into after the last block: whenever the
    /// document ends in anything but a paragraph that already has content
    /// (or cannot hold text, like a table), an empty paragraph follows it.
    /// That is the way out of code blocks, quotes, tables and the like.
    pub fn ensure_editable(&mut self) {
        let needs_tail = self.blocks.last().is_none_or(|block| match &block.kind {
            BlockKind::Paragraph(_) => false,
            kind => kind.content().is_none_or(|content| !content.is_empty()),
        });
        if needs_tail {
            self.blocks.push(Block::paragraph(""));
        }
    }

    /// Every editable target in reading order (table cells row by row).
    #[must_use]
    pub fn targets(&self) -> Vec<TextTarget> {
        self.blocks
            .iter()
            .flat_map(|block| match &block.kind {
                BlockKind::Table(table) => table
                    .rows()
                    .iter()
                    .enumerate()
                    .flat_map(|(row, cells)| {
                        (0..cells.len()).map(move |column| TextTarget {
                            block: block.id,
                            cell: Some(CellPos { row, column }),
                        })
                    })
                    .collect::<Vec<_>>(),
                kind if kind.content().is_some() => vec![TextTarget::from(block.id)],
                _ => Vec::new(),
            })
            .collect()
    }

    #[must_use]
    pub fn previous_target(&self, target: TextTarget) -> Option<TextTarget> {
        let targets = self.targets();
        let index = targets.iter().position(|t| *t == target)?;
        index.checked_sub(1).map(|i| targets[i])
    }

    #[must_use]
    pub fn next_target(&self, target: TextTarget) -> Option<TextTarget> {
        let targets = self.targets();
        let index = targets.iter().position(|t| *t == target)?;
        targets.get(index + 1).copied()
    }

    pub fn insert(&mut self, index: usize, block: Block) -> BlockId {
        let id = block.id;
        self.blocks.insert(index.min(self.blocks.len()), block);
        id
    }

    pub fn insert_after(&mut self, anchor: BlockId, block: Block) -> BlockId {
        let index = self.index_of(anchor).map_or(self.blocks.len(), |i| i + 1);
        self.insert(index, block)
    }

    pub fn remove(&mut self, id: BlockId) -> Option<Block> {
        let index = self.index_of(id)?;
        let block = self.blocks.remove(index);
        self.ensure_editable();
        Some(block)
    }

    /// Move a block by `delta` positions (negative moves up).
    pub fn move_block(&mut self, id: BlockId, delta: isize) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        let Some(destination) = index.checked_add_signed(delta) else {
            return false;
        };
        destination < self.blocks.len() && self.move_block_to(id, destination)
    }

    /// Move a block so it ends at `index` (clamped to the document).
    pub fn move_block_to(&mut self, id: BlockId, index: usize) -> bool {
        let Some(from) = self.index_of(id) else {
            return false;
        };
        let to = index.min(self.blocks.len() - 1);
        if from == to {
            return false;
        }
        let block = self.blocks.remove(from);
        self.blocks.insert(to, block);
        self.normalize_depths();
        self.ensure_editable();
        true
    }

    /// Move `id` to the position currently held by `target`, the usual
    /// drop semantics of a drag handle.
    pub fn move_block_onto(&mut self, id: BlockId, target: BlockId) -> bool {
        self.index_of(target)
            .is_some_and(|index| self.move_block_to(id, index))
    }

    /// Clamp list depths so every item is at most one level deeper than
    /// the item before it, keeping the markdown valid after moves.
    fn normalize_depths(&mut self) {
        let mut previous: Option<u8> = None;
        for block in &mut self.blocks {
            match &mut block.kind {
                BlockKind::ListItem { depth, .. } => {
                    let limit = previous.map_or(0, |depth| depth.saturating_add(1));
                    *depth = (*depth).min(limit);
                    previous = Some(*depth);
                }
                _ => previous = None,
            }
        }
    }

    /// Convert a block, keeping its text. Unsupported kinds are left as is.
    pub fn set_type(&mut self, id: BlockId, block_type: BlockType) -> bool {
        let Some(block) = self.block_mut(id) else {
            return false;
        };
        if block.kind.block_type() == Some(block_type) {
            return false;
        }
        let Some(content) = block.kind.content().cloned() else {
            return false;
        };
        let depth = match block.kind {
            BlockKind::ListItem { depth, .. } => depth,
            _ => 0,
        };
        let content = match block.kind.role() {
            Some(TextRole::Plain) => Inline::from(content.plain_text()),
            _ => content,
        };
        block.kind = BlockKind::from_type(block_type, content, depth);
        true
    }

    /// Enter key: split `target` at `offset`.
    pub fn split(&mut self, target: TextTarget, offset: usize) -> Option<Caret> {
        if target.cell.is_some() {
            return None;
        }
        let index = self.index_of(target.block)?;
        let block = &mut self.blocks[index];
        let content = block.kind.content()?;
        let is_empty = content.is_empty();

        if is_empty {
            match &mut block.kind {
                BlockKind::ListItem { depth, .. } if *depth > 0 => {
                    *depth -= 1;
                    return Some(Caret::new(target.block, 0));
                }
                BlockKind::Paragraph(_) => {}
                kind if kind.role() == Some(TextRole::Rich) => {
                    *kind = BlockKind::Paragraph(Inline::default());
                    return Some(Caret::new(target.block, 0));
                }
                _ => {}
            }
        }

        if offset == 0 && !is_empty {
            let mut fresh = Block::from(block.kind.continuation());
            if !matches!(block.kind, BlockKind::ListItem { .. }) {
                fresh.kind = BlockKind::Paragraph(Inline::default());
            }
            self.blocks.insert(index, fresh);
            return Some(Caret::new(target.block, 0));
        }

        let mut next = Block::from(block.kind.continuation());
        let align = block.align;
        let tail = block.kind.content_mut()?.split_off(offset);
        if let Some(content) = next.kind.content_mut() {
            *content = tail;
        }
        if next.kind.is_paragraph() {
            next.align = align;
        }
        let id = next.id;
        self.blocks.insert(index + 1, next);
        Some(Caret::new(id, 0))
    }

    /// Backspace at offset 0 of `target`.
    pub fn join_backward(&mut self, target: TextTarget) -> Option<Caret> {
        if target.cell.is_some() {
            return None;
        }
        let index = self.index_of(target.block)?;
        let block = &mut self.blocks[index];
        match &mut block.kind {
            BlockKind::Paragraph(_) => {}
            BlockKind::ListItem { depth, .. } if *depth > 0 => {
                *depth -= 1;
                return Some(Caret::new(target.block, 0));
            }
            kind if kind.role() == Some(TextRole::Rich) => {
                let content = kind.content().cloned().unwrap_or_default();
                *kind = BlockKind::Paragraph(content);
                return Some(Caret::new(target.block, 0));
            }
            kind => {
                if kind.content().is_some_and(Inline::is_empty) {
                    *kind = BlockKind::Paragraph(Inline::default());
                    return Some(Caret::new(target.block, 0));
                }
                return None;
            }
        }

        let previous_index = index.checked_sub(1)?;
        let previous_id = self.blocks[previous_index].id;
        match &self.blocks[previous_index].kind {
            BlockKind::Divider | BlockKind::TableOfContents | BlockKind::Transclusion(_) => {
                self.blocks.remove(previous_index);
                Some(Caret::new(target.block, 0))
            }
            BlockKind::Table(table) => {
                let row = table.rows().len() - 1;
                let column = table.columns().len() - 1;
                let offset = table.cell(row, column).map_or(0, Inline::len);
                if self.blocks[index]
                    .kind
                    .content()
                    .is_some_and(Inline::is_empty)
                {
                    self.blocks.remove(index);
                    self.ensure_editable();
                }
                Some(Caret {
                    target: TextTarget {
                        block: previous_id,
                        cell: Some(CellPos { row, column }),
                    },
                    offset,
                })
            }
            _ => {
                let current = self.blocks.remove(index);
                let content = current.kind.content().cloned().unwrap_or_default();
                let previous = &mut self.blocks[previous_index].kind;
                let plain = previous.role() == Some(TextRole::Plain);
                let destination = previous.content_mut()?;
                let offset = destination.len();
                if plain {
                    let end = destination.len();
                    destination.replace(end..end, &content.plain_text());
                } else {
                    destination.append(content);
                }
                Some(Caret::new(previous_id, offset))
            }
        }
    }

    /// Delete key at the end of `target`.
    pub fn join_forward(&mut self, target: TextTarget) -> Option<Caret> {
        if target.cell.is_some() {
            return None;
        }
        let index = self.index_of(target.block)?;
        let next_index = index + 1;
        let next = self.blocks.get(next_index)?;
        let offset = self.blocks[index].kind.content()?.len();
        match &next.kind {
            BlockKind::Divider | BlockKind::TableOfContents | BlockKind::Transclusion(_) => {
                self.blocks.remove(next_index);
            }
            BlockKind::Table(_) => return None,
            kind => {
                let content = kind.content().cloned().unwrap_or_default();
                self.blocks.remove(next_index);
                let current = &mut self.blocks[index].kind;
                let plain = current.role() == Some(TextRole::Plain);
                let destination = current.content_mut()?;
                if plain {
                    let end = destination.len();
                    destination.replace(end..end, &content.plain_text());
                } else {
                    destination.append(content);
                }
            }
        }
        self.ensure_editable();
        Some(Caret::new(target.block, offset))
    }

    /// Change the depth of a list item by `delta`, bounded by its previous
    /// sibling so the markdown stays valid.
    pub fn indent(&mut self, id: BlockId, delta: i8) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        let max_depth = match index
            .checked_sub(1)
            .map(|previous| &self.blocks[previous].kind)
        {
            Some(BlockKind::ListItem { depth, .. }) => depth.saturating_add(1),
            _ => 0,
        };
        let BlockKind::ListItem { depth, .. } = &mut self.blocks[index].kind else {
            return false;
        };
        let wanted = depth.saturating_add_signed(delta).min(max_depth);
        let changed = wanted != *depth;
        *depth = wanted;
        changed
    }

    pub fn toggle_task(&mut self, id: BlockId) -> bool {
        let Some(BlockKind::ListItem {
            kind: ListKind::Task { checked },
            ..
        }) = self.block_mut(id).map(|block| &mut block.kind)
        else {
            return false;
        };
        *checked = !*checked;
        true
    }

    /// Turn a paragraph starting with a markdown block marker (`# `, `- `,
    /// `1. `, `> `, `[ ] `, `` ``` ``, `---`, `$$`) into that block. Only fires
    /// when `cursor` sits right after the marker, i.e. it was just typed.
    pub fn apply_block_shortcut(&mut self, id: BlockId, cursor: usize) -> Option<Caret> {
        let index = self.index_of(id)?;
        let block = &mut self.blocks[index];
        let BlockKind::Paragraph(content) = &block.kind else {
            if let BlockKind::ListItem {
                kind: kind @ ListKind::Bullet,
                content,
                ..
            } = &mut block.kind
            {
                let marker_len = ["[ ] ", "[] ", "[x] "]
                    .iter()
                    .find(|marker| content.text().starts_with(**marker))
                    .map(|marker| marker.len())?;
                if marker_len != cursor {
                    return None;
                }
                let checked = content.text().starts_with("[x]");
                content.replace(0..marker_len, "");
                *kind = ListKind::Task { checked };
                return Some(Caret::new(id, 0));
            }
            return None;
        };
        let text = content.text().to_owned();

        let whole = match text.as_str() {
            _ if cursor != utf16_len(&text) => None,
            "---" | "***" | "___" => Some(BlockKind::Divider),
            "```" => Some(BlockKind::Code {
                language: String::new(),
                code: Inline::default(),
            }),
            "$$" => Some(BlockKind::Math(Inline::default())),
            _ => None,
        };
        if let Some(kind) = whole {
            if kind == BlockKind::Divider {
                block.kind = kind;
                let next = self.insert(index + 1, Block::paragraph(""));
                return Some(Caret::new(next, 0));
            }
            block.kind = kind;
            return Some(Caret::new(id, 0));
        }

        let (marker_len, kind) = shortcut_marker(&text)?;
        if marker_len != cursor {
            return None;
        }
        let mut content = content.clone();
        content.replace(0..marker_len, "");
        block.kind = match kind {
            Shortcut::Heading(level) => BlockKind::Heading {
                level,
                content,
                anchor: None,
            },
            Shortcut::List(kind) => BlockKind::ListItem {
                kind,
                depth: 0,
                content,
            },
            Shortcut::Quote => BlockKind::Quote(content),
        };
        Some(Caret::new(id, 0))
    }

    /// Text of every block without formatting, one block per line.
    /// Table rows join their cells with ` | `.
    #[must_use]
    pub fn plain_text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|block| match &block.kind {
                BlockKind::Table(table) => Some(
                    table
                        .rows()
                        .iter()
                        .map(|row| {
                            row.iter()
                                .map(Inline::plain_text)
                                .collect::<Vec<_>>()
                                .join(" | ")
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                kind => kind.content().map(Inline::plain_text),
            })
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Headings in document order, used to build tables of contents.
    pub fn headings(&self) -> impl Iterator<Item = (BlockId, HeadingLevel, &Inline)> {
        self.blocks.iter().filter_map(|block| match &block.kind {
            BlockKind::Heading { level, content, .. } => Some((block.id, *level, content)),
            _ => None,
        })
    }

    /// Headings with a unique anchor each: the explicit `{#id}` when the
    /// heading has one, otherwise a GitHub style slug of its text with a
    /// `-1`, `-2`... suffix when it is taken. Explicit anchors are
    /// reserved first, so a heading added later never steals one. The
    /// anchors are what `[text](#anchor)` links point to.
    #[must_use]
    pub fn outline(&self) -> Vec<HeadingRef> {
        let headings: Vec<(BlockId, HeadingLevel, String, Option<&String>)> = self
            .blocks
            .iter()
            .filter_map(|block| match &block.kind {
                BlockKind::Heading {
                    level,
                    content,
                    anchor,
                } => Some((block.id, *level, content.plain_text(), anchor.as_ref())),
                _ => None,
            })
            .collect();

        let mut taken: HashSet<String> = HashSet::new();
        let mut slugs: Vec<Option<String>> = headings
            .iter()
            .map(|(_, _, _, anchor)| {
                anchor
                    .filter(|anchor| taken.insert((*anchor).clone()))
                    .cloned()
            })
            .collect();
        for (slug, (_, _, text, anchor)) in slugs.iter_mut().zip(&headings) {
            if slug.is_some() {
                continue;
            }
            let base = anchor.map_or_else(|| crate::markdown::slug(text), Clone::clone);
            let mut unique = base.clone();
            let mut suffix = 0;
            while taken.contains(&unique) {
                suffix += 1;
                unique = format!("{base}-{suffix}");
            }
            taken.insert(unique.clone());
            *slug = Some(unique);
        }

        headings
            .into_iter()
            .zip(slugs)
            .map(|((block, level, text, anchor), slug)| HeadingRef {
                block,
                level,
                text,
                slug: slug.unwrap_or_default(),
                pinned: anchor.is_some(),
            })
            .collect()
    }

    /// Give heading `id` a fixed anchor (`{#anchor}` in markdown), or go
    /// back to one derived from its text with `None`. A fixed anchor
    /// keeps links to the heading working when its text changes.
    /// Returns whether `id` is a heading.
    pub fn set_heading_anchor(&mut self, id: BlockId, new_anchor: Option<String>) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        match &mut self.blocks[index].kind {
            BlockKind::Heading { anchor, .. } => {
                *anchor = new_anchor;
                true
            }
            _ => false,
        }
    }

    /// Block of the heading whose [`Self::outline`] anchor is `slug`.
    #[must_use]
    pub fn heading_by_slug(&self, slug: &str) -> Option<BlockId> {
        self.outline()
            .into_iter()
            .find(|heading| heading.slug == slug)
            .map(|heading| heading.block)
    }

    /// Number shown in front of an ordered list item: the run start plus
    /// the number of same depth items before it.
    #[must_use]
    pub fn ordinal(&self, id: BlockId) -> Option<u32> {
        let index = self.index_of(id)?;
        let BlockKind::ListItem {
            kind: ListKind::Ordered { .. },
            depth,
            ..
        } = self.blocks[index].kind
        else {
            return None;
        };
        let mut first_start = 1;
        let mut count = 0u32;
        for block in self.blocks[..=index].iter().rev() {
            match block.kind {
                BlockKind::ListItem {
                    kind: ListKind::Ordered { start },
                    depth: d,
                    ..
                } if d == depth => {
                    first_start = start;
                    count += 1;
                }
                BlockKind::ListItem { depth: d, .. } if d > depth => {}
                _ => break,
            }
        }
        Some(first_start.saturating_add(count - 1))
    }
}

enum Shortcut {
    Heading(HeadingLevel),
    List(ListKind),
    Quote,
}

fn shortcut_marker(text: &str) -> Option<(usize, Shortcut)> {
    let hashes = text.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&hashes) && text[hashes..].starts_with(' ') {
        let level = HeadingLevel::try_from(u8::try_from(hashes).ok()?).ok()?;
        return Some((hashes + 1, Shortcut::Heading(level)));
    }
    for marker in ["- [ ] ", "- [x] ", "[ ] ", "[] ", "[x] "] {
        if text.starts_with(marker) {
            let checked = marker.contains('x');
            return Some((marker.len(), Shortcut::List(ListKind::Task { checked })));
        }
    }
    for marker in ["- ", "* ", "+ "] {
        if text.starts_with(marker) {
            return Some((2, Shortcut::List(ListKind::Bullet)));
        }
    }
    if text.starts_with("> ") {
        return Some((2, Shortcut::Quote));
    }
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && (text[digits..].starts_with(". ") || text[digits..].starts_with(") ")) {
        let start = text[..digits].parse().ok()?;
        return Some((digits + 2, Shortcut::List(ListKind::Ordered { start })));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(blocks: Vec<BlockKind>) -> Document {
        Document::from(blocks.into_iter().map(Block::from).collect::<Vec<_>>())
    }

    fn text_of(document: &Document, index: usize) -> &str {
        document.blocks()[index]
            .kind
            .content()
            .map_or("", Inline::text)
    }

    #[test]
    fn split_paragraph_moves_tail() {
        let mut document = doc(vec![BlockKind::Paragraph("hello world".into())]);
        let id = document.blocks()[0].id;
        let caret = document.split(id.into(), 5).expect("split");
        assert_eq!(text_of(&document, 0), "hello");
        assert_eq!(text_of(&document, 1), " world");
        assert_eq!(caret.offset, 0);
        assert_eq!(caret.target.block, document.blocks()[1].id);
    }

    #[test]
    fn enter_on_empty_list_item_exits_list() {
        let mut document = doc(vec![BlockKind::ListItem {
            kind: ListKind::Bullet,
            depth: 0,
            content: Inline::default(),
        }]);
        let id = document.blocks()[0].id;
        document.split(id.into(), 0);
        assert!(matches!(document.blocks()[0].kind, BlockKind::Paragraph(_)));
    }

    #[test]
    fn backspace_merges_paragraphs() {
        let mut document = doc(vec![
            BlockKind::Paragraph("ab".into()),
            BlockKind::Paragraph("cd".into()),
        ]);
        let second = document.blocks()[1].id;
        let caret = document.join_backward(second.into()).expect("join");
        assert_eq!(document.blocks().len(), 1);
        assert_eq!(text_of(&document, 0), "abcd");
        assert_eq!(caret.offset, 2);
    }

    #[test]
    fn backspace_on_heading_turns_into_paragraph() {
        let mut document = doc(vec![BlockKind::Heading {
            level: HeadingLevel::H2,
            content: "title".into(),
            anchor: None,
        }]);
        let id = document.blocks()[0].id;
        document.join_backward(id.into());
        assert!(matches!(document.blocks()[0].kind, BlockKind::Paragraph(_)));
    }

    #[test]
    fn shortcuts_convert_paragraphs() {
        let mut document = doc(vec![BlockKind::Paragraph("## title".into())]);
        let id = document.blocks()[0].id;
        assert!(document.apply_block_shortcut(id, 5).is_none());
        document.apply_block_shortcut(id, 3);
        assert!(matches!(
            document.blocks()[0].kind,
            BlockKind::Heading {
                level: HeadingLevel::H2,
                ..
            }
        ));

        let mut document = doc(vec![BlockKind::Paragraph("3. ".into())]);
        let id = document.blocks()[0].id;
        document.apply_block_shortcut(id, 3);
        assert!(matches!(
            document.blocks()[0].kind,
            BlockKind::ListItem {
                kind: ListKind::Ordered { start: 3 },
                ..
            }
        ));
    }

    #[test]
    fn divider_shortcut_adds_paragraph_after() {
        let mut document = doc(vec![BlockKind::Paragraph("---".into())]);
        let id = document.blocks()[0].id;
        let caret = document.apply_block_shortcut(id, 3).expect("divider");
        assert_eq!(document.blocks()[0].kind, BlockKind::Divider);
        assert_eq!(caret.target.block, document.blocks()[1].id);
    }

    #[test]
    fn indent_is_bounded_by_previous_item() {
        let item = |depth| BlockKind::ListItem {
            kind: ListKind::Bullet,
            depth,
            content: "x".into(),
        };
        let mut document = doc(vec![item(0), item(0)]);
        let second = document.blocks()[1].id;
        assert!(document.indent(second, 1));
        assert!(!document.indent(second, 1));
    }

    #[test]
    fn ordinals_count_runs() {
        let item = |start, depth| BlockKind::ListItem {
            kind: ListKind::Ordered { start },
            depth,
            content: "x".into(),
        };
        let document = doc(vec![item(4, 0), item(1, 1), item(1, 0)]);
        let ids: Vec<_> = document.blocks().iter().map(|b| b.id).collect();
        assert_eq!(document.ordinal(ids[0]), Some(4));
        assert_eq!(document.ordinal(ids[1]), Some(1));
        assert_eq!(document.ordinal(ids[2]), Some(5));
    }

    #[test]
    fn code_with_text_at_the_end_gets_a_paragraph_after() {
        let empty_code = doc(vec![BlockKind::Code {
            language: String::new(),
            code: Inline::default(),
        }]);
        assert_eq!(empty_code.blocks().len(), 1);

        let code = doc(vec![BlockKind::Code {
            language: String::new(),
            code: "fn main() {}".into(),
        }]);
        assert_eq!(code.blocks().len(), 2);
        assert!(matches!(&code.blocks()[1].kind, BlockKind::Paragraph(text) if text.is_empty()));
    }

    #[test]
    fn outline_slugs_are_unique() {
        let document = Document::parse("# Intro\n\n## Notas\n\n## Notas\n\n## Otra {#custom}");
        let slugs: Vec<String> = document
            .outline()
            .into_iter()
            .map(|heading| heading.slug)
            .collect();
        assert_eq!(slugs, ["intro", "notas", "notas-1", "custom"]);
        assert_eq!(
            document.heading_by_slug("notas-1"),
            Some(document.outline()[2].block)
        );
    }

    #[test]
    fn pinned_anchor_survives_renames_and_wins_collisions() {
        let mut document = Document::parse("## Notas\n\n## Otra");
        let first = document.outline()[0].block;
        assert!(document.set_heading_anchor(first, Some("notas".into())));

        let renamed = Document::parse(
            &document
                .to_markdown(crate::markdown::Dialect::Rich)
                .replace("## Notas", "## Apuntes"),
        );
        assert_eq!(renamed.outline()[0].slug, "notas");
        assert!(renamed.outline()[0].pinned);

        let collision = Document::parse("## Notas\n\n## Apuntes {#notas}");
        let slugs: Vec<String> = collision.outline().into_iter().map(|h| h.slug).collect();
        assert_eq!(slugs, ["notas-1", "notas"]);
    }

    #[test]
    fn moving_blocks_keeps_list_depths_valid() {
        let item = |depth, text: &str| BlockKind::ListItem {
            kind: ListKind::Bullet,
            depth,
            content: text.into(),
        };
        let mut document = doc(vec![
            item(0, "a"),
            item(1, "b"),
            BlockKind::Paragraph("p".into()),
        ]);
        let ids: Vec<_> = document.blocks().iter().map(|b| b.id).collect();
        assert!(document.move_block_onto(ids[2], ids[0]));
        let order: Vec<_> = document.blocks().iter().map(|b| b.id).collect();
        assert_eq!(&order[..3], &[ids[2], ids[0], ids[1]]);
        assert!(document.move_block(ids[1], -2));
        assert!(matches!(
            document.blocks()[0].kind,
            BlockKind::ListItem { depth: 0, .. }
        ));
    }
}
