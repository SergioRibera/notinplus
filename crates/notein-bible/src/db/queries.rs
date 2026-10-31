//! Row shapes + non-FTS queries (book list, chapter load, etc.).

use rusqlite::{OptionalExtension, params};

use super::{Db, DbError, Result};

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BibleRow {
    pub id: String,
    pub abbrev: String,
    pub name_local: String,
    pub language: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookRow {
    pub book_id: String,
    pub name: String,
    pub abbrev: String,
    pub order_num: u16,
    pub testament: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerseRow {
    pub verse: u16,
    pub text: String,
    /// `true` when this verse starts a new paragraph in the source
    /// typography. UI respects it when grouping verses inside the
    /// flowing paragraph nodes.
    pub paragraph_break: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderRow {
    /// Verse number this header appears *before*. `0` for a
    /// chapter-level header rendered at the very top.
    pub before_verse: u16,
    pub text: String,
}

/// Full chapter payload loaded in one DB round-trip. The UI assembles
/// paragraph groups + section headers directly from these three
/// collections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChapterRow {
    pub bible_id: String,
    pub book_id: String,
    pub book_name: String,
    pub chapter: u16,
    pub verses: Vec<VerseRow>,
    pub headers: Vec<HeaderRow>,
}

#[cfg_attr(feature = "sismo", sismo::function("bible::db_list_bibles"))]
pub(crate) fn list_bibles(db: &Db) -> Result<Vec<BibleRow>> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare_cached(
            "SELECT id, abbrev, name_local, language FROM bibles
             WHERE id NOT IN (SELECT bible_id FROM installs_pending)
             ORDER BY name_local",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(BibleRow {
                    id: r.get(0)?,
                    abbrev: r.get(1)?,
                    name_local: r.get(2)?,
                    language: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

#[cfg_attr(feature = "sismo", sismo::function("bible::db_pending_installs"))]
pub(crate) fn pending_installs(db: &Db) -> Result<Vec<String>> {
    db.with_conn(|conn| {
        let mut stmt =
            conn.prepare_cached("SELECT bible_id FROM installs_pending ORDER BY bible_id")?;
        let rows = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(rows)
    })
}

#[cfg_attr(feature = "sismo", sismo::function("bible::db_finish_install"))]
pub(crate) fn finish_install(db: &Db, bible_id: &str) -> Result<()> {
    db.with_conn_mut(|conn| {
        conn.execute(
            "DELETE FROM installs_pending WHERE bible_id = ?1",
            [bible_id],
        )?;
        Ok(())
    })
}

#[cfg_attr(feature = "sismo", sismo::function("bible::db_list_books"))]
pub(crate) fn list_books(db: &Db, bible_id: &str) -> Result<Vec<BookRow>> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare_cached(
            "SELECT book_id, name, abbrev, order_num, testament
             FROM books
             WHERE bible_id = ?1
             ORDER BY order_num",
        )?;
        let rows = stmt
            .query_map(params![bible_id], |r| {
                Ok(BookRow {
                    book_id: r.get(0)?,
                    name: r.get(1)?,
                    abbrev: r.get(2)?,
                    order_num: r.get::<_, u16>(3)?,
                    testament: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

#[cfg_attr(feature = "sismo", sismo::function("bible::db_chapter_count"))]
pub(crate) fn chapter_count(db: &Db, bible_id: &str, book_id: &str) -> Result<u16> {
    db.with_conn(|conn| {
        let count: Option<i64> = conn
            .query_row(
                "SELECT MAX(chapter) FROM verses WHERE bible_id = ?1 AND book_id = ?2",
                params![bible_id, book_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(u16::try_from(count.unwrap_or(0)).unwrap_or(u16::MAX))
    })
}

#[cfg_attr(feature = "sismo", sismo::function("bible::db_load_chapter"))]
pub(crate) fn load_chapter(
    db: &Db,
    bible_id: &str,
    book_id: &str,
    chapter: u16,
) -> Result<ChapterRow> {
    db.with_conn(|conn| {
        let book_name: String = {
            #[cfg(feature = "sismo")]
            sismo::scope!("bible::db_chapter_book");
            conn.query_row(
                "SELECT name FROM books WHERE bible_id = ?1 AND book_id = ?2",
                params![bible_id, book_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| DbError::MissingBible {
                id: bible_id.to_string(),
            })?
        };

        let verses = {
            #[cfg(feature = "sismo")]
            sismo::scope!("bible::db_chapter_verses");
            let mut verses_stmt = conn.prepare_cached(
                "SELECT verse, text, paragraph_break
                 FROM verses
                 WHERE bible_id = ?1 AND book_id = ?2 AND chapter = ?3
                 ORDER BY verse",
            )?;
            verses_stmt
                .query_map(params![bible_id, book_id, i64::from(chapter)], |r| {
                    Ok(VerseRow {
                        verse: r.get::<_, u16>(0)?,
                        text: r.get(1)?,
                        paragraph_break: r.get::<_, i64>(2)? != 0,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };

        let headers = {
            #[cfg(feature = "sismo")]
            sismo::scope!("bible::db_chapter_headers");
            let mut headers_stmt = conn.prepare_cached(
                "SELECT before_verse, text
                 FROM headers
                 WHERE bible_id = ?1 AND book_id = ?2 AND chapter = ?3
                 ORDER BY before_verse",
            )?;
            headers_stmt
                .query_map(params![bible_id, book_id, i64::from(chapter)], |r| {
                    Ok(HeaderRow {
                        before_verse: r.get::<_, u16>(0)?,
                        text: r.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        #[cfg(feature = "sismo")]
        sismo::counter!("bible.rows_returned", verses.len() + headers.len());

        Ok(ChapterRow {
            bible_id: bible_id.to_string(),
            book_id: book_id.to_string(),
            book_name,
            chapter,
            verses,
            headers,
        })
    })
}
