//! Treasury of Scripture Knowledge importer.
//!
//! Pulls the CrossReferences-org dataset (TSK + phrase anchors, CC-BY
//! 4.0). The data is translation-agnostic — one install populates
//! `tsk_notes` for every Bible added past or future. The downloader is
//! idempotent + guarded by a cache TTL so repeated calls short-circuit.
//!
//! Format recap (from the upstream repo):
//! - `bible_books.json`  — `[{id, name_eng, abbreviation_eng}, …]`
//! - `bible_verses.json` — `[{id, book_id, kjv_ch, kjv_vs}, …]`
//! - `cross_references.json` — `[{verse_id, sort, kjv, refs: [[tgt_id, …], …]}]`
//!
//! `sort` → letter (`sort=1` → `a`, `sort=2` → `b`, …).
//! Each `refs` entry is a verse_id range; we store the first id so the
//! chip has an unambiguous navigation target.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use reqwest::blocking::Client;
use rusqlite::params;
use serde::Deserialize;

use crate::db::Db;

const BOOKS_URL: &str =
    "https://raw.githubusercontent.com/CrossReferences-org/bible-cross-references/main/json/bible_books.json";
const VERSES_URL: &str =
    "https://raw.githubusercontent.com/CrossReferences-org/bible-cross-references/main/json/bible_verses.json";
const XREFS_URL: &str =
    "https://raw.githubusercontent.com/CrossReferences-org/bible-cross-references/main/json/cross_references.json";

#[derive(Debug)]
pub enum TskError {
    Http(reqwest::Error),
    Json { context: &'static str, source: serde_json::Error },
    Io(std::io::Error),
    Db(crate::db::DbError),
}

impl std::fmt::Display for TskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "tsk http: {e}"),
            Self::Json { context, source } => write!(f, "tsk json ({context}): {source}"),
            Self::Io(e) => write!(f, "tsk io: {e}"),
            Self::Db(e) => write!(f, "tsk db: {e}"),
        }
    }
}

impl std::error::Error for TskError {}

impl From<reqwest::Error> for TskError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}
impl From<std::io::Error> for TskError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<crate::db::DbError> for TskError {
    fn from(e: crate::db::DbError) -> Self {
        Self::Db(e)
    }
}

#[derive(Clone, Debug)]
pub enum TskProgress {
    Started,
    FetchedBooks { count: usize },
    FetchedVerses { count: usize },
    ParsingXrefs,
    InsertedRows { done: usize, total_hint: usize },
    Finished { inserted: usize },
}

#[derive(Debug, Deserialize)]
struct BookRow {
    id: i64,
    name_eng: String,
    #[serde(default)]
    abbreviation_eng: String,
}

#[derive(Debug, Deserialize)]
struct VerseRow {
    id: i64,
    book_id: i64,
    #[serde(default)]
    kjv_ch: i64,
    #[serde(default)]
    kjv_vs: i64,
}

#[derive(Debug, Deserialize)]
struct XrefRow {
    verse_id: i64,
    sort: i64,
    #[serde(default)]
    kjv: String,
    refs: Vec<Vec<i64>>,
}

