//! helloao.org footnote fetcher.
//!
//! helloao serves per-chapter JSON with publisher-authored footnotes
//! (caller + text + verse reference). The upstream dataset is
//! translation-specific, so we map biblionlabs ids → helloao ids via
//! [`helloao_id_for_bible`]. Pull is lazy — the reader triggers one
//! fetch per `(bible, book, chapter)` as the user opens it, cached on
//! disk + indexed in the local `footnotes` table so subsequent opens
//! short-circuit.
//!
//! Many translations ship empty footnote arrays (public-domain Spanish
//! Bibles) — in that case the chapter row count stays `0` and the
//! reader simply shows no inline letters. BSB (English) is the richest
//! source with "Cited in Scripture:Reference" style apparatus.

use std::path::Path;
use std::time::Duration;

use reqwest::blocking::Client;
use rusqlite::params;
use serde::Deserialize;

use crate::db::Db;

/// Base API endpoint. Configurable only via source edit for now; the
/// upstream project does not publish a stable mirror story.
const HELLOAO_BASE: &str = "https://bible.helloao.org/api";

#[derive(Debug)]
pub enum HelloaoError {
    Http(reqwest::Error),
    Json {
        context: String,
        source: serde_json::Error,
    },
    Io(std::io::Error),
    Db(crate::db::DbError),
    Unmapped {
        bible_id: String,
    },
}

impl std::fmt::Display for HelloaoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "helloao http: {e}"),
            Self::Json { context, source } => write!(f, "helloao json ({context}): {source}"),
            Self::Io(e) => write!(f, "helloao io: {e}"),
            Self::Db(e) => write!(f, "helloao db: {e}"),
            Self::Unmapped { bible_id } => write!(f, "no helloao mapping for bible {bible_id}"),
        }
    }
}

impl std::error::Error for HelloaoError {}

impl From<reqwest::Error> for HelloaoError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}
impl From<std::io::Error> for HelloaoError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<crate::db::DbError> for HelloaoError {
    fn from(e: crate::db::DbError) -> Self {
        Self::Db(e)
    }
}

#[derive(Debug, Deserialize)]
struct ChapterEnvelope {
    #[serde(default)]
    chapter: ChapterPayload,
}

#[derive(Debug, Default, Deserialize)]
struct ChapterPayload {
    #[serde(default)]
    footnotes: Vec<FootnoteEntry>,
    #[serde(default)]
    content: Vec<ContentNode>,
}

/// Items inside `chapter.content`. We only care about `heading` + the
/// `verse` number so we can emit `headers` rows at `before_verse =
/// next verse number`. Everything else (`line_break`, `hebrew_subtitle`,
/// paragraph markers, …) is ignored.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ContentNode {
    #[serde(rename = "verse")]
    Verse {
        #[serde(default)]
        number: u16,
    },
    #[serde(rename = "heading")]
    Heading {
        #[serde(default)]
        content: Vec<HeadingPiece>,
    },
    #[serde(other)]
    Other,
}

/// Headings arrive as a mix of raw strings + typed sub-items (hebrew
/// subtitles, references). The simple string form covers the ~99%
/// case; typed pieces fall through to their stringified value.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum HeadingPiece {
    Raw(String),
    Typed(serde_json::Value),
}

