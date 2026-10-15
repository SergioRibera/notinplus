//! Highlight CRUD. Mirrors `BibleBookmark` on the state side — this
//! module is the authoritative persisted copy; `BibleViewerState` just
//! caches the subset for the current chapter.

use notein_sdk::BookmarkId;
use rusqlite::params;

use super::{Db, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightRow {
    pub id: BookmarkId,
    pub bible_id: String,
    pub book_id: String,
    pub chapter: u16,
    pub start_verse: u16,
    pub end_verse: u16,
    pub color: Option<u32>,
    pub note: String,
    pub created_at: i64,
}

pub(crate) fn insert_highlight(db: &Db, row: &HighlightRow) -> Result<()> {
    db.with_conn(|conn| {
        conn.execute(
            "INSERT INTO highlights
             (id, bible_id, book_id, chapter, start_verse, end_verse, color, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                 bible_id = excluded.bible_id,
                 book_id = excluded.book_id,
                 chapter = excluded.chapter,
                 start_verse = excluded.start_verse,
                 end_verse = excluded.end_verse,
                 color = excluded.color,
                 note = excluded.note",
            params![
                row.id.0.as_slice(),
                row.bible_id,
                row.book_id,
                row.chapter as i64,
                row.start_verse as i64,
                row.end_verse as i64,
                row.color.map(|c| c as i64),
                row.note,
                row.created_at,
            ],
        )?;
        Ok(())
    })
}

pub(crate) fn delete_highlight(db: &Db, id: BookmarkId) -> Result<bool> {
    db.with_conn(|conn| {
        let n = conn.execute("DELETE FROM highlights WHERE id = ?1", params![id.0.as_slice()])?;
        Ok(n > 0)
    })
}

pub(crate) fn list_highlights(db: &Db, bible_id: &str) -> Result<Vec<HighlightRow>> {
    db.with_conn(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, bible_id, book_id, chapter, start_verse, end_verse, color, note, created_at
             FROM highlights
             WHERE bible_id = ?1
             ORDER BY book_id, chapter, start_verse",
        )?;
        let rows = stmt
            .query_map(params![bible_id], |r| {
                let id_bytes: Vec<u8> = r.get(0)?;
                let mut id_arr = [0u8; 16];
                if id_bytes.len() == 16 {
                    id_arr.copy_from_slice(&id_bytes);
                }
                Ok(HighlightRow {
                    id: BookmarkId(id_arr),
                    bible_id: r.get(1)?,
                    book_id: r.get(2)?,
                    chapter: r.get::<_, i64>(3)? as u16,
                    start_verse: r.get::<_, i64>(4)? as u16,
                    end_verse: r.get::<_, i64>(5)? as u16,
                    color: r.get::<_, Option<i64>>(6)?.map(|v| v as u32),
                    note: r.get(7)?,
                    created_at: r.get(8)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
}
