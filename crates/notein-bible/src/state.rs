//! Per-instance viewer state.
//!
//! One `BibleViewerState` lives per open Bible tab. The canvas trait
//! methods take a `u64` instance handle; the plugin resolves it to a
//! `BibleViewerState` through a `RwLock<HashMap<u64, _>>` guarded by
//! the plugin struct.
//!
//! State is intentionally small: the heavy data (verses, headers) lives
//! in `SQLite`, loaded on demand per chapter. We only cache the currently
//! visible chapter plus a small ring of recent ones so navigation feels
//! instant without pinning the whole Bible in memory.

use std::collections::VecDeque;

use notein_sdk::{BookmarkId, Rgba};

use crate::db::{BookRow, ChapterRow, Db, DbError};

/// Back-navigation entries kept; older ones are dropped.
const MAX_HISTORY: usize = 64;

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
    /// User-created verse highlights. Each entry is a tinted range the
    /// reader renders as a background colour wash on the matching
    /// paragraph spans.
    pub highlights: Vec<BibleBookmark>,
    /// In-memory back stack of visited positions. NOT persisted — users
    /// expect browser-style "back" to clear across sessions. Pushed on
    /// every explicit navigation (`jump_to` / `switch_bible` / cross-ref
    /// click) so the toolbar's back arrow can rewind the trail.
    pub nav_history: Vec<NavEntry>,
    /// Forward stack — populated when the user hits "back"; cleared on
    /// any explicit forward navigation that is not `pop_nav`.
    pub nav_future: Vec<NavEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavEntry {
    pub bible_id: String,
    pub book_id: String,
    pub chapter: u16,
}

/// Internal highlight / bookmark representation. The host's `Bookmark`
/// model is pixel-coord based; this one is verse-coord based so it
/// survives chapter reflow + font-size changes.
#[derive(Clone, Debug, PartialEq, Eq, bincode::Encode, bincode::Decode)]
pub struct BibleBookmark {
    pub id: BookmarkId,
    pub book_id: String,
    pub chapter: u16,
    pub start_verse: u16,
    pub end_verse: u16,
    /// `0xRRGGBBAA` packed colour. `None` = default yellow wash.
    pub color: Option<u32>,
    /// Optional note body. Empty when the highlight is colour-only.
    pub note: String,
}

/// Snapshot payload the shell persists via `CanvasKind::serialize_snapshot`.
///
/// Carries the full viewer state (highlights + last position) so a
/// subsequent `restore` lands the user exactly where they left off.
///
/// Keep the layout additive: `#[bincode(skip)]` on new fields OR add
/// `Option<T>` variants so older snapshots keep decoding.
#[derive(Clone, Debug, PartialEq, Eq, bincode::Encode, bincode::Decode)]
pub struct BibleSnapshot {
    pub version: u16,
    pub bible_id: String,
    pub current_book: String,
    pub current_chapter: u16,
    pub highlights: Vec<BibleBookmark>,
}

impl Default for BibleSnapshot {
    fn default() -> Self {
        Self {
            version: Self::VERSION,
            bible_id: String::new(),
            current_book: String::new(),
            current_chapter: 1,
            highlights: Vec::new(),
        }
    }
}

impl BibleSnapshot {
    pub const VERSION: u16 = 1;

    #[must_use]
    pub fn from_state(st: &BibleViewerState) -> Self {
        Self {
            version: Self::VERSION,
            bible_id: st.bible_id.clone(),
            current_book: st.current_book.clone(),
            current_chapter: st.current_chapter,
            highlights: st.highlights.clone(),
        }
    }

    /// Apply the snapshot onto `st`. Does not touch the chapter cache —
    /// the caller runs `ensure_current_loaded(db)` afterwards.
    pub fn apply_to(self, st: &mut BibleViewerState) {
        st.bible_id = self.bible_id;
        st.current_book = self.current_book;
        st.current_chapter = self.current_chapter;
        st.highlights = self.highlights;
        st.selection = None;
        st.scroll_y = 0.0;
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        notein_sdk::Payload::to_bytes(self)
    }

    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        bincode::decode_from_slice(bytes, bincode::config::standard())
            .ok()
            .map(|(v, _)| v)
    }
}

