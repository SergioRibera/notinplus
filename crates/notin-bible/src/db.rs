//! `SQLite` + FTS5 storage layer.
//!
//! One database file holds every installed translation. Tables are
//! keyed by `bible_id` so adding a Bible is a bulk-insert, not a new
//! file. The FTS5 virtual table `verses_fts` indexes verse text with
//! `unicode61 remove_diacritics 2` so accent-insensitive queries work
//! out of the box.
//!
//! One connection writes (installs, fixes) and a second, read-only one
//! serves the reader and the search, so an install running in the
//! background never makes a page wait: in WAL mode a reader sees the
//! last commit while the writer works on the next.

use std::path::Path;
use std::sync::{Arc, Mutex};

use notin_i18n::tr;
use notin_sdk::UserMessage;
use rusqlite::{Connection, OpenFlags};

mod queries;
mod schema;
mod search;

pub use queries::{
    BibleRow, BookRow, ChapterRow, HeaderRow, LinkRow, PlacedLink, RenderingRow, VerseRow,
    VerseTextRow,
};
pub use search::{SearchHit, snippet_markers};

#[derive(Debug)]
pub enum DbError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    MissingBible {
        id: String,
    },
    /// A setting could not be encoded for storage.
    Encode(String),
}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "sqlite: {e}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::MissingBible { id } => write!(f, "bible not installed: {id}"),
            Self::Encode(reason) => write!(f, "encode setting: {reason}"),
        }
    }
}

impl std::error::Error for DbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(e) => Some(e),
            Self::Io(e) => Some(e),
            Self::MissingBible { .. } | Self::Encode(_) => None,
        }
    }
}

impl UserMessage for DbError {
    fn user_message(&self) -> String {
        use rusqlite::ErrorCode;
        use std::io::ErrorKind;
        match self {
            Self::Io(err) => err.user_message(),
            Self::Sqlite(rusqlite::Error::SqliteFailure(failure, _)) => match failure.code {
                ErrorCode::DiskFull => ErrorKind::StorageFull.user_message(),
                ErrorCode::PermissionDenied | ErrorCode::CannotOpen => {
                    ErrorKind::PermissionDenied.user_message()
                }
                ErrorCode::ReadOnly => ErrorKind::ReadOnlyFilesystem.user_message(),
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked => {
                    ErrorKind::ResourceBusy.user_message()
                }
                ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase => tr!("errors-db-damaged"),
                _ => tr!("errors-db-other"),
            },
            Self::Sqlite(_) | Self::Encode(_) => tr!("errors-db-other"),
            Self::MissingBible { id } => tr!("errors-db-missing-bible", "id" => id.as_str()),
        }
    }
}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<std::io::Error> for DbError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, DbError>;

#[derive(Clone, Debug)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
    /// Read-only connection to the same file; `None` in memory, where
    /// a second connection would see another database.
    read: Option<Arc<Mutex<Connection>>>,
}

