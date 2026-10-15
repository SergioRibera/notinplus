//! Per-instance viewer state.
//!
//! One `BibleViewerState` lives per open Bible tab. The canvas trait
//! methods take a `u64` instance handle; the plugin resolves it to a
//! `BibleViewerState` through a `RwLock<HashMap<u64, _>>` guarded by
//! the plugin struct.
//!
//! State is intentionally small: the heavy data (verses, headers) lives
//! in SQLite, loaded on demand per chapter. We only cache the currently
//! visible chapter plus a small ring of recent ones so navigation feels
//! instant without pinning the whole Bible in memory.

use std::collections::VecDeque;

use crate::db::ChapterRow;

/// Everything a single open Bible tab needs to render + navigate.
#[derive(Debug)]
pub struct BibleViewerState {
    pub bible_id: String,
    pub current_book: String,
    pub current_chapter: u16,
    /// Scroll offset in logical pixels within the current chapter.
    /// Reset on chapter change — intra-chapter scroll is not persisted.
    pub scroll_y: f32,
    /// Last up-to-N chapters, newest at the back. Keeps swipe-prev
    /// warm. Capacity enforced in [`Self::push_chapter`].
    pub chapter_cache: VecDeque<ChapterRow>,
    /// Current in-progress or finalised selection, if any.
    pub selection: Option<VerseSelection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerseSelection {
    pub book_id: String,
    pub chapter: u16,
    pub start_verse: u16,
    pub end_verse: u16,
    /// `None` = whole verse. Set only when the user drag-selects inside
    /// a verse rather than tapping the whole thing.
    pub start_char: Option<u32>,
    pub end_char: Option<u32>,
}

impl BibleViewerState {
    const CACHE_CAP: usize = 4;

    #[must_use]
    pub fn new(bible_id: impl Into<String>, book_id: impl Into<String>, chapter: u16) -> Self {
        Self {
            bible_id: bible_id.into(),
            current_book: book_id.into(),
            current_chapter: chapter,
            scroll_y: 0.0,
            chapter_cache: VecDeque::with_capacity(Self::CACHE_CAP),
            selection: None,
        }
    }

    /// Insert a freshly-loaded chapter at the end of the cache,
    /// evicting the oldest entry when capacity is exceeded. If an entry
    /// for the same (book, chapter) already exists it is replaced in
    /// place rather than duplicated.
    pub fn push_chapter(&mut self, chapter: ChapterRow) {
        if let Some(pos) = self.chapter_cache.iter().position(|c| {
            c.book_id == chapter.book_id && c.chapter == chapter.chapter
        }) {
            self.chapter_cache[pos] = chapter;
            return;
        }
        if self.chapter_cache.len() == Self::CACHE_CAP {
            self.chapter_cache.pop_front();
        }
        self.chapter_cache.push_back(chapter);
    }

    #[must_use]
    pub fn current_chapter_data(&self) -> Option<&ChapterRow> {
        self.chapter_cache.iter().find(|c| {
            c.book_id == self.current_book && c.chapter == self.current_chapter
        })
    }
}