#[must_use]
pub fn rgba_to_u32(rgba: Rgba) -> u32 {
    (u32::from(rgba[0]) << 24)
        | (u32::from(rgba[1]) << 16)
        | (u32::from(rgba[2]) << 8)
        | u32::from(rgba[3])
}

#[must_use]
pub const fn u32_to_rgba(packed: u32) -> Rgba {
    [
        ((packed >> 24) & 0xff) as u8,
        ((packed >> 16) & 0xff) as u8,
        ((packed >> 8) & 0xff) as u8,
        (packed & 0xff) as u8,
    ]
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
            highlights: Vec::new(),
            nav_history: Vec::new(),
            nav_future: Vec::new(),
        }
    }

    #[must_use]
    pub fn current_as_nav(&self) -> NavEntry {
        NavEntry {
            bible_id: self.bible_id.clone(),
            book_id: self.current_book.clone(),
            chapter: self.current_chapter,
        }
    }

    pub fn push_nav(&mut self, entry: NavEntry) {
        // Dedup: adjacent duplicates (e.g. re-click same ref) collapse
        // so back-arrow doesn't require two taps to leave.
        if self.nav_history.last() != Some(&entry) {
            self.nav_history.push(entry);
        }
        // Any explicit forward navigation (not pop_nav) invalidates the
        // forward trail — browsers behave the same way so a user never
        // sees stale forward entries after a side-step.
        self.nav_future.clear();
        // Cap depth so unbounded navigation doesn't balloon memory.
        if self.nav_history.len() > MAX_HISTORY {
            self.nav_history.drain(..self.nav_history.len() - MAX_HISTORY);
        }
    }

    /// Pop the back stack; the current position is pushed onto the
    /// forward stack so a subsequent `pop_nav_forward` restores it.
    pub fn pop_nav(&mut self) -> Option<NavEntry> {
        let entry = self.nav_history.pop()?;
        let here = self.current_as_nav();
        if self.nav_future.last() != Some(&here) {
            self.nav_future.push(here);
        }
        Some(entry)
    }

    /// Walk the forward stack. Mirror of `pop_nav` — current position
    /// moves back onto `nav_history` so the user can rewind again.
    pub fn pop_nav_forward(&mut self) -> Option<NavEntry> {
        let entry = self.nav_future.pop()?;
        let here = self.current_as_nav();
        if self.nav_history.last() != Some(&here) {
            self.nav_history.push(here);
        }
        Some(entry)
    }

    #[must_use]
    pub fn has_back(&self) -> bool {
        !self.nav_history.is_empty()
    }

    #[must_use]
    pub fn has_forward(&self) -> bool {
        !self.nav_future.is_empty()
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

    /// Ensure the current chapter is loaded in the cache. No-op when
    /// already present. Scroll resets on an actual load (i.e. after a
    /// navigation step) but is preserved when the chapter was already
    /// cached.
    pub fn ensure_current_loaded(&mut self, db: &Db) -> Result<(), DbError> {
        if self.current_chapter_data().is_some() {
            return Ok(());
        }
        let chap = db.load_chapter(&self.bible_id, &self.current_book, self.current_chapter)?;
        self.push_chapter(chap);
        self.scroll_y = 0.0;
        Ok(())
    }

    /// Move to the previous chapter. Crosses book boundaries by walking
    /// `books` backwards; the first chapter of the first book is a
    /// no-op (returns `false`).
    pub fn go_prev_chapter(&mut self, db: &Db, books: &[BookRow]) -> Result<bool, DbError> {
        if self.current_chapter > 1 {
            self.current_chapter -= 1;
            self.ensure_current_loaded(db)?;
            self.scroll_y = 0.0;
            return Ok(true);
        }
        let Some(idx) = books.iter().position(|b| b.book_id == self.current_book) else {
            return Ok(false);
        };
        if idx == 0 {
            return Ok(false);
        }
        let prev = &books[idx - 1];
        let last_chapter = db.chapter_count(&self.bible_id, &prev.book_id)?.max(1);
        self.current_book.clone_from(&prev.book_id);
        self.current_chapter = last_chapter;
        self.ensure_current_loaded(db)?;
        self.scroll_y = 0.0;
        Ok(true)
    }

    /// Move to the next chapter. Crosses book boundaries by walking
    /// `books` forward; the last chapter of the last book is a no-op
    /// (returns `false`).
    pub fn go_next_chapter(&mut self, db: &Db, books: &[BookRow]) -> Result<bool, DbError> {
        let max_in_current = db.chapter_count(&self.bible_id, &self.current_book)?;
        if self.current_chapter < max_in_current {
            self.current_chapter += 1;
            self.ensure_current_loaded(db)?;
            self.scroll_y = 0.0;
            return Ok(true);
        }
        let Some(idx) = books.iter().position(|b| b.book_id == self.current_book) else {
            return Ok(false);
        };
        if idx + 1 >= books.len() {
            return Ok(false);
        }
        let next = &books[idx + 1];
        self.current_book.clone_from(&next.book_id);
        self.current_chapter = 1;
        self.ensure_current_loaded(db)?;
        self.scroll_y = 0.0;
        Ok(true)
    }

    /// Swap the active translation while staying on the same book +
    /// chapter. Clears the chapter cache so the new text loads fresh.
    pub fn switch_bible(&mut self, db: &Db, new_bible_id: &str) -> Result<(), DbError> {
        let from = self.current_as_nav();
        self.bible_id = new_bible_id.into();
        self.chapter_cache.clear();
        self.scroll_y = 0.0;
        self.selection = None;
        // Highlights are keyed by (bible_id, book, chap, verse); dropping
        // them on a translation switch keeps the reader honest — the
        // shell's snapshot sidecar is per-item and will restore them if
        // the user flips back.
        self.highlights.clear();
        self.ensure_current_loaded(db)?;
        self.push_nav(from);
        Ok(())
    }

    /// Jump directly to `(book, chapter)`. Pushes the current position
    /// onto the back stack so the user can rewind via the toolbar.
    /// Fails if the chapter does not exist in the installed Bible.
    pub fn jump_to(&mut self, db: &Db, book: &str, chapter: u16) -> Result<(), DbError> {
        let from = self.current_as_nav();
        self.current_book = book.into();
        self.current_chapter = chapter;
        self.ensure_current_loaded(db)?;
        self.scroll_y = 0.0;
        self.selection = None;
        self.push_nav(from);
        Ok(())
    }

    /// Jump directly to a stored history entry WITHOUT re-pushing the
    /// current position — used by the back arrow so a repeated back
    /// click walks the stack monotonically.
    pub fn jump_to_entry(&mut self, db: &Db, entry: &NavEntry) -> Result<(), DbError> {
        self.bible_id.clone_from(&entry.bible_id);
        self.current_book.clone_from(&entry.book_id);
        self.current_chapter = entry.chapter;
        let current = (&self.bible_id, &self.current_book, self.current_chapter);
        self.chapter_cache
            .retain(|c| (&c.bible_id, &c.book_id, c.chapter) == current);
        self.selection = None;
        self.scroll_y = 0.0;
        self.ensure_current_loaded(db)
    }

    pub fn begin_selection(
        &mut self,
        book: impl Into<String>,
        chapter: u16,
        verse: u16,
    ) {
        self.selection = Some(VerseSelection {
            book_id: book.into(),
            chapter,
            start_verse: verse,
            end_verse: verse,
            start_char: None,
            end_char: None,
        });
    }

    pub const fn extend_selection(&mut self, verse: u16) {
        if let Some(sel) = self.selection.as_mut() {
            if verse >= sel.start_verse {
                sel.end_verse = verse;
            } else {
                sel.end_verse = sel.start_verse;
                sel.start_verse = verse;
            }
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Convert the current selection into a persistent highlight.
    /// Returns the created bookmark id, or `None` when there is no
    /// selection to promote.
    pub fn commit_selection_as_highlight(
        &mut self,
        color: Option<u32>,
        note: impl Into<String>,
    ) -> Option<BookmarkId> {
        let sel = self.selection.take()?;
        let id = BookmarkId::new_v4();
        self.highlights.push(BibleBookmark {
            id,
            book_id: sel.book_id,
            chapter: sel.chapter,
            start_verse: sel.start_verse,
            end_verse: sel.end_verse,
            color,
            note: note.into(),
        });
        Some(id)
    }

    pub fn remove_highlight(&mut self, id: BookmarkId) -> bool {
        let before = self.highlights.len();
        self.highlights.retain(|h| h.id != id);
        before != self.highlights.len()
    }

    /// Apply a highlight across `[start, end]` on `(book, chapter)`,
    /// re-using any existing bookmark whose range overlaps instead of
    /// stacking a new one on top. Overlapping bookmark gets its range
    /// unioned with the new span and its colour / note replaced.
    ///
    /// Returns the `BookmarkId` of the mutated / inserted bookmark.
    pub fn upsert_highlight(
        &mut self,
        book_id: &str,
        chapter: u16,
        start: u16,
        end: u16,
        color: Option<u32>,
        note: impl Into<String>,
    ) -> BookmarkId {
        let (lo, hi) = if start <= end { (start, end) } else { (end, start) };
        let note = note.into();
        if let Some(existing) = self.highlights.iter_mut().find(|h| {
            h.book_id == book_id
                && h.chapter == chapter
                && h.start_verse <= hi
                && h.end_verse >= lo
        }) {
            existing.start_verse = existing.start_verse.min(lo);
            existing.end_verse = existing.end_verse.max(hi);
            existing.color = color;
            existing.note = note;
            return existing.id;
        }
        let id = BookmarkId::new_v4();
        self.highlights.push(BibleBookmark {
            id,
            book_id: book_id.to_string(),
            chapter,
            start_verse: lo,
            end_verse: hi,
            color,
            note,
        });
        id
    }

    /// Highlights visible on `(book, chapter)` — the UI consults this
    /// per chapter render to decide which verses need a background
    /// wash + gutter note icon.
    #[must_use]
    pub fn highlights_on(&self, book_id: &str, chapter: u16) -> Vec<&BibleBookmark> {
        self.highlights
            .iter()
            .filter(|h| h.book_id == book_id && h.chapter == chapter)
            .collect()
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn two_book_db() -> Db {
        let db = Db::in_memory().unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO bibles (id, abbrev, name_local, language)
                 VALUES ('spa_rv1960', 'RVR1960', 'Reina Valera 1960', 'spa')",
                [],
            )?;
            for (bid, name, order) in [
                ("GEN", "Génesis", 1),
                ("EPH", "Efesios", 49),
            ] {
                conn.execute(
                    "INSERT INTO books (bible_id, book_id, name, abbrev, order_num, testament)
                     VALUES ('spa_rv1960', ?1, ?2, ?2, ?3, 'OT')",
                    params![bid, name, i64::from(order)],
                )?;
            }
            for chap in 1..=3 {
                conn.execute(
                    "INSERT INTO verses (bible_id, book_id, chapter, verse, text, paragraph_break)
                     VALUES ('spa_rv1960', 'GEN', ?1, 1, 'gen chap text', 0)",
                    params![i64::from(chap)],
                )?;
            }
            for chap in 1..=6 {
                conn.execute(
                    "INSERT INTO verses (bible_id, book_id, chapter, verse, text, paragraph_break)
                     VALUES ('spa_rv1960', 'EPH', ?1, 1, 'eph chap text', 0)",
                    params![i64::from(chap)],
                )?;
            }
            Ok(())
        })
        .unwrap();
        db
    }

    #[test]
    fn next_prev_within_book() {
        let db = two_book_db();
        let books = db.list_books("spa_rv1960").unwrap();
        let mut st = BibleViewerState::new("spa_rv1960", "GEN", 1);
        st.ensure_current_loaded(&db).unwrap();
        assert!(st.go_next_chapter(&db, &books).unwrap());
        assert_eq!(st.current_chapter, 2);
        assert!(st.go_prev_chapter(&db, &books).unwrap());
        assert_eq!(st.current_chapter, 1);
    }

    #[test]
    fn next_crosses_book_boundary() {
        let db = two_book_db();
        let books = db.list_books("spa_rv1960").unwrap();
        let mut st = BibleViewerState::new("spa_rv1960", "GEN", 3);
        st.ensure_current_loaded(&db).unwrap();
        assert!(st.go_next_chapter(&db, &books).unwrap());
        assert_eq!(st.current_book, "EPH");
        assert_eq!(st.current_chapter, 1);
    }

    #[test]
    fn prev_crosses_book_boundary() {
        let db = two_book_db();
        let books = db.list_books("spa_rv1960").unwrap();
        let mut st = BibleViewerState::new("spa_rv1960", "EPH", 1);
        st.ensure_current_loaded(&db).unwrap();
        assert!(st.go_prev_chapter(&db, &books).unwrap());
        assert_eq!(st.current_book, "GEN");
        assert_eq!(st.current_chapter, 3);
    }

    #[test]
    fn prev_at_first_chapter_first_book_is_noop() {
        let db = two_book_db();
        let books = db.list_books("spa_rv1960").unwrap();
        let mut st = BibleViewerState::new("spa_rv1960", "GEN", 1);
        st.ensure_current_loaded(&db).unwrap();
        assert!(!st.go_prev_chapter(&db, &books).unwrap());
        assert_eq!(st.current_book, "GEN");
        assert_eq!(st.current_chapter, 1);
    }

    #[test]
    fn switch_bible_clears_cache_and_reloads() {
        let db = two_book_db();
        let mut st = BibleViewerState::new("spa_rv1960", "GEN", 1);
        st.ensure_current_loaded(&db).unwrap();
        assert_eq!(st.chapter_cache.len(), 1);
        st.scroll_y = 300.0;

        // Install a second translation sharing the same book id so the
        // reload path finds a chapter in the new context too.
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO bibles (id, abbrev, name_local, language)
                 VALUES ('spa_lbla', 'LBLA', 'La Biblia de las Américas', 'spa')",
                [],
            )?;
            conn.execute(
                "INSERT INTO books (bible_id, book_id, name, abbrev, order_num, testament)
                 VALUES ('spa_lbla', 'GEN', 'Génesis', 'Gn', 1, 'OT')",
                [],
            )?;
            conn.execute(
                "INSERT INTO verses (bible_id, book_id, chapter, verse, text, paragraph_break)
                 VALUES ('spa_lbla', 'GEN', 1, 1, 'alt verse text', 0)",
                [],
            )?;
            Ok(())
        })
        .unwrap();

        st.switch_bible(&db, "spa_lbla").unwrap();
        assert_eq!(st.bible_id, "spa_lbla");
        assert!(st.scroll_y.abs() < f32::EPSILON);
        assert_eq!(st.chapter_cache.len(), 1);
        let chap = st.current_chapter_data().unwrap();
        assert!(chap.verses[0].text.contains("alt"));
    }

    #[test]
    fn commit_selection_as_highlight_round_trip() {
        let mut st = BibleViewerState::new("spa_rv1960", "EPH", 6);
        st.begin_selection("EPH", 6, 3);
        st.extend_selection(5);
        let id = st
            .commit_selection_as_highlight(Some(0xffcc_3399), "mi nota")
            .unwrap();
        assert!(st.selection.is_none());
        assert_eq!(st.highlights.len(), 1);
        assert_eq!(st.highlights_on("EPH", 6).len(), 1);
        assert_eq!(st.highlights_on("GEN", 1).len(), 0);
        assert!(st.remove_highlight(id));
        assert!(st.highlights.is_empty());
    }

    #[test]
    fn selection_extend_normalises_range() {
        let mut st = BibleViewerState::new("spa_rv1960", "EPH", 6);
        st.begin_selection("EPH", 6, 3);
        st.extend_selection(5);
        let sel = st.selection.clone().unwrap();
        assert_eq!(sel.start_verse, 3);
        assert_eq!(sel.end_verse, 5);
        st.extend_selection(1);
        let sel = st.selection.unwrap();
        assert_eq!(sel.start_verse, 1);
        assert_eq!(sel.end_verse, 3);
    }
}