impl HeadingPiece {
    fn render(&self) -> String {
        match self {
            Self::Raw(s) => s.clone(),
            Self::Typed(v) => v
                .get("text")
                .and_then(|t| t.as_str())
                .map(ToOwned::to_owned)
                .unwrap_or_default(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct FootnoteEntry {
    #[serde(default)]
    #[serde(rename = "noteId")]
    note_id: i64,
    #[serde(default)]
    caller: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    reference: Option<FootnoteRef>,
}

#[derive(Debug, Deserialize)]
struct FootnoteRef {
    #[serde(default)]
    verse: i64,
}

/// `/api/<source>/complete.json`: every book of a translation.
#[derive(Debug, Deserialize)]
struct CompleteTranslation {
    #[serde(default)]
    books: Vec<CompleteBook>,
}

#[derive(Debug, Deserialize)]
struct CompleteBook {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    chapters: Vec<CompleteChapter>,
}

#[derive(Debug, Deserialize)]
struct CompleteChapter {
    #[serde(default)]
    chapter: CompleteChapterInner,
}

#[derive(Debug, Default, Deserialize)]
struct CompleteChapterInner {
    #[serde(default)]
    number: u16,
    #[serde(default)]
    content: Vec<ContentNode>,
}

/// Fetches from the helloao API into `db`, caching each response
/// under `cache_dir` for `cache_ttl`.
#[derive(Debug, Clone, Copy)]
pub struct Helloao<'a> {
    pub db: &'a Db,
    pub client: &'a Client,
    pub cache_dir: &'a Path,
    pub cache_ttl: Duration,
}

impl Helloao<'_> {
    /// Download the footnote payload for `(bible, book, chapter)` + persist
    /// into `footnotes`.
    ///
    /// Idempotent: wipes the matching rows first so a re-run (e.g. cache busted)
    /// never duplicates. `Ok(n)` reports how many rows actually landed — a
    /// chapter with no publisher apparatus returns `Ok(0)` without erroring.
    #[cfg_attr(feature = "sismo", sismo::function("bible::helloao_footnotes"))]
    pub fn footnotes_for_chapter(
        &self,
        bible_id: &str,
        book_id: &str,
        chapter: u16,
    ) -> Result<usize, HelloaoError> {
        let Some(helloao_translation) = helloao_id_for_bible(bible_id) else {
            return Err(HelloaoError::Unmapped {
                bible_id: bible_id.to_string(),
            });
        };
        let helloao_book = helloao_book_for_osis(book_id).unwrap_or_else(|| book_id.to_uppercase());

        let chapter_dir = self
            .cache_dir
            .join("helloao")
            .join(helloao_translation)
            .join(&helloao_book);
        std::fs::create_dir_all(&chapter_dir)?;
        let dest = chapter_dir.join(format!("{chapter}.json"));
        let url = format!("{HELLOAO_BASE}/{helloao_translation}/{helloao_book}/{chapter}.json");
        fetch_if_stale(self.client, &url, &dest, self.cache_ttl)?;

        let bytes = std::fs::read(&dest)?;
        let env: ChapterEnvelope =
            serde_json::from_slice(&bytes).map_err(|e| HelloaoError::Json {
                context: url.clone(),
                source: e,
            })?;

        let inserted = self.db.with_conn_mut(|conn| {
            let tx = conn.transaction().map_err(crate::db::DbError::from)?;
            tx.execute(
                "DELETE FROM footnotes
                 WHERE bible_id = ?1 AND book_id = ?2 AND chapter = ?3",
                params![bible_id, book_id, i64::from(chapter)],
            )
            .map_err(crate::db::DbError::from)?;
            let mut inserted = 0usize;
            {
                let mut stmt = tx
                    .prepare(
                        "INSERT INTO footnotes
                         (bible_id, book_id, chapter, verse, note_id, caller, text)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    )
                    .map_err(crate::db::DbError::from)?;
                for note in &env.chapter.footnotes {
                    let verse = note.reference.as_ref().map_or(0, |r| r.verse).max(0);
                    stmt.execute(params![
                        bible_id,
                        book_id,
                        i64::from(chapter),
                        verse,
                        note.note_id,
                        &note.caller,
                        &note.text,
                    ])
                    .map_err(crate::db::DbError::from)?;
                    inserted += 1;
                }
            }
            tx.commit().map_err(crate::db::DbError::from)?;
            Ok(inserted)
        })?;

        Ok(inserted)
    }

    /// Bulk variant: one HTTP request pulls the entire translation
    /// (`/api/<source>/complete.json`, ~6 MB for a Spanish Bible) and
    /// writes every `(book, chapter)` heading row in a single DB
    /// transaction per book.
    ///
    /// Replaces the per-chapter fan-out used by [`Self::headings_for_chapter`]
    /// when the whole Bible needs to be enriched at once — ~1189 requests
    /// collapse to 1.
    ///
    /// `on_book_progress(idx, total, book_display_name)` fires after each
    /// book commit so the caller can forward progress to the install
    /// modal.
    #[cfg_attr(feature = "sismo", sismo::function("bible::helloao_all_headings"))]
    pub fn all_headings(
        &self,
        bible_id: &str,
        source_translation: &str,
        mut on_book_progress: impl FnMut(usize, usize, &str),
    ) -> Result<usize, HelloaoError> {
        let bulk_dir = self.cache_dir.join("helloao").join(source_translation);
        std::fs::create_dir_all(&bulk_dir)?;
        let dest = bulk_dir.join("complete.json");
        let url = format!("{HELLOAO_BASE}/{source_translation}/complete.json");
        fetch_if_stale(self.client, &url, &dest, self.cache_ttl)?;

        let bytes = std::fs::read(&dest)?;
        let bundle: CompleteTranslation =
            serde_json::from_slice(&bytes).map_err(|e| HelloaoError::Json {
                context: url.clone(),
                source: e,
            })?;

        // The DB stores book ids lowercase (`gen`, `1co`, …) but helloao
        // emits uppercase OSIS (`GEN`, `1CO`). Normalise once here so the
        // insert matches the primary key.
        let total_books = bundle.books.len();
        let mut total_inserted = 0usize;
        for (idx, book) in bundle.books.iter().enumerate() {
            let book_id = book.id.to_lowercase();
            let display = if book.name.trim().is_empty() {
                book.id.as_str()
            } else {
                book.name.as_str()
            };
            let book_rows = self.db.with_conn_mut(|conn| {
                let tx = conn.transaction().map_err(crate::db::DbError::from)?;
                tx.execute(
                    "DELETE FROM headers WHERE bible_id = ?1 AND book_id = ?2",
                    params![bible_id, &book_id],
                )
                .map_err(crate::db::DbError::from)?;
                let mut inserted = 0usize;
                {
                    let mut stmt = tx
                        .prepare(
                            "INSERT INTO headers (bible_id, book_id, chapter, before_verse, text)
                             VALUES (?1, ?2, ?3, ?4, ?5)",
                        )
                        .map_err(crate::db::DbError::from)?;
                    for chap in &book.chapters {
                        let chapter_num = chap.chapter.number.max(1);
                        let mut next_verse: u16 = 1;
                        for node in &chap.chapter.content {
                            match node {
                                ContentNode::Verse { number } => {
                                    next_verse = (*number).max(1);
                                }
                                ContentNode::Heading { content } => {
                                    let text: String = content
                                        .iter()
                                        .map(HeadingPiece::render)
                                        .collect::<Vec<_>>()
                                        .join(" ");
                                    let text = text.trim().to_string();
                                    if text.is_empty() {
                                        continue;
                                    }
                                    stmt.execute(params![
                                        bible_id,
                                        &book_id,
                                        i64::from(chapter_num),
                                        i64::from(next_verse),
                                        text,
                                    ])
                                    .map_err(crate::db::DbError::from)?;
                                    inserted += 1;
                                }
                                ContentNode::Other => {}
                            }
                        }
                    }
                }
                tx.commit().map_err(crate::db::DbError::from)?;
                Ok(inserted)
            })?;
            total_inserted += book_rows;
            on_book_progress(idx + 1, total_books, display);
        }
        Ok(total_inserted)
    }

    /// Download the heading payload for `(bible, book, chapter)` + persist
    /// into `headers`. Idempotent: wipes the matching rows first so a
    /// re-run never duplicates. Returns the number of inserted rows.
    ///
    /// Translations without inline headings (e.g. `spa_r09` the public-
    /// domain Reina Valera 1909) return 0 silently — the caller then falls
    /// back to a sibling translation via
    /// [`helloao_headings_id_for_bible`].
    #[cfg_attr(feature = "sismo", sismo::function("bible::helloao_headings"))]
    pub fn headings_for_chapter(
        &self,
        bible_id: &str,
        source_translation: &str,
        book_id: &str,
        chapter: u16,
    ) -> Result<usize, HelloaoError> {
        let helloao_book = helloao_book_for_osis(book_id).unwrap_or_else(|| book_id.to_uppercase());
        let chapter_dir = self
            .cache_dir
            .join("helloao")
            .join(source_translation)
            .join(&helloao_book);
        std::fs::create_dir_all(&chapter_dir)?;
        let dest = chapter_dir.join(format!("{chapter}.json"));
        let url = format!("{HELLOAO_BASE}/{source_translation}/{helloao_book}/{chapter}.json");
        fetch_if_stale(self.client, &url, &dest, self.cache_ttl)?;

        let bytes = std::fs::read(&dest)?;
        let env: ChapterEnvelope =
            serde_json::from_slice(&bytes).map_err(|e| HelloaoError::Json {
                context: url.clone(),
                source: e,
            })?;

        // Walk the content array tracking the *next* verse number so
        // `heading` entries emit a `headers` row anchored to the verse
        // they precede. A heading at the very top of a chapter (before
        // any verse) uses `before_verse = 1`.
        let mut next_verse: u16 = 1;
        let mut pending: Vec<(u16, String)> = Vec::new();
        for node in &env.chapter.content {
            match node {
                ContentNode::Verse { number } => {
                    next_verse = (*number).max(1);
                }
                ContentNode::Heading { content } => {
                    let text: String = content
                        .iter()
                        .map(HeadingPiece::render)
                        .collect::<Vec<_>>()
                        .join(" ");
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        pending.push((next_verse, text));
                    }
                }
                ContentNode::Other => {}
            }
        }

        if pending.is_empty() {
            return Ok(0);
        }

        let inserted = self.db.with_conn_mut(|conn| {
            let tx = conn.transaction().map_err(crate::db::DbError::from)?;
            tx.execute(
                "DELETE FROM headers
                 WHERE bible_id = ?1 AND book_id = ?2 AND chapter = ?3",
                params![bible_id, book_id, i64::from(chapter)],
            )
            .map_err(crate::db::DbError::from)?;
            let mut inserted = 0usize;
            {
                let mut stmt = tx
                    .prepare(
                        "INSERT INTO headers (bible_id, book_id, chapter, before_verse, text)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                    )
                    .map_err(crate::db::DbError::from)?;
                for (before_verse, text) in &pending {
                    stmt.execute(params![
                        bible_id,
                        book_id,
                        i64::from(chapter),
                        i64::from(*before_verse),
                        text,
                    ])
                    .map_err(crate::db::DbError::from)?;
                    inserted += 1;
                }
            }
            tx.commit().map_err(crate::db::DbError::from)?;
            Ok(inserted)
        })?;

        Ok(inserted)
    }
}

/// Pick the best helloao translation to source section headings from
/// when the primary bible has none inline.
///
/// Spanish RV / r09 route through `spa_pdt` (Palabra de Dios para Todos) which
/// ships the standard "Pablo es enviado a Roma" style apparatus. English bibles
/// already have BSB-style headings in their own feed, so this just echoes the
/// primary id. `None` means no sensible fallback — callers skip the
/// enrichment pass.
#[must_use]
pub fn helloao_headings_id_for_bible(bible_id: &str) -> Option<&'static str> {
    match bible_id {
        "spa_rv1960" | "spa_rvg" | "spa_r09" | "spa_bes" | "spa_vbl" | "spa_blm" | "spa_v2p" => {
            Some("spa_pdt")
        }
        "spa_pdt" | "spa_onbv" => helloao_id_for_bible(bible_id),
        other => helloao_id_for_bible(other),
    }
}

/// Map a biblionlabs bible id to the closest public-domain helloao
/// translation id. Returns `None` for unknown ids so the caller can
/// skip the fetch cleanly without a cryptic 404.
#[must_use]
pub fn helloao_id_for_bible(bible_id: &str) -> Option<&'static str> {
    match bible_id {
        // Spanish — pick the public-domain Reina Valera 1909 since
        // RVR1960 is SBU copyright (not on helloao). The reader sees
        // Spanish text from biblionlabs; the footnotes side-channel
        // through r09 is close enough linguistically.
        "spa_rv1960" | "spa_rvg" | "spa_r09" => Some("spa_r09"),
        "spa_blm" => Some("spa_blm"),
        "spa_vbl" => Some("spa_vbl"),
        "spa_pdt" => Some("spa_pdt"),
        "spa_bes" => Some("spa_bes"),
        "spa_onbv" => Some("spa_onbv"),
        "spa_v2p" => Some("spa_v2p"),
        // English — common seed Bibles. BSB is the richest apparatus.
        "eng_bsb" | "bsb" | "BSB" => Some("BSB"),
        "eng_web" | "WEB" => Some("WEB"),
        "eng_asv" | "ASV" => Some("ASV"),
        "eng_kjv" | "KJV" => Some("KJV"),
        _ => None,
    }
}

