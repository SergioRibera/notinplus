//! Row shapes + non-FTS queries (book list, chapter load, etc.).

use notin_sdk::ConfigValue;
use rusqlite::{OptionalExtension, params};

use super::{Db, DbError, Result};

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BibleRow {
    pub id: String,
    pub abbrev: String,
    pub name_local: String,
    pub language: String,
}

impl BibleRow {
    /// The name the interface calls the translation: its local name,
    /// else its abbreviation (which falls back to the id).
    #[must_use]
    pub fn display_name(&self) -> &str {
        if self.name_local.is_empty() {
            &self.abbrev
        } else {
            &self.name_local
        }
    }
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

/// Every persisted plugin setting, bincoded [`ConfigValue`]s by key.
/// A value that no longer decodes is dropped: the schema's default
/// takes its place.
#[cfg_attr(feature = "sismo", sismo::function("bible::db_load_settings"))]
pub(crate) fn load_settings(db: &Db) -> Result<Vec<(String, ConfigValue)>> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare_cached("SELECT key, value FROM plugin_settings")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(key, bytes)| {
                match bincode::decode_from_slice::<ConfigValue, _>(&bytes, notin_sdk::WIRE) {
                    Ok((value, _)) => Some((key, value)),
                    Err(e) => {
                        log::warn!("bible: setting {key} unreadable, dropped: {e}");
                        None
                    }
                }
            })
            .collect())
    })
}

/// Persist `value` under `key`, replacing the previous one.
#[cfg_attr(feature = "sismo", sismo::function("bible::db_save_setting"))]
pub(crate) fn save_setting(db: &Db, key: &str, value: &ConfigValue) -> Result<()> {
    let bytes = bincode::encode_to_vec(value, bincode::config::standard())
        .map_err(|e| DbError::Encode(e.to_string()))?;
    db.with_conn_mut(|conn| {
        conn.execute(
            "INSERT INTO plugin_settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, bytes],
        )?;
        Ok(())
    })
}

/// The value stored under `key` in `meta`, if any.
pub(crate) fn meta_get(db: &Db, key: &str) -> Result<Option<String>> {
    db.with_conn(|conn| {
        let value = conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get::<_, String>(0)
            })
            .optional()?;
        Ok(value)
    })
}

/// Store `value` under `key` in `meta`, replacing the previous one.
pub(crate) fn meta_set(db: &Db, key: &str, value: &str) -> Result<()> {
    db.with_conn_mut(|conn| {
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    })
}

/// One verse of a translation, for the aligner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerseTextRow {
    pub book_id: String,
    pub chapter: u16,
    pub verse: u16,
    pub text: String,
}

/// Which translation tokens render the original word at `position`:
/// the span `start..end` of the verse's tokens.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkRow {
    pub position: u16,
    pub start: u16,
    pub end: u16,
    pub prob: f32,
}

/// One way a translation renders a lemma, with how often.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderingRow {
    pub token: String,
    pub count: u32,
    pub prob: f32,
}

/// A link with its place, as the aligner produces them.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedLink {
    pub book: u8,
    pub chapter: u16,
    pub verse: u16,
    pub link: LinkRow,
}

/// Every verse of `bible_id` in canon order.
#[cfg_attr(feature = "sismo", sismo::function("bible::db_verses_of_bible"))]
pub(crate) fn verses_of_bible(db: &Db, bible_id: &str) -> Result<Vec<VerseTextRow>> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare_cached(
            "SELECT v.book_id, v.chapter, v.verse, v.text
             FROM verses v
             JOIN books b ON b.bible_id = v.bible_id AND b.book_id = v.book_id
             WHERE v.bible_id = ?1
             ORDER BY b.order_num, v.chapter, v.verse",
        )?;
        let rows = stmt
            .query_map(params![bible_id], |r| {
                Ok(VerseTextRow {
                    book_id: r.get(0)?,
                    chapter: r.get::<_, u16>(1)?,
                    verse: r.get::<_, u16>(2)?,
                    text: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// Replace the alignment of `bible_id` for `testament` (0 Old, 1 New)
/// in one transaction.
#[cfg_attr(feature = "sismo", sismo::function("bible::db_replace_alignment"))]
pub(crate) fn replace_alignment(
    db: &Db,
    bible_id: &str,
    testament: u8,
    links: &[PlacedLink],
    renderings: &[(u32, RenderingRow)],
) -> Result<()> {
    db.with_conn_mut(|conn| {
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM orig_links WHERE bible_id = ?1 AND testament = ?2",
            params![bible_id, i64::from(testament)],
        )?;
        tx.execute(
            "DELETE FROM orig_renderings WHERE bible_id = ?1 AND testament = ?2",
            params![bible_id, i64::from(testament)],
        )?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR REPLACE INTO orig_links
                 (bible_id, testament, book, chapter, verse, position, start, end, prob)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )?;
            for placed in links {
                insert.execute(params![
                    bible_id,
                    i64::from(testament),
                    i64::from(placed.book),
                    i64::from(placed.chapter),
                    i64::from(placed.verse),
                    i64::from(placed.link.position),
                    i64::from(placed.link.start),
                    i64::from(placed.link.end),
                    f64::from(placed.link.prob),
                ])?;
            }
            let mut insert = tx.prepare_cached(
                "INSERT OR REPLACE INTO orig_renderings
                 (bible_id, testament, lemma, token, count, prob)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for (lemma, rendering) in renderings {
                insert.execute(params![
                    bible_id,
                    i64::from(testament),
                    i64::from(*lemma),
                    rendering.token,
                    i64::from(rendering.count),
                    f64::from(rendering.prob),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    })
}

/// The links of one verse, by position.
#[cfg_attr(feature = "sismo", sismo::function("bible::db_alignment_links"))]
pub(crate) fn alignment_links(
    db: &Db,
    bible_id: &str,
    testament: u8,
    book: u8,
    chapter: u16,
    verse: u16,
) -> Result<Vec<LinkRow>> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare_cached(
            "SELECT position, start, end, prob FROM orig_links
             WHERE bible_id = ?1 AND testament = ?2 AND book = ?3 AND chapter = ?4 AND verse = ?5
             ORDER BY position, start",
        )?;
        let rows = stmt
            .query_map(
                params![
                    bible_id,
                    i64::from(testament),
                    i64::from(book),
                    i64::from(chapter),
                    i64::from(verse)
                ],
                |r| {
                    Ok(LinkRow {
                        position: r.get::<_, u16>(0)?,
                        start: r.get::<_, u16>(1)?,
                        end: r.get::<_, u16>(2)?,
                        #[allow(clippy::cast_possible_truncation)] // stored from f32
                        prob: r.get::<_, f64>(3)? as f32,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}

/// How `bible_id` renders `lemma`, most frequent first.
#[cfg_attr(feature = "sismo", sismo::function("bible::db_renderings"))]
pub(crate) fn renderings(
    db: &Db,
    bible_id: &str,
    testament: u8,
    lemma: u32,
) -> Result<Vec<RenderingRow>> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare_cached(
            "SELECT token, count, prob FROM orig_renderings
             WHERE bible_id = ?1 AND testament = ?2 AND lemma = ?3
             ORDER BY count DESC, token",
        )?;
        let rows = stmt
            .query_map(
                params![bible_id, i64::from(testament), i64::from(lemma)],
                |r| {
                    Ok(RenderingRow {
                        token: r.get(0)?,
                        count: r.get::<_, u32>(1)?,
                        #[allow(clippy::cast_possible_truncation)] // stored from f32
                        prob: r.get::<_, f64>(2)? as f32,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}
