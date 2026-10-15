//! FTS5-backed full-text search over `verses`.
//!
//! Query syntax matches FTS5's grammar (`word`, `"exact phrase"`, `word
//! NEAR word`, `col:word`). The UI sanitises raw user input before
//! handing it off — bare punctuation causes FTS5 to error.
//!
//! The `snippet()` wrapper produces inline markers around matches so
//! the result list can bold the hit without re-tokenising on the UI
//! side.

use rusqlite::params;

use super::{Db, Result};

/// Markers wrapped around every FTS5 match inside `SearchHit::snippet`.
/// Picked from the private-use Unicode block so they never collide with
/// real Bible text.
pub(crate) const SNIPPET_OPEN: &str = "\u{E000}";
pub(crate) const SNIPPET_CLOSE: &str = "\u{E001}";

#[must_use]
pub const fn snippet_markers() -> (&'static str, &'static str) {
    (SNIPPET_OPEN, SNIPPET_CLOSE)
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub bible_id: String,
    pub book_id: String,
    pub chapter: u16,
    pub verse: u16,
    /// Verse text with every match wrapped in [`SNIPPET_OPEN`] /
    /// [`SNIPPET_CLOSE`]. The UI splits on the markers to render bold
    /// runs without doing its own tokenisation.
    pub snippet: String,
    /// Relevance score — lower is better in FTS5 BM25 land.
    pub score: f64,
}

pub(crate) fn search(
    db: &Db,
    bible_id: Option<&str>,
    query: &str,
    limit: usize,
) -> Result<Vec<SearchHit>> {
    let query = sanitise(query);
    if query.is_empty() {
        return Ok(Vec::new());
    }

    db.with_conn(|conn| {
        // snippet(table, col_index, open, close, ellipsis, max_tokens)
        // bm25(table) ascends — ORDER BY bm25 ASC puts best matches first.
        let sql_all = r#"
            SELECT bible_id, book_id, chapter, verse,
                   snippet(verses_fts, 0, ?1, ?2, '…', 32) AS snip,
                   bm25(verses_fts) AS score
            FROM verses_fts
            WHERE verses_fts MATCH ?3
            ORDER BY score ASC
            LIMIT ?4
        "#;
        let sql_scoped = r#"
            SELECT bible_id, book_id, chapter, verse,
                   snippet(verses_fts, 0, ?1, ?2, '…', 32) AS snip,
                   bm25(verses_fts) AS score
            FROM verses_fts
            WHERE verses_fts MATCH ?3 AND bible_id = ?5
            ORDER BY score ASC
            LIMIT ?4
        "#;

        let limit_i = limit as i64;
        let rows: Vec<SearchHit> = if let Some(bid) = bible_id {
            let mut stmt = conn.prepare(sql_scoped)?;
            stmt.query_map(
                params![SNIPPET_OPEN, SNIPPET_CLOSE, &query, limit_i, bid],
                map_hit,
            )?
            .collect::<rusqlite::Result<_>>()?
        } else {
            let mut stmt = conn.prepare(sql_all)?;
            stmt.query_map(
                params![SNIPPET_OPEN, SNIPPET_CLOSE, &query, limit_i],
                map_hit,
            )?
            .collect::<rusqlite::Result<_>>()?
        };
        Ok(rows)
    })
}

fn map_hit(r: &rusqlite::Row<'_>) -> rusqlite::Result<SearchHit> {
    Ok(SearchHit {
        bible_id: r.get(0)?,
        book_id: r.get(1)?,
        chapter: r.get::<_, i64>(2)? as u16,
        verse: r.get::<_, i64>(3)? as u16,
        snippet: r.get(4)?,
        score: r.get(5)?,
    })
}

/// Strip FTS5-breaking punctuation + collapse whitespace. Keeps word
/// chars, spaces, quotes and the explicit operators (`AND`, `OR`,
/// `NOT`, `NEAR`) the UI exposes.
fn sanitise(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_alphanumeric() || ch == ' ' || ch == '"' || ch == '-' || ch == '\'' {
            out.push(ch);
        } else {
            out.push(' ');
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