/// Convert a biblionlabs lowercase OSIS book id to the helloao API's
/// uppercase form. Falls back to `None` for unmapped books so callers
/// skip + log without 404 noise.
#[must_use]
pub fn helloao_book_for_osis(book_id: &str) -> Option<String> {
    let out = match book_id.to_ascii_lowercase().as_str() {
        "gen" => "GEN",
        "exo" => "EXO",
        "lev" => "LEV",
        "num" => "NUM",
        "deu" => "DEU",
        "jos" => "JOS",
        "jdg" => "JDG",
        "rut" => "RUT",
        "1sa" => "1SA",
        "2sa" => "2SA",
        "1ki" => "1KI",
        "2ki" => "2KI",
        "1ch" => "1CH",
        "2ch" => "2CH",
        "ezr" => "EZR",
        "neh" => "NEH",
        "est" => "EST",
        "job" => "JOB",
        "psa" => "PSA",
        "pro" => "PRO",
        "ecc" => "ECC",
        "sng" => "SNG",
        "isa" => "ISA",
        "jer" => "JER",
        "lam" => "LAM",
        "ezk" => "EZK",
        "dan" => "DAN",
        "hos" => "HOS",
        "jol" => "JOL",
        "amo" => "AMO",
        "oba" => "OBA",
        "jon" => "JON",
        "mic" => "MIC",
        "nam" => "NAM",
        "hab" => "HAB",
        "zep" => "ZEP",
        "hag" => "HAG",
        "zec" => "ZEC",
        "mal" => "MAL",
        "mat" => "MAT",
        "mrk" => "MRK",
        "luk" => "LUK",
        "jhn" => "JHN",
        "act" => "ACT",
        "rom" => "ROM",
        "1co" => "1CO",
        "2co" => "2CO",
        "gal" => "GAL",
        "eph" => "EPH",
        "php" => "PHP",
        "col" => "COL",
        "1th" => "1TH",
        "2th" => "2TH",
        "1ti" => "1TI",
        "2ti" => "2TI",
        "tit" => "TIT",
        "phm" => "PHM",
        "heb" => "HEB",
        "jas" => "JAS",
        "1pe" => "1PE",
        "2pe" => "2PE",
        "1jn" => "1JN",
        "2jn" => "2JN",
        "3jn" => "3JN",
        "jud" => "JUD",
        "rev" => "REV",
        _ => return None,
    };
    Some(out.to_string())
}

