//! Schema migration — idempotent via `CREATE … IF NOT EXISTS`.
//!
//! The FTS5 virtual table uses the `content='verses'` option so verse
//! text is not duplicated between the base table and the index; the
//! trigger block keeps the two in sync on insert / update / delete.
//!
//! Tokenizer: `unicode61 remove_diacritics 2` folds accents and
//! case-insensitively tokenises on Unicode word boundaries — queries
//! for "corazon" match "corazón" without extra preprocessing.

use super::{Db, Result};

const SCHEMA_SQL: &str = r#"
BEGIN;

CREATE TABLE IF NOT EXISTS bibles (
    id          TEXT PRIMARY KEY,
    abbrev      TEXT NOT NULL,
    name_local  TEXT NOT NULL,
    language    TEXT NOT NULL
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

CREATE TABLE IF NOT EXISTS cross_refs (
    bible_id    TEXT NOT NULL,
    src_book    TEXT NOT NULL,
    src_chap    INTEGER NOT NULL,
    src_verse   INTEGER NOT NULL,
    tgt_book    TEXT NOT NULL,
    tgt_chap    INTEGER NOT NULL,
    tgt_verse   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_xref_src
    ON cross_refs(bible_id, src_book, src_chap, src_verse);

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

CREATE TABLE IF NOT EXISTS highlights (
    id            BLOB PRIMARY KEY,
    bible_id      TEXT NOT NULL,
    book_id       TEXT NOT NULL,
    chapter       INTEGER NOT NULL,
    start_verse   INTEGER NOT NULL,
    end_verse     INTEGER NOT NULL,
    color         INTEGER,
    note          TEXT NOT NULL DEFAULT '',
    created_at    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_highlights_loc
    ON highlights(bible_id, book_id, chapter);

COMMIT;
"#;

pub(crate) fn migrate(db: &Db) -> Result<()> {
    db.with_conn(|conn| {
        conn.execute_batch(SCHEMA_SQL)?;
        Ok(())
    })
}