impl Db {
    /// Open or create the database file. Runs `schema::migrate` so a
    /// first-time call lands a usable schema; a subsequent call is a
    /// cheap no-op (`CREATE … IF NOT EXISTS`).
    #[cfg_attr(feature = "sismo", sismo::function("bible::db_open"))]
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path.as_ref())?;
        // WAL = concurrent readers vs one writer. `synchronous=NORMAL`
        // trades a tiny durability window on crash for a large speedup
        // on bulk inserts (importer path).
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut db = Self {
            conn: Arc::new(Mutex::new(conn)),
            read: None,
        };
        schema::migrate(&db)?;
        let read = Connection::open_with_flags(
            path.as_ref(),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        db.read = Some(Arc::new(Mutex::new(read)));
        Ok(db)
    }

    /// Resolve the canonical on-disk location for the Bible DB — a
    /// `bible.db` file under the shared notinplus data directory
    /// (`istmo::path::data_dir()/notinplus/bible/`). Mirrors the layout
    /// the rest of notinplus uses for library bodies, pane layouts,
    /// data store namespaces, etc.
    #[must_use]
    pub fn default_path() -> std::path::PathBuf {
        istmo_core::path::data_dir()
            .join("notinplus")
            .join("bible")
            .join("bible.db")
    }

    /// Open the DB at [`Self::default_path`]. Creates the enclosing
    /// directory tree on first run.
    ///
    /// # Errors
    /// The directory could not be created or the database could not be
    /// opened or migrated.
    pub fn open_default() -> Result<Self> {
        Self::open(Self::default_path())
    }

    /// In-memory database — tests only. Each call gets its own fresh
    /// connection, so parallel tests don't trample each other.
    #[cfg_attr(feature = "sismo", sismo::function("bible::db_open_memory"))]
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
            read: None,
        };
        schema::migrate(&db)?;
        Ok(db)
    }

    /// Run a read on the read-only connection.
    pub(crate) fn with_conn<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let conn = self.read.as_ref().unwrap_or(&self.conn);
        let guard = {
            #[cfg(feature = "sismo")]
            sismo::scope!("bible::db_read_lock");
            conn.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        };
        f(&guard)
    }

    #[cfg_attr(not(feature = "importer"), allow(dead_code))]
    pub(crate) fn with_conn_mut<R>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<R>,
    ) -> Result<R> {
        let mut guard = {
            #[cfg(feature = "sismo")]
            sismo::scope!("bible::db_write_lock");
            self.conn
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        };
        f(&mut guard)
    }

    // ─── Public API delegates (see queries.rs / search.rs) ─────────

    /// Every installed translation.
    ///
    /// # Errors
    /// The database could not be read.
    pub fn list_bibles(&self) -> Result<Vec<queries::BibleRow>> {
        queries::list_bibles(self)
    }

    /// Translations whose download started but has not finished; they
    /// stay out of [`Self::list_bibles`] and the search.
    ///
    /// # Errors
    /// The database could not be read.
    pub fn pending_installs(&self) -> Result<Vec<String>> {
        queries::pending_installs(self)
    }

    /// Show `bible_id` once its download is complete.
    ///
    /// # Errors
    /// The database could not be written.
    pub fn finish_install(&self, bible_id: &str) -> Result<()> {
        queries::finish_install(self, bible_id)
    }

    /// The plugin settings saved on this device.
    ///
    /// # Errors
    /// The database could not be read.
    pub fn load_settings(&self) -> Result<Vec<(String, notin_sdk::ConfigValue)>> {
        queries::load_settings(self)
    }

    /// Save one plugin setting.
    ///
    /// # Errors
    /// The value could not be encoded or the database could not be
    /// written.
    pub fn save_setting(&self, key: &str, value: &notin_sdk::ConfigValue) -> Result<()> {
        queries::save_setting(self, key, value)
    }

    /// A fact about the database itself (`meta` table), if recorded.
    ///
    /// # Errors
    /// The database could not be read.
    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        queries::meta_get(self, key)
    }

    /// Record a fact about the database itself.
    ///
    /// # Errors
    /// The database could not be written.
    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        queries::meta_set(self, key, value)
    }

    /// Every verse of a translation, for the aligner.
    ///
    /// # Errors
    /// The database could not be read.
    pub fn verses_of_bible(&self, bible_id: &str) -> Result<Vec<VerseTextRow>> {
        queries::verses_of_bible(self, bible_id)
    }

    /// Replace a translation's alignment with one testament (0 Old, 1 New).
    ///
    /// # Errors
    /// The database could not be written.
    pub fn replace_alignment(
        &self,
        bible_id: &str,
        testament: u8,
        links: &[PlacedLink],
        renderings: &[(u32, RenderingRow)],
    ) -> Result<()> {
        queries::replace_alignment(self, bible_id, testament, links, renderings)
    }

    /// Which translation tokens render each original word of a verse.
    ///
    /// # Errors
    /// The database could not be read.
    pub fn alignment_links(
        &self,
        bible_id: &str,
        testament: u8,
        book: u8,
        chapter: u16,
        verse: u16,
    ) -> Result<Vec<LinkRow>> {
        queries::alignment_links(self, bible_id, testament, book, chapter, verse)
    }

    /// How a translation renders a lemma across the text.
    ///
    /// # Errors
    /// The database could not be read.
    pub fn renderings(
        &self,
        bible_id: &str,
        testament: u8,
        lemma: u32,
    ) -> Result<Vec<RenderingRow>> {
        queries::renderings(self, bible_id, testament, lemma)
    }

    /// Books of a translation.
    ///
    /// # Errors
    /// The database could not be read.
    pub fn list_books(&self, bible_id: &str) -> Result<Vec<BookRow>> {
        queries::list_books(self, bible_id)
    }

    /// Number of chapters in one book.
    ///
    /// # Errors
    /// The database could not be read.
    pub fn chapter_count(&self, bible_id: &str, book_id: &str) -> Result<u16> {
        queries::chapter_count(self, bible_id, book_id)
    }

    /// Every verse of one chapter.
    ///
    /// # Errors
    /// The database could not be read, or [`DbError::MissingBible`] when
    /// the translation is not installed.
    pub fn load_chapter(&self, bible_id: &str, book_id: &str, chapter: u16) -> Result<ChapterRow> {
        queries::load_chapter(self, bible_id, book_id, chapter)
    }

    /// Full-text search in one translation, or in all of them when
    /// `bible_id` is `None`.
    ///
    /// # Errors
    /// The database could not be read.
    pub fn search(
        &self,
        bible_id: Option<&str>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        search::search(self, bible_id, query, limit)
    }

    /// Cross-references originating in `(book, chapter)`. Returns a
    /// per-verse map so the reader can render inline chips without one
    /// query per verse. Includes ALL targets even for the same source
    /// verse — collapsing happens UI-side.
    #[cfg_attr(feature = "sismo", sismo::function("bible::db_cross_refs"))]
    pub fn cross_refs_for_chapter(&self, book_id: &str, chapter: u16) -> Result<Vec<CrossRefRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT src_verse, tgt_book, tgt_chap, tgt_verse
                 FROM cross_refs
                 WHERE src_book = ?1 COLLATE NOCASE AND src_chap = ?2
                 ORDER BY src_verse, tgt_book, tgt_chap, tgt_verse",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![book_id, i64::from(chapter)], |r| {
                    Ok(CrossRefRow {
                        src_verse: r.get::<_, u16>(0)?,
                        tgt_book: r.get(1)?,
                        tgt_chap: r.get::<_, u16>(2)?,
                        tgt_verse: r.get::<_, u16>(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            #[cfg(feature = "sismo")]
            sismo::counter!("bible.rows_returned", rows.len());
            Ok(rows)
        })
    }

    /// Fetch the display text of a single verse. Used by the cross-ref
    /// hover popup — the row count is `0..=1` so the call stays cheap
    /// per chip.
    #[cfg_attr(feature = "sismo", sismo::function("bible::db_verse_text"))]
    pub fn verse_text(
        &self,
        bible_id: &str,
        book_id: &str,
        chapter: u16,
        verse: u16,
    ) -> Result<Option<String>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT text FROM verses
                 WHERE bible_id = ?1 AND book_id = ?2 AND chapter = ?3 AND verse = ?4
                 LIMIT 1",
            )?;
            let text = stmt
                .query_row(
                    rusqlite::params![bible_id, book_id, i64::from(chapter), i64::from(verse)],
                    |r| r.get::<_, String>(0),
                )
                .ok();
            Ok(text)
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrossRefRow {
    pub src_verse: u16,
    pub tgt_book: String,
    pub tgt_chap: u16,
    pub tgt_verse: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FootnoteRow {
    pub verse: u16,
    pub note_id: i64,
    pub caller: String,
    pub text: String,
}

impl Db {
    /// Publisher-authored footnotes (via helloao.org) for `(book,
    /// chapter)` under `bible_id`. Ordered by verse then `note_id` so the
    /// reader can walk them in-line with the paragraph.
    #[cfg_attr(feature = "sismo", sismo::function("bible::db_footnotes"))]
    pub fn footnotes_for_chapter(
        &self,
        bible_id: &str,
        book_id: &str,
        chapter: u16,
    ) -> Result<Vec<FootnoteRow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT verse, note_id, caller, text
                 FROM footnotes
                 WHERE bible_id = ?1 AND book_id = ?2 AND chapter = ?3
                 ORDER BY verse, note_id",
            )?;
            let rows = stmt
                .query_map(
                    rusqlite::params![bible_id, book_id, i64::from(chapter)],
                    |r| {
                        Ok(FootnoteRow {
                            verse: r.get::<_, u16>(0)?,
                            note_id: r.get(1)?,
                            caller: r.get(2)?,
                            text: r.get(3)?,
                        })
                    },
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            #[cfg(feature = "sismo")]
            sismo::counter!("bible.rows_returned", rows.len());
            Ok(rows)
        })
    }

    /// `true` when at least one footnote row exists for `(bible, book,
    /// chap)`. Used to decide whether the lazy fetcher needs to hit
    /// helloao this render pass.
    #[cfg_attr(feature = "sismo", sismo::function("bible::db_has_footnotes"))]
    pub fn has_footnotes_for_chapter(
        &self,
        bible_id: &str,
        book_id: &str,
        chapter: u16,
    ) -> Result<bool> {
        self.with_conn(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(1) FROM footnotes
                 WHERE bible_id = ?1 AND book_id = ?2 AND chapter = ?3",
                rusqlite::params![bible_id, book_id, i64::from(chapter)],
                |r| r.get(0),
            )?;
            Ok(count > 0)
        })
    }

    /// `true` when at least one section header row exists for `(bible,
    /// book, chap)`. The lazy helloao enricher uses this to skip
    /// chapters already populated (either from the primary importer or
    /// from a prior enrichment pass).
    #[cfg_attr(feature = "sismo", sismo::function("bible::db_has_headers"))]
    pub fn has_headers_for_chapter(
        &self,
        bible_id: &str,
        book_id: &str,
        chapter: u16,
    ) -> Result<bool> {
        self.with_conn(|conn| {
            let count: i64 = conn.query_row(
                "SELECT COUNT(1) FROM headers
                 WHERE bible_id = ?1 AND book_id = ?2 AND chapter = ?3",
                rusqlite::params![bible_id, book_id, i64::from(chapter)],
                |r| r.get(0),
            )?;
            Ok(count > 0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn seed_eph6(db: &Db) {
        db.with_conn_mut(|conn| {
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
                (
                    1u16,
                    "Hijos, obedeced en el Señor a vuestros padres, porque esto es justo.",
                ),
                (
                    2,
                    "Honra a tu padre y a tu madre, que es el primer mandamiento con promesa;",
                ),
                (
                    3,
                    "para que te vaya bien, y seas de larga vida sobre la tierra.",
                ),
            ] {
                conn.execute(
                    "INSERT INTO verses (bible_id, book_id, chapter, verse, text, paragraph_break)
                     VALUES ('spa_rv1960', 'EPH', 6, ?1, ?2, 0)",
                    params![i64::from(verse), text],
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
        assert!(
            !hits.is_empty(),
            "expected hit for 'senor' matching 'Señor'"
        );
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

    #[test]
    fn pending_install_stays_hidden_until_finished() {
        let db = Db::in_memory().unwrap();
        seed_eph6(&db);
        db.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO installs_pending (bible_id) VALUES ('spa_rv1960')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert!(db.list_bibles().unwrap().is_empty());
        assert!(db.search(None, "padre", 10).unwrap().is_empty());
        assert_eq!(db.pending_installs().unwrap(), ["spa_rv1960"]);

        db.finish_install("spa_rv1960").unwrap();
        assert_eq!(db.list_bibles().unwrap().len(), 1);
        assert!(!db.search(None, "padre", 10).unwrap().is_empty());
    }

    #[test]
    fn a_file_database_reads_what_its_writer_committed() {
        let dir = std::env::temp_dir().join(format!("notin-bible-{}", std::process::id()));
        let path = dir.join("bible.db");
        let _ = std::fs::remove_dir_all(&dir);
        let db = Db::open(&path).unwrap();
        assert!(db.list_bibles().unwrap().is_empty());
        seed_eph6(&db);
        assert_eq!(db.list_bibles().unwrap().len(), 1);
        assert!(!db.search(None, "padre", 10).unwrap().is_empty());
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_resumes_installs_cut_before_cross_refs() {
        let db = Db::in_memory().unwrap();
        seed_eph6(&db);
        db.with_conn(|conn| {
            conn.pragma_update(None, "user_version", 0)?;
            Ok(())
        })
        .unwrap();
        schema::migrate(&db).unwrap();
        assert_eq!(db.pending_installs().unwrap(), ["spa_rv1960"]);

        db.finish_install("spa_rv1960").unwrap();
        schema::migrate(&db).unwrap();
        assert!(db.pending_installs().unwrap().is_empty());
    }
}

/// Helpers for tests that need an installed translation.
#[cfg(test)]
pub(crate) mod test_support {
    use rusqlite::params;

    use super::Db;

    /// Install `bible_id` with the given `(book_id, chapter, verse, text)`
    /// rows, books in the order they first appear.
    pub(crate) fn install_sample_bible(db: &Db, bible_id: &str, verses: &[(&str, u16, u16, &str)]) {
        db.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO bibles (id, abbrev, name_local, language) VALUES (?1, ?1, ?1, 'spa')",
                params![bible_id],
            )?;
            let mut books: Vec<&str> = Vec::new();
            for (book, _, _, _) in verses {
                if !books.contains(book) {
                    books.push(book);
                }
            }
            for (order, book) in books.iter().enumerate() {
                conn.execute(
                    "INSERT INTO books (bible_id, book_id, name, abbrev, order_num, testament)
                     VALUES (?1, ?2, ?2, ?2, ?3, 'NT')",
                    params![bible_id, book, order],
                )?;
            }
            for (book, chapter, verse, text) in verses {
                conn.execute(
                    "INSERT INTO verses (bible_id, book_id, chapter, verse, text, paragraph_break)
                     VALUES (?1, ?2, ?3, ?4, ?5, 0)",
                    params![bible_id, book, chapter, verse, text],
                )?;
            }
            Ok(())
        })
        .unwrap();
    }
}