#[cfg_attr(feature = "sismo", sismo::function("bible::helloao_fetch"))]
fn fetch_if_stale(
    client: &Client,
    url: &str,
    dest: &Path,
    ttl: Duration,
) -> Result<(), HelloaoError> {
    if dest.exists() {
        if let Ok(meta) = dest.metadata() {
            if let Ok(modified) = meta.modified() {
                let fresh = modified.elapsed().is_ok_and(|e| e <= ttl);
                if fresh && cache_looks_like_json(dest) {
                    #[cfg(feature = "sismo")]
                    sismo::instant!("bible::helloao_fetch_cached");
                    return Ok(());
                }
            }
        }
    }
    #[cfg(feature = "sismo")]
    sismo::scope!("bible::helloao_download");
    let response = client.get(url).send()?;
    // 404 is fine — some chapters simply have no footnotes catalogue.
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        std::fs::write(dest, b"{\"chapter\":{\"footnotes\":[]}}")?;
        return Ok(());
    }
    let response = response.error_for_status()?;
    let bytes = response.bytes()?;
    #[cfg(feature = "sismo")]
    sismo::counter!("bible.download_bytes", bytes.len());
    if !bytes_look_like_json(&bytes) {
        return Err(HelloaoError::Io(std::io::Error::other(format!(
            "non-JSON response from {url}"
        ))));
    }
    std::fs::write(dest, &bytes)?;
    Ok(())
}

fn cache_looks_like_json(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 16];
    let n = f.read(&mut buf).unwrap_or(0);
    bytes_look_like_json(&buf[..n])
}

fn bytes_look_like_json(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .is_some_and(|i| {
            matches!(
                bytes[i],
                b'{' | b'[' | b'"' | b't' | b'f' | b'n' | b'-' | b'0'..=b'9'
            )
        })
}
