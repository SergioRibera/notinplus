//! FTS5-backed full-text search over `verses`.
//!
//! What the user types is turned into FTS5 syntax by [`sanitise`]: words
//! match as prefixes, `"…"` is a phrase, `OR` an alternative and `-word`
//! an exclusion. Accents are ignored by the `unicode61` tokenizer.
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
        let sql_all = r"
            SELECT bible_id, book_id, chapter, verse,
                   snippet(verses_fts, 0, ?1, ?2, '…', 32) AS snip,
                   bm25(verses_fts) AS score
            FROM verses_fts
            WHERE verses_fts MATCH ?3
            ORDER BY score ASC
            LIMIT ?4
        ";
        let sql_scoped = r"
            SELECT bible_id, book_id, chapter, verse,
                   snippet(verses_fts, 0, ?1, ?2, '…', 32) AS snip,
                   bm25(verses_fts) AS score
            FROM verses_fts
            WHERE verses_fts MATCH ?3 AND bible_id = ?5
            ORDER BY score ASC
            LIMIT ?4
        ";

        let limit_i = i64::try_from(limit).unwrap_or(i64::MAX);
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
        chapter: r.get::<_, u16>(2)?,
        verse: r.get::<_, u16>(3)?,
        snippet: r.get(4)?,
        score: r.get(5)?,
    })
}

/// One piece of what the user typed.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Term {
    /// A word, matched as the start of any word (`amor` finds `amorosa`).
    Word(String),
    /// Words in quotes, matched together and in order.
    Phrase(String),
    /// `OR` between two terms: either one.
    Or,
    /// A term after `-`: verses without it.
    Not(String),
}

/// The FTS5 query for what the user typed. Words match as prefixes and
/// all must appear; `"…"` is an exact phrase, `OR` (or `|`) offers an
/// alternative and `-word` leaves verses with it out. Every word is
/// quoted, so punctuation never reaches FTS5 as syntax.
fn sanitise(raw: &str) -> String {
    let mut terms = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find('"') {
        words(&rest[..start], &mut terms);
        let after = &rest[start + 1..];
        let (inside, next) = after.split_once('"').unwrap_or((after, ""));
        let phrase = clean(inside);
        if !phrase.is_empty() {
            terms.push(Term::Phrase(phrase));
        }
        rest = next;
    }
    words(rest, &mut terms);
    render(&terms)
}

/// Split loose text into [`Term`]s.
fn words(text: &str, terms: &mut Vec<Term>) {
    for raw in text.split_whitespace() {
        if raw == "OR" || raw == "|" {
            terms.push(Term::Or);
            continue;
        }
        let (negated, raw) = raw.strip_prefix('-').map_or((false, raw), |w| (true, w));
        for word in clean(raw).split_whitespace() {
            terms.push(if negated {
                Term::Not(word.to_owned())
            } else {
                Term::Word(word.to_owned())
            });
        }
    }
}

/// `text` with only letters, digits and single spaces.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Join `terms` into FTS5 syntax. An `OR` with nothing on one side and
/// exclusions with nothing to exclude from are dropped.
fn render(terms: &[Term]) -> String {
    let mut positive: Vec<String> = Vec::new();
    let mut excluded: Vec<String> = Vec::new();
    let mut pending_or = false;
    for term in terms {
        let piece = match term {
            Term::Or => {
                pending_or = !positive.is_empty();
                continue;
            }
            Term::Not(word) => {
                excluded.push(format!("\"{word}\"*"));
                continue;
            }
            Term::Word(word) => format!("\"{word}\"*"),
            Term::Phrase(phrase) => format!("\"{phrase}\""),
        };
        match (pending_or, positive.last_mut()) {
            (true, Some(last)) => *last = format!("({last} OR {piece})"),
            _ => positive.push(piece),
        }
        pending_or = false;
    }
    if positive.is_empty() {
        return String::new();
    }
    let mut query = positive.join(" AND ");
    for word in excluded {
        query = format!("{query} NOT {word}");
    }
    query
}

#[cfg(test)]
mod tests {
    use super::sanitise;

    #[test]
    fn words_match_as_prefixes_and_all_must_appear() {
        assert_eq!(sanitise("amor  Dios"), r#""amor"* AND "Dios"*"#);
    }

    #[test]
    fn quotes_keep_a_phrase_together() {
        assert_eq!(
            sanitise(r#"el "buen pastor""#),
            r#""el"* AND "buen pastor""#
        );
    }

    #[test]
    fn or_and_minus_become_operators() {
        assert_eq!(
            sanitise("fe OR esperanza -muerte"),
            r#"("fe"* OR "esperanza"*) NOT "muerte"*"#
        );
    }

    #[test]
    fn punctuation_never_reaches_fts() {
        assert_eq!(sanitise("¿quién? don't"), r#""quién"* AND "don"* AND "t"*"#);
        assert_eq!(sanitise(r#"OR -x " ""#), "");
    }
}
