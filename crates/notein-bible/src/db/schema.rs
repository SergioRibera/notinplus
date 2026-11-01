//! Schema migration — idemotent via `CREATE … IF NOT EXISTS`.
//!
//! The FTS5 virtual table uses the `content='verses'` otion so verse
//! text is not duplicated between the base table and the index; the
//! trigger block keeps the two in sync on insert / update / delete.
//!
//! Tokenizer: `unicode61 remove_diacritics 2` folds accents and
//! case-insensitively tokenises on Unicode word boundaries — queries
//! for "corazon" match "corazón" without extra preprocessing.

use super::{Db, Result};

const SCHEMA_SQL: &str = r"
BEGIN;

CREATE TABLE IF NOT EXISTS bibles (
    id          TEXT PRIMARY KEY,
    abbrev      TEXT NOT NULL,
    name_local  TEXT NOT NULL,
    language    TEXT NOT NULL
);

-- A translation still downloading, or cut short; hidden until done.
CREATE TABLE IF NOT EXISTS installs_pending (
    bible_id    TEXT PRIMARY KEY,
    FOREIGN KEY (bible_id) REFERENCES bibles(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS books (
    bible_id    TEXT NOT NULL,
    book_id     TEXT NOT NULL,
    name        TEXT NOT NULL,
    abbrev      TEXT NOT NULL,
    order_num   INTEGER NOT NULL,
    testament   TEXT NOT NULL,
    PRIMARY KEY (bible_id, book_id),
    FOREIGN KEY (bible_id) REFERENCES bibles(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_books_order
    ON books(bible_id, order_num);

CREATE TABLE IF NOT EXISTS verses (
    bible_id         TEXT NOT NULL,
    book_id          TEXT NOT NULL,
    chapter          INTEGER NOT NULL,
    verse            INTEGER NOT NULL,
    text             TEXT NOT NULL,
    paragraph_break  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (bible_id, book_id, chapter, verse),
    FOREIGN KEY (bible_id, book_id) REFERENCES books(bible_id, book_id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_verses_chapter
    ON verses(bible_id, book_id, chapter);

CREATE TABLE IF NOT EXISTS headers (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    bible_id      TEXT NOT NULL,
    book_id       TEXT NOT NULL,
    chapter       INTEGER NOT NULL,
    before_verse  INTEGER NOT NULL,
    text          TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_headers_loc
    ON headers(bible_id, book_id, chapter, before_verse);

-- The plugin's settings, one bincoded `ConfigValue` per key.
CREATE TABLE IF NOT EXISTS plugin_settings (
    key         TEXT PRIMARY KEY,
    value       BLOB NOT NULL
);

-- What the database holds that is not a translation: the level of the
-- cross-reference set, for one.
CREATE TABLE IF NOT EXISTS meta (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS footnotes (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    bible_id    TEXT NOT NULL,
    book_id     TEXT NOT NULL,
    chapter     INTEGER NOT NULL,
    verse       INTEGER NOT NULL,
    note_id     INTEGER NOT NULL,
    caller      TEXT NOT NULL DEFAULT '',
    text        TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_footnotes_loc
    ON footnotes(bible_id, book_id, chapter);
CREATE UNIQUE INDEX IF NOT EXISTS idx_footnotes_pk
    ON footnotes(bible_id, book_id, chapter, note_id);

CREATE VIRTUAL TABLE IF NOT EXISTS verses_fts USING fts5(
    text,
    bible_id UNINDEXED,
    book_id  UNINDEXED,
    chapter  UNINDEXED,
    verse    UNINDEXED,
    content  = 'verses',
    content_rowid = 'rowid',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS verses_ai
AFTER INSERT ON verses BEGIN
    INSERT INTO verses_fts(rowid, text, bible_id, book_id, chapter, verse)
    VALUES (new.rowid, new.text, new.bible_id, new.book_id, new.chapter, new.verse);
END;

CREATE TRIGGER IF NOT EXISTS verses_ad
AFTER DELETE ON verses BEGIN
    INSERT INTO verses_fts(verses_fts, rowid, text, bible_id, book_id, chapter, verse)
    VALUES ('delete', old.rowid, old.text, old.bible_id, old.book_id, old.chapter, old.verse);
END;

CREATE TRIGGER IF NOT EXISTS verses_au
AFTER UPDATE ON verses BEGIN
    INSERT INTO verses_fts(verses_fts, rowid, text, bible_id, book_id, chapter, verse)
    VALUES ('delete', old.rowid, old.text, old.bible_id, old.book_id, old.chapter, old.verse);
    INSERT INTO verses_fts(rowid, text, bible_id, book_id, chapter, verse)
    VALUES (new.rowid, new.text, new.bible_id, new.book_id, new.chapter, new.verse);
END;

COMMIT;
";

/// Cross-references are one set for every translation: fetch.bible keys
/// them by book, chapter and verse, so a second Bible adds nothing.
const CROSS_REFS_SQL: &str = r"
CREATE TABLE IF NOT EXISTS cross_refs (
    src_book    TEXT NOT NULL,
    src_chap    INTEGER NOT NULL,
    src_verse   INTEGER NOT NULL,
    tgt_book    TEXT NOT NULL,
    tgt_chap    INTEGER NOT NULL,
    tgt_verse   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_xref_src
    ON cross_refs(src_book, src_chap, src_verse);
";

/// Bumped by each one-off data fix below.
const VERSION: i64 = 3;

#[cfg_attr(feature = "sismo", sismo::function("bible::db_migrate"))]
pub(crate) fn migrate(db: &Db) -> Result<()> {
    db.with_conn_mut(|conn| {
        conn.execute_batch(SCHEMA_SQL)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            // Installs from before `installs_pending` could stop after the
            // verses; one without headings never finished.
            conn.execute_batch(
                "INSERT OR IGNORE INTO installs_pending (bible_id)
                 SELECT id FROM bibles
                 WHERE id NOT IN (SELECT DISTINCT bible_id FROM headers);",
            )?;
        }
        if version < 2 {
            repair_book_order(conn)?;
        }
        if version < 3 {
            // Cross-references used to be stored once per translation; the
            // shared set downloads again at the level in the settings.
            conn.execute_batch("DROP TABLE IF EXISTS cross_refs;")?;
        }
        conn.execute_batch(CROSS_REFS_SQL)?;
        conn.pragma_update(None, "user_version", VERSION)?;
        Ok(())
    })
}

/// Installs made before the importer kept the manifest order stored the
/// books alphabetically. Put the Protestant canon back in its order;
/// any other book keeps its place after it.
fn repair_book_order(conn: &rusqlite::Connection) -> Result<()> {
    let mut books = conn.prepare("SELECT bible_id, book_id, order_num FROM books")?;
    let rows = books
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u16>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut update =
        conn.prepare("UPDATE books SET order_num = ?1 WHERE bible_id = ?2 AND book_id = ?3")?;
    for (bible_id, book_id, order_num) in rows {
        let repaired = crate::canon::protestant_position(&book_id)
            .unwrap_or_else(|| crate::canon::PROTESTANT_LEN.saturating_add(order_num));
        update.execute(rusqlite::params![i64::from(repaired), bible_id, book_id])?;
    }
    Ok(())
}