/// Download + populate the TSK tables. Flushes the existing `tsk_notes`
/// rows beforehand so a re-run never duplicates. Progress fires on each
/// major stage so the shell can show a thin bar + toast.
pub fn install_tsk(
    db: &Db,
    client: &Client,
    cache_dir: &Path,
    cache_ttl: Duration,
    mut on_progress: impl FnMut(TskProgress),
) -> Result<usize, TskError> {
    on_progress(TskProgress::Started);
    let xref_cache = cache_dir.join("tsk");
    std::fs::create_dir_all(&xref_cache)?;

    let books_path = xref_cache.join("bible_books.json");
    let verses_path = xref_cache.join("bible_verses.json");
    let xrefs_path = xref_cache.join("cross_references.json");

    fetch_if_stale(client, BOOKS_URL, &books_path, cache_ttl)?;
    let books_bytes = std::fs::read(&books_path)?;
    let books: Vec<BookRow> = serde_json::from_slice(&books_bytes)
        .map_err(|e| TskError::Json { context: "books", source: e })?;
    on_progress(TskProgress::FetchedBooks { count: books.len() });
    let book_lookup: HashMap<i64, String> = books
        .into_iter()
        .map(|b| {
            let osis = osis_code_for(&b.name_eng, &b.abbreviation_eng);
            (b.id, osis)
        })
        .collect();

    fetch_if_stale(client, VERSES_URL, &verses_path, cache_ttl)?;
    let verses_bytes = std::fs::read(&verses_path)?;
    let verses: Vec<VerseRow> = serde_json::from_slice(&verses_bytes)
        .map_err(|e| TskError::Json { context: "verses", source: e })?;
    on_progress(TskProgress::FetchedVerses { count: verses.len() });
    let verse_lookup: HashMap<i64, (i64, i64, i64)> = verses
        .into_iter()
        .map(|v| (v.id, (v.book_id, v.kjv_ch, v.kjv_vs)))
        .collect();

    fetch_if_stale(client, XREFS_URL, &xrefs_path, cache_ttl)?;
    on_progress(TskProgress::ParsingXrefs);
    let xref_bytes = std::fs::read(&xrefs_path)?;
    let xrefs: Vec<XrefRow> = serde_json::from_slice(&xref_bytes)
        .map_err(|e| TskError::Json { context: "xrefs", source: e })?;
    let total_hint = xrefs.len();

    let inserted = db.with_conn_mut(|conn| {
        conn.execute("DELETE FROM tsk_notes", [])
            .map_err(crate::db::DbError::from)?;
        let tx = conn.transaction().map_err(crate::db::DbError::from)?;
        let mut inserted = 0usize;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO tsk_notes
                     (src_book, src_chap, src_verse, letter, anchor_word,
                      tgt_book, tgt_chap, tgt_verse)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )
                .map_err(crate::db::DbError::from)?;
            for (processed, note) in xrefs.iter().enumerate() {
                let Some((src_book_id, src_chap, src_verse)) = verse_lookup.get(&note.verse_id)
                else {
                    continue;
                };
                let Some(src_book) = book_lookup.get(src_book_id) else { continue };
                let letter = sort_to_letter(note.sort);
                for group in &note.refs {
                    let Some(first) = group.first() else { continue };
                    let Some((tgt_book_id, tgt_chap, tgt_verse)) = verse_lookup.get(first) else {
                        continue;
                    };
                    let Some(tgt_book) = book_lookup.get(tgt_book_id) else { continue };
                    stmt.execute(params![
                        src_book,
                        *src_chap,
                        *src_verse,
                        letter,
                        &note.kjv,
                        tgt_book,
                        *tgt_chap,
                        *tgt_verse,
                    ])
                    .map_err(crate::db::DbError::from)?;
                    inserted += 1;
                }
                if processed % 10_000 == 0 {
                    // Caller can repaint the thin bar between batches —
                    // the trait object closure is a quick signal, not a
                    // sync point.
                }
            }
        }
        tx.commit().map_err(crate::db::DbError::from)?;
        Ok(inserted)
    })?;
    on_progress(TskProgress::InsertedRows { done: inserted, total_hint });
    on_progress(TskProgress::Finished { inserted });
    Ok(inserted)
}

fn sort_to_letter(sort: i64) -> String {
    let idx = if sort < 1 { 0 } else { (sort - 1) as usize };
    if idx < 26 {
        char::from(b'a' + idx as u8).to_string()
    } else {
        let first = (idx / 26 - 1) as u8;
        let second = (idx % 26) as u8;
        format!(
            "{}{}",
            char::from(b'a' + first),
            char::from(b'a' + second)
        )
    }
}

