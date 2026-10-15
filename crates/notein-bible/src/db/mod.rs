//! SQLite + FTS5 storage layer.
//!
//! One database file holds every installed translation. Tables are
//! keyed by `bible_id` so adding a Bible is a bulk-insert, not a new
//! file. The FTS5 virtual table `verses_fts` indexes verse text with
//! `unicode61 remove_diacritics 2` so accent-insensitive queries work
//! out of the box.
//!
//! A single `Arc<Mutex<Connection>>` serialises writes. SQLite does its
//! own fine-grained locking on reads via WAL mode (enabled at
//! connection open).

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

mod queries;
mod schema;
mod search;

pub use queries::{BookRow, ChapterRow, HeaderRow, VerseRow};
pub use search::{SearchHit, snippet_markers};

#[derive(Debug)]
pub enum DbError {
    Sqlite(rusqlite::Error),
    MissingBible { id: String },
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "sqlite: {e}"),
            Self::MissingBible { id } => write!(f, "bible not installed: {id}"),
        }
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(e) => Some(e),
            Self::MissingBible { .. } => None,
        }
    }
}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

pub type Result<T> = std::result::Result<T, DbError>;

#[derive(Clone, Debug)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    /// Open or create the database file. Runs `schema::migrate` so a
    /// first-time call lands a usable schema; a subsequent call is a
    /// cheap no-op (`CREATE … IF NOT EXISTS`).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path.as_ref())?;
        // WAL = concurrent readers vs one writer. `synchronous=NORMAL`
        // trades a tiny durability window on crash for a large speedup
        // on bulk inserts (importer path).
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self { conn: Arc::new(Mutex::new(conn)) };
        schema::migrate(&db)?;
        Ok(db)
    }

    /// In-memory database — tests only. Each call gets its own fresh
    /// connection, so parallel tests don't trample each other.
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self { conn: Arc::new(Mutex::new(conn)) };
        schema::migrate(&db)?;
        Ok(db)
    }

    pub(crate) fn with_conn<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let guard = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&guard)
    }

    // ─── Public API delegates (see queries.rs / search.rs) ─────────

    pub fn list_bibles(&self) -> Result<Vec<queries::BibleRow>> {
        queries::list_bibles(self)
    }

    pub fn list_books(&self, bible_id: &str) -> Result<Vec<BookRow>> {
        queries::list_books(self, bible_id)
    }

    pub fn chapter_count(&self, bible_id: &str, book_id: &str) -> Result<u16> {
        queries::chapter_count(self, bible_id, book_id)
    }

    pub fn load_chapter(&self, bible_id: &str, book_id: &str, chapter: u16) -> Result<ChapterRow> {
        queries::load_chapter(self, bible_id, book_id, chapter)
    }

    pub fn search(&self, bible_id: Option<&str>, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        search::search(self, bible_id, query, limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn seed_eph6(db: &Db) {
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO bibles (id, abbrev, name_local, language)
                 VALUES ('spa_rv1960', 'RVR1960', 'Reina Valera 1960', 'spa')",
                [],
            )?;
            conn.execute(
                "INSERT INTO books (bible_id, book_id, name, abbrev, order_num, testament)
                 VALUES ('spa_rv1960', 'EPH', 'Efesios', 'Ef', 49, 'NT')",
                [],
            )?;
            for (verse, text) in [
                (1u16, "Hijos, obedeced en el Señor a vuestros padres, porque esto es justo."),
                (2, "Honra a tu padre y a tu madre, que es el primer mandamiento con promesa;"),
                (3, "para que te vaya bien, y seas de larga vida sobre la tierra."),
            ] {
                conn.execute(
                    "INSERT INTO verses (bible_id, book_id, chapter, verse, text, paragraph_break)
                     VALUES ('spa_rv1960', 'EPH', 6, ?1, ?2, 0)",
                    params![verse as i64, text],
                )?;
            }
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn migrate_and_load_chapter() {
        let db = Db::in_memory().unwrap();
        seed_eph6(&db);
        let chap = db.load_chapter("spa_rv1960", "EPH", 6).unwrap();
        assert_eq!(chap.verses.len(), 3);
        assert_eq!(chap.verses[0].verse, 1);
        assert!(chap.verses[0].text.contains("obedeced"));
    }

    #[test]
    fn fts_accent_insensitive() {
        let db = Db::in_memory().unwrap();
        seed_eph6(&db);
        let hits = db.search(Some("spa_rv1960"), "senor", 10).unwrap();
        assert!(!hits.is_empty(), "expected hit for 'senor' matching 'Señor'");
        assert!(hits[0].snippet.contains(search::SNIPPET_OPEN));
    }

    #[test]
    fn fts_scoped_by_bible() {
        let db = Db::in_memory().unwrap();
        seed_eph6(&db);
        let all = db.search(None, "padre", 10).unwrap();
        let scoped = db.search(Some("spa_rv1960"), "padre", 10).unwrap();
        assert_eq!(all.len(), scoped.len());
        let miss = db.search(Some("eng_kjv"), "padre", 10).unwrap();
        assert!(miss.is_empty());
    }
}