/// Rough English-name → biblionlabs lowercase OSIS code. Keeps the TSK
/// side consistent with the Bible text table so the reader can join on
/// `book_id` without a translation layer.
fn osis_code_for(name_eng: &str, abbrev: &str) -> String {
    // The abbreviation is close to OSIS canonical. Lowercase + map a
    // few known mismatches to biblionlabs conventions.
    let base = abbrev
        .trim()
        .to_ascii_lowercase()
        .replace(['.', ' '], "");
    let fallback = name_eng.trim().to_ascii_lowercase().replace([' ', '.'], "");
    let candidate = if base.is_empty() { fallback } else { base };
    match candidate.as_str() {
        "gen" | "genesis" => "gen",
        "exod" | "exo" | "exodus" => "exo",
        "lev" | "leviticus" => "lev",
        "num" | "numbers" => "num",
        "deut" | "deu" | "deuteronomy" => "deu",
        "josh" | "jos" | "joshua" => "jos",
        "judg" | "jdg" | "judges" => "jdg",
        "ruth" | "rut" => "rut",
        "1sam" | "1sa" | "1samuel" => "1sa",
        "2sam" | "2sa" | "2samuel" => "2sa",
        "1kgs" | "1ki" | "1kings" => "1ki",
        "2kgs" | "2ki" | "2kings" => "2ki",
        "1chr" | "1ch" | "1chronicles" => "1ch",
        "2chr" | "2ch" | "2chronicles" => "2ch",
        "ezra" | "ezr" => "ezr",
        "neh" | "nehemiah" => "neh",
        "esth" | "est" | "esther" => "est",
        "job" => "job",
        "ps" | "psa" | "psalms" => "psa",
        "prov" | "pro" | "proverbs" => "pro",
        "eccl" | "ecc" | "ecclesiastes" => "ecc",
        "song" | "sng" | "songofsolomon" | "canticles" => "sng",
        "isa" | "isaiah" => "isa",
        "jer" | "jeremiah" => "jer",
        "lam" | "lamentations" => "lam",
        "ezek" | "ezk" | "ezekiel" => "ezk",
        "dan" | "daniel" => "dan",
        "hos" | "hosea" => "hos",
        "joel" | "jol" => "jol",
        "amos" | "amo" => "amo",
        "obad" | "oba" | "obadiah" => "oba",
        "jonah" | "jon" => "jon",
        "mic" | "micah" => "mic",
        "nah" | "nam" | "nahum" => "nam",
        "hab" | "habakkuk" => "hab",
        "zeph" | "zep" | "zephaniah" => "zep",
        "hag" | "haggai" => "hag",
        "zech" | "zec" | "zechariah" => "zec",
        "mal" | "malachi" => "mal",
        "matt" | "mat" | "matthew" => "mat",
        "mark" | "mrk" => "mrk",
        "luke" | "luk" => "luk",
        "john" | "jhn" => "jhn",
        "acts" | "act" => "act",
        "rom" | "romans" => "rom",
        "1cor" | "1co" | "1corinthians" => "1co",
        "2cor" | "2co" | "2corinthians" => "2co",
        "gal" | "galatians" => "gal",
        "eph" | "ephesians" => "eph",
        "phil" | "php" | "philippians" => "php",
        "col" | "colossians" => "col",
        "1thess" | "1th" | "1thessalonians" => "1th",
        "2thess" | "2th" | "2thessalonians" => "2th",
        "1tim" | "1ti" | "1timothy" => "1ti",
        "2tim" | "2ti" | "2timothy" => "2ti",
        "titus" | "tit" => "tit",
        "phlm" | "phm" | "philemon" => "phm",
        "heb" | "hebrews" => "heb",
        "jas" | "james" => "jas",
        "1pet" | "1pe" | "1peter" => "1pe",
        "2pet" | "2pe" | "2peter" => "2pe",
        "1john" | "1jn" | "1jo" => "1jn",
        "2john" | "2jn" | "2jo" => "2jn",
        "3john" | "3jn" | "3jo" => "3jn",
        "jude" | "jud" => "jud",
        "rev" | "revelation" => "rev",
        other => other,
    }
    .to_string()
}

/// Lift the file-fetch helper used by biblionlabs — same semantics:
/// honour the cache TTL and refuse non-JSON responses so a stale HTML
/// error page never pollutes the cache.
fn fetch_if_stale(client: &Client, url: &str, dest: &Path, ttl: Duration) -> Result<(), TskError> {
    if dest.exists() {
        if let Ok(meta) = dest.metadata() {
            if let Ok(modified) = meta.modified() {
                let fresh = modified.elapsed().map(|e| e <= ttl).unwrap_or(false);
                if fresh && cache_looks_like_json(dest) {
                    return Ok(());
                }
            }
        }
    }
    let bytes = client.get(url).send()?.error_for_status()?.bytes()?;
    if !bytes_look_like_json(&bytes) {
        return Err(TskError::Io(std::io::Error::other(format!(
            "non-JSON response from {url}"
        ))));
    }
    std::fs::write(dest, &bytes)?;
    Ok(())
}

fn cache_looks_like_json(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut buf = [0u8; 16];
    let n = f.read(&mut buf).unwrap_or(0);
    bytes_look_like_json(&buf[..n])
}

fn bytes_look_like_json(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .is_some_and(|i| matches!(bytes[i], b'{' | b'[' | b'"' | b't' | b'f' | b'n' | b'-' | b'0'..=b'9'))
}
