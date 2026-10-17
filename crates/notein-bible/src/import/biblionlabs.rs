//! biblionlabs downloader.
//!
//! The catalogue publishes:
//!
//! - A top-level `manifest.json` listing every Bible by id.
//! - Per-Bible `extra.json` (aka "desc") carrying book names, chapter
//!   headings and section ranges.
//! - Per-book JSON files under `bibles/<id>/books/<book>.json` whose
//!   `contents` field is a 3-level nested array
//!   (`chapter → verse → content items`). Each content item is either
//!   a raw string (actual verse text), a heading, or a footnote note.
//!
//! The downloader caches each stage on disk so a re-run skips untouched
//! files; the DB population walks the cache deterministically.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::blocking::Client;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::db::Db;

/// Default endpoints used by `install_bible` when the caller doesn't
/// supply a custom source. Mirror the biblionlabs/fetch.bible layout.
pub const DEFAULT_MANIFEST_URL: &str = "https://v1.fetch.bible/manifest.json";

/// fetch.bible cross-reference bundle, per lowercase book id.
/// Shape: `{ chapter_str: { verse_str: [[tgt_book, chap, verse, …], …] } }`.
pub const CROSSREF_URL_TEMPLATE: &str =
    "https://v1.fetch.bible/crossref/large/{book}.json";

#[derive(Debug)]
pub enum ImportError {
    Http(reqwest::Error),
    Json { context: String, source: serde_json::Error },
    Io(std::io::Error),
    Db(crate::db::DbError),
    Format { reason: String },
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => write!(f, "biblionlabs http: {e}"),
            Self::Json { context, source } => write!(f, "biblionlabs json ({context}): {source}"),
            Self::Io(e) => write!(f, "biblionlabs io: {e}"),
            Self::Db(e) => write!(f, "biblionlabs db: {e}"),
            Self::Format { reason } => write!(f, "biblionlabs format: {reason}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<reqwest::Error> for ImportError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

impl From<std::io::Error> for ImportError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<crate::db::DbError> for ImportError {
    fn from(e: crate::db::DbError) -> Self {
        Self::Db(e)
    }
}

pub type Result<T> = std::result::Result<T, ImportError>;

/// Progress event emitted while [`install_bible`] runs. Hooked by the
/// translation-picker UI to drive its progress bar.
#[derive(Clone, Debug)]
pub enum Progress {
    Started { bible_id: String },
    ManifestFetched { total_books: usize },
    BookDone { book_id: String, verses: usize },
    Finished { bible_id: String, inserted_verses: usize },
}

/// Catalogue manifest entry — the top-level `manifest.json` is roughly
/// `{ "bibles": { id: BibleEntry }, "languages": { … } }`. We only
/// deserialise the fields we actually need.
#[derive(Debug, Deserialize, Serialize)]
struct Catalogue {
    #[serde(default)]
    bibles: HashMap<String, serde_json::Value>,
    #[serde(default)]
    languages: HashMap<String, LanguageEntry>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct LanguageEntry {
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    local: Option<String>,
    #[serde(default)]
    english: Option<String>,
}

/// Per-Bible description — book names + headings. Nested in
/// `BibleVariant` on the upstream service; cloned here with permissive
/// defaults so partial files don't abort the import.
#[derive(Debug, Default, Deserialize, Serialize)]
struct BibleDesc {
    #[serde(default)]
    book_names: HashMap<String, BookName>,
    #[serde(default)]
    chapter_headings: HashMap<String, Vec<String>>,
    #[serde(default)]
    sections: HashMap<String, Vec<SectionEntry>>,
    /// Optional language id — if absent we fall back to the bible_id prefix.
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    name_local: Option<String>,
    #[serde(default)]
    name_english: Option<String>,
    #[serde(default)]
    abbrev: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct BookName {
    #[serde(default)]
    normal: String,
    #[serde(default)]
    long: String,
    #[serde(default)]
    abbrev: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum SectionEntry {
    Raw(String),
    Range {
        #[serde(default)]
        start_chapter: i64,
        #[serde(default)]
        start_verse: i64,
        #[serde(default)]
        end_chapter: i64,
        #[serde(default)]
        end_verse: i64,
        #[serde(default)]
        heading: Option<String>,
    },
}

/// Per-book JSON. `contents` is a 3-level array — chapter / verse /
/// content items; the inner item is either a raw string (verse text)
/// or an object (`type: "note"` footnotes, `type: "heading"` sections).
#[derive(Debug, Deserialize, Serialize)]
struct BookDoc {
    #[serde(default)]
    book: String,
    #[serde(default)]
    name: BookName,
    #[serde(default)]
    contents: Vec<Vec<Vec<ContentItem>>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum ContentItem {
    Typed {
        r#type: String,
        #[serde(default)]
        contents: String,
    },
    Raw(String),
}

/// Metadata overrides from the top-level catalogue. The per-bible
/// biblionlabs manifest only carries `book_names` / `chapter_headings`
/// / `sections` — the user-facing name, language, and abbreviation
/// live on the catalogue entry, so the caller resolves them there and
/// hands them in here. Any field left `None` falls back to a sensible
/// default (bible_id prefix for language, bible_id itself for name /
/// abbrev).
#[derive(Debug, Default, Clone)]
pub struct BibleMeta {
    pub name_local: Option<String>,
    pub name_english: Option<String>,
    pub language: Option<String>,
    pub abbrev: Option<String>,
}

/// Download the whole catalogue entry for `bible_id`, land JSON files
/// into `cache_dir`, then populate `db`. The HTTP step is skipped per
/// file when the cache is fresher than `cache_ttl`.
pub fn install_bible(
    db: &Db,
    client: &Client,
    bible_id: &str,
    cache_dir: &Path,
    desc_url: &str,
    books_url_template: &str,
    cache_ttl: Duration,
    meta: BibleMeta,
    mut on_progress: impl FnMut(Progress),
) -> Result<usize> {
    on_progress(Progress::Started { bible_id: bible_id.into() });

    let bible_dir = cache_dir.join("bibles").join(bible_id);
    std::fs::create_dir_all(&bible_dir)?;

    let desc_path = bible_dir.join("manifest.json");
    fetch_if_stale(client, desc_url, &desc_path, cache_ttl)?;
    let desc_bytes = std::fs::read(&desc_path)?;
    let mut desc: BibleDesc = serde_json::from_slice(&desc_bytes)
        .map_err(|e| ImportError::Json { context: format!("{bible_id} desc"), source: e })?;
    // Catalogue-provided fields override the per-bible manifest when
    // the latter is silent (which is the common case on biblionlabs —
    // the per-bible manifest ships without name/language/abbrev).
    if desc.name_local.is_none() {
        desc.name_local = meta.name_local.clone().or_else(|| meta.name_english.clone());
    }
    if desc.name_english.is_none() {
        desc.name_english = meta.name_english.clone();
    }
    if desc.language.is_none() {
        desc.language = meta.language.clone();
    }
    if desc.abbrev.is_none() {
        desc.abbrev = meta.abbrev.clone();
    }

    let books_dir = bible_dir.join("books");
    std::fs::create_dir_all(&books_dir)?;

    on_progress(Progress::ManifestFetched { total_books: desc.book_names.len() });

    // Collect + sort book ids by `order_num` later; biblionlabs stores
    // them in a HashMap so iteration order is unstable, which would make
    // the install non-deterministic.
    let mut book_ids: Vec<&String> = desc.book_names.keys().collect();
    book_ids.sort();

    let mut inserted = 0usize;

    db.with_conn_mut(|conn| {
        upsert_bible(conn, bible_id, &desc).map_err(crate::db::DbError::from)?;
        let tx = conn.transaction().map_err(crate::db::DbError::from)?;
        for (order_num, book_id) in book_ids.iter().enumerate() {
            upsert_book(&tx, bible_id, book_id, &desc, order_num as u16)
                .map_err(crate::db::DbError::from)?;
        }
        tx.commit().map_err(crate::db::DbError::from)?;
        Ok(())
    })?;

    for (order_num, book_id) in book_ids.iter().enumerate() {
        let book_path = books_dir.join(format!("{book_id}.json"));
        let url = books_url_template
            .replace("{bible_id}", bible_id)
            .replace("{book}", book_id);
        if let Err(e) = fetch_if_stale(client, &url, &book_path, cache_ttl) {
            // Keep going — missing books are tolerated so a slow network
            // doesn't nuke an otherwise-complete install.
            log::warn!("biblionlabs: skipping {book_id}: {e}");
            continue;
        }

        let book_bytes = std::fs::read(&book_path)?;
        let book_doc: BookDoc = match serde_json::from_slice(&book_bytes) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("biblionlabs: {book_id} parse failed: {e}");
                continue;
            }
        };

        let book_insert_count = db.with_conn_mut(|conn| {
            let tx = conn.transaction().map_err(crate::db::DbError::from)?;
            let n = insert_book_contents(&tx, bible_id, book_id, &book_doc)
                .map_err(crate::db::DbError::from)?;
            insert_headers(&tx, bible_id, book_id, &desc)
                .map_err(crate::db::DbError::from)?;
            let _ = order_num;
            tx.commit().map_err(crate::db::DbError::from)?;
            Ok(n)
        })?;

        inserted += book_insert_count;
        on_progress(Progress::BookDone {
            book_id: (**book_id).clone(),
            verses: book_insert_count,
        });
    }

    // Cross-references land once per installed Bible. Each per-book
    // file is small (~1-5 MB total across the canon) and the upstream
    // caches at the HTTP layer, so a background re-install only pays
    // the parse cost.
    let xref_cache = cache_dir.join("crossrefs");
    std::fs::create_dir_all(&xref_cache)?;
    for book_id in &book_ids {
        let xref_path = xref_cache.join(format!("{book_id}.json"));
        let xref_url = CROSSREF_URL_TEMPLATE.replace("{book}", book_id);
        if let Err(e) = fetch_if_stale(client, &xref_url, &xref_path, cache_ttl) {
            log::debug!("biblionlabs: no cross-refs for {book_id}: {e}");
            continue;
        }
        let bytes = match std::fs::read(&xref_path) {
            Ok(b) => b,
            Err(e) => {
                log::debug!("biblionlabs: read {book_id} xrefs: {e}");
                continue;
            }
        };
        let value: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                log::debug!("biblionlabs: parse {book_id} xrefs: {e}");
                continue;
            }
        };
        if let Err(e) = db.with_conn_mut(|conn| {
            let tx = conn.transaction().map_err(crate::db::DbError::from)?;
            tx.execute(
                "DELETE FROM cross_refs WHERE bible_id = ?1 AND src_book = ?2",
                rusqlite::params![bible_id, book_id],
            )
            .map_err(crate::db::DbError::from)?;
            insert_crossrefs_for_book(&tx, bible_id, book_id, &value)
                .map_err(crate::db::DbError::from)?;
            tx.commit().map_err(crate::db::DbError::from)?;
            Ok(())
        }) {
            log::debug!("biblionlabs: insert {book_id} xrefs: {e}");
        }
    }

    on_progress(Progress::Finished {
        bible_id: bible_id.into(),
        inserted_verses: inserted,
    });
    Ok(inserted)
}

/// Walk the fetch.bible crossref payload for one book + emit rows. The
/// JSON shape is `{ chapter_str: { verse_str: [[tgt_book, chap, verse,
/// end_chap?, end_verse?], …] } }`. We insert one row per target
/// (collapsing ranges to their start since the reader only needs a
/// clickable anchor, not a span).
fn insert_crossrefs_for_book(
    conn: &rusqlite::Connection,
    bible_id: &str,
    src_book: &str,
    value: &serde_json::Value,
) -> rusqlite::Result<()> {
    let Some(chapters) = value.as_object() else { return Ok(()) };
    let mut stmt = conn.prepare_cached(
        "INSERT INTO cross_refs
         (bible_id, src_book, src_chap, src_verse, tgt_book, tgt_chap, tgt_verse)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for (chap_key, verses) in chapters {
        let Ok(src_chap) = chap_key.parse::<i64>() else { continue };
        let Some(verses) = verses.as_object() else { continue };
        for (verse_key, targets) in verses {
            let Ok(src_verse) = verse_key.parse::<i64>() else { continue };
            let Some(targets) = targets.as_array() else { continue };
            for target in targets {
                let Some(items) = target.as_array() else { continue };
                let Some(tgt_book) = items.first().and_then(serde_json::Value::as_str) else {
                    continue;
                };
                let Some(tgt_chap) = items.get(1).and_then(serde_json::Value::as_i64) else {
                    continue;
                };
                let Some(tgt_verse) = items.get(2).and_then(serde_json::Value::as_i64) else {
                    continue;
                };
                stmt.execute(rusqlite::params![
                    bible_id, src_book, src_chap, src_verse, tgt_book, tgt_chap, tgt_verse,
                ])?;
            }
        }
    }
    Ok(())
}

/// Download `url` into `dest` when the on-disk copy is missing, older
/// than `ttl`, or does not start with a JSON prefix (handles the case
/// where a previous attempt cached an HTML error page from a 200 OK).
fn fetch_if_stale(client: &Client, url: &str, dest: &Path, ttl: Duration) -> Result<()> {
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
        return Err(ImportError::Format {
            reason: format!(
                "non-JSON response from {url} (got {} bytes starting with {:?})",
                bytes.len(),
                &bytes[..bytes.len().min(32)]
            ),
        });
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

fn upsert_bible(
    conn: &rusqlite::Connection,
    bible_id: &str,
    desc: &BibleDesc,
) -> rusqlite::Result<()> {
    let abbrev = desc.abbrev.clone().unwrap_or_else(|| bible_id.to_string());
    let name_local = desc
        .name_local
        .clone()
        .unwrap_or_else(|| bible_id.to_string());
    let language = desc.language.clone().unwrap_or_else(|| {
        // Fallback: first 3 chars of `bible_id` match ISO 639-3 in
        // biblionlabs conventions (e.g. "spa_rv1960" → "spa").
        bible_id.get(..3).unwrap_or("und").into()
    });
    conn.execute(
        "INSERT INTO bibles (id, abbrev, name_local, language) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
             abbrev = excluded.abbrev,
             name_local = excluded.name_local,
             language = excluded.language",
        params![bible_id, abbrev, name_local, language],
    )?;
    Ok(())
}

fn upsert_book(
    conn: &rusqlite::Connection,
    bible_id: &str,
    book_id: &str,
    desc: &BibleDesc,
    order_num: u16,
) -> rusqlite::Result<()> {
    let name = desc
        .book_names
        .get(book_id)
        .map(|n| if n.long.is_empty() { n.normal.clone() } else { n.long.clone() })
        .unwrap_or_else(|| book_id.to_string());
    let abbrev = desc
        .book_names
        .get(book_id)
        .map(|n| n.abbrev.clone())
        .unwrap_or_else(|| book_id.to_string());
    let testament = testament_for(book_id);
    conn.execute(
        "INSERT INTO books (bible_id, book_id, name, abbrev, order_num, testament)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(bible_id, book_id) DO UPDATE SET
             name = excluded.name,
             abbrev = excluded.abbrev,
             order_num = excluded.order_num,
             testament = excluded.testament",
        params![bible_id, book_id, name, abbrev, order_num as i64, testament],
    )?;
    Ok(())
}

/// Walk the 3-level `contents` array + emit one row per raw string.
/// Inline headings carried as `ContentItem::Typed { type = "heading" }`
/// also materialise as `headers` rows attached to the verse they
/// precede.
fn insert_book_contents(
    conn: &rusqlite::Connection,
    bible_id: &str,
    book_id: &str,
    book: &BookDoc,
) -> rusqlite::Result<usize> {
    let mut insert = conn.prepare_cached(
        "INSERT INTO verses (bible_id, book_id, chapter, verse, text, paragraph_break)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(bible_id, book_id, chapter, verse) DO UPDATE SET
             text = excluded.text,
             paragraph_break = excluded.paragraph_break",
    )?;

    let mut insert_header = conn.prepare_cached(
        "INSERT INTO headers (bible_id, book_id, chapter, before_verse, text)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;

    let mut count = 0usize;

    for (chapter_idx, chapter) in book.contents.iter().enumerate() {
        // biblionlabs uses 1-based indexing on `contents` — index 0 is
        // always an empty placeholder so `contents[1]` is chapter 1.
        if chapter_idx == 0 {
            continue;
        }
        let chapter_num = chapter_idx as u16;
        let mut pending_header: Option<String> = None;
        for (verse_idx, verse_contents) in chapter.iter().enumerate() {
            if verse_idx == 0 {
                // Same 1-based convention for verses.
                continue;
            }
            let verse_num = verse_idx as u16;
            let mut text = String::new();
            let mut paragraph_break = false;
            for item in verse_contents {
                match item {
                    ContentItem::Raw(s) => {
                        if !text.is_empty() && !text.ends_with(' ') {
                            text.push(' ');
                        }
                        text.push_str(s.trim());
                    }
                    ContentItem::Typed { r#type, contents } => match r#type.as_str() {
                        "heading" => {
                            pending_header = Some(contents.trim().to_string());
                        }
                        "paragraph" | "p" => {
                            paragraph_break = true;
                        }
                        // "note" footnotes drop out — they're reader
                        // apparatus, not verse text.
                        _ => {}
                    },
                }
            }

            if text.is_empty() {
                continue;
            }
            if let Some(h) = pending_header.take() {
                insert_header.execute(params![
                    bible_id,
                    book_id,
                    chapter_num as i64,
                    verse_num as i64,
                    h
                ])?;
            }
            insert.execute(params![
                bible_id,
                book_id,
                chapter_num as i64,
                verse_num as i64,
                text,
                paragraph_break as i64,
            ])?;
            count += 1;
        }
    }
    Ok(count)
}

/// Insert chapter-level headings (`chapter_headings`) + fine-grained
/// section ranges (`sections`) from the Bible description. Chapter
/// headings land at `before_verse = 0`; section headings at the start
/// verse of each range.
fn insert_headers(
    conn: &rusqlite::Connection,
    bible_id: &str,
    book_id: &str,
    desc: &BibleDesc,
) -> rusqlite::Result<()> {
    let mut insert_header = conn.prepare_cached(
        "INSERT INTO headers (bible_id, book_id, chapter, before_verse, text)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;

    if let Some(headings) = desc.chapter_headings.get(book_id) {
        for (chapter_idx, heading) in headings.iter().enumerate() {
            // biblionlabs 1-based indexing — slot 0 is placeholder.
            if chapter_idx == 0 || heading.is_empty() {
                continue;
            }
            let chapter_num = chapter_idx as u16;
            insert_header.execute(params![
                bible_id,
                book_id,
                chapter_num as i64,
                0i64,
                heading
            ])?;
        }
    }

    if let Some(sections) = desc.sections.get(book_id) {
        for section in sections {
            if let SectionEntry::Range { start_chapter, start_verse, heading, .. } = section {
                let Some(text) = heading else { continue };
                if text.is_empty() {
                    continue;
                }
                insert_header.execute(params![
                    bible_id,
                    book_id,
                    *start_chapter,
                    *start_verse,
                    text,
                ])?;
            }
        }
    }

    Ok(())
}

/// Minimal OT / NT lookup. Books absent from this list fall through as
/// `"OT"`; the deuterocanon is grouped with the OT per convention.
fn testament_for(book_id: &str) -> &'static str {
    const NT: &[&str] = &[
        "MAT", "MRK", "LUK", "JHN", "ACT", "ROM", "1CO", "2CO", "GAL", "EPH", "PHP", "COL",
        "1TH", "2TH", "1TI", "2TI", "TIT", "PHM", "HEB", "JAS", "1PE", "2PE", "1JN", "2JN",
        "3JN", "JUD", "REV",
    ];
    if NT.contains(&book_id) {
        "NT"
    } else {
        "OT"
    }
}

// ─── Catalogue extras ────────────────────────────────────────────
//
// Translations the biblionlabs raw repo hosts
// (github.com/biblionlabs/extra_data_source) but the top-level
// `v1.fetch.bible/manifest.json` catalogue does NOT list — typically
// because the translation is still under active copyright and the
// public catalogue only advertises CC / public-domain entries.
//
// Everything here must still be fetchable at the standard biblionlabs
// per-bible paths:
//   {BIBLIONLABS_BASE}/bibles/<id>/manifest.json
//   {BIBLIONLABS_BASE}/bibles/<id>/desc.json
//   {BIBLIONLABS_BASE}/bibles/<id>/books/<lowercase-book>.json
//
// The host app is responsible for having negotiated the appropriate
// usage license with the attribution holder before shipping any of
// these in a production build.

#[derive(Debug, Clone)]
pub struct ExtraBible {
    pub id: &'static str,
    pub name_local: &'static str,
    pub name_english: &'static str,
    pub local_abbrev: &'static str,
    pub language: &'static str,
}

pub const EXTRAS: &[ExtraBible] = &[ExtraBible {
    id: "spa_rv1960",
    name_local: "Reina Valera 1960",
    name_english: "Reina-Valera 1960",
    local_abbrev: "RVR1960",
    language: "Español",
}];

/// Lookup an extra by id. Returns `None` for ids served by the regular
/// catalogue (which already carry their own name / language metadata).
#[must_use]
pub fn find_extra(id: &str) -> Option<&'static ExtraBible> {
    EXTRAS.iter().find(|e| e.id == id)
}

// ─── Catalogue helpers ────────────────────────────────────────────

/// Load the top-level manifest from `cache_dir/manifest.json`, fetching
/// via HTTP when the cache is stale. Returns the parsed value so the UI
/// can list languages / bibles for the picker.
pub fn load_catalogue(
    client: &Client,
    cache_dir: &Path,
    manifest_url: &str,
    cache_ttl: Duration,
) -> Result<CatalogueView> {
    let path = cache_dir.join("manifest.json");
    std::fs::create_dir_all(cache_dir)?;
    fetch_if_stale(client, manifest_url, &path, cache_ttl)?;
    let bytes = std::fs::read(&path)?;
    let cat: Catalogue = serde_json::from_slice(&bytes)
        .map_err(|e| ImportError::Json { context: "catalogue".into(), source: e })?;
    Ok(CatalogueView::from_raw(cat))
}

/// UI-facing projection of the catalogue — kept narrow so callers don't
/// need to know the raw JSON shape.
#[derive(Debug, Default, Clone)]
pub struct CatalogueView {
    pub bibles: Vec<BibleEntryView>,
    pub languages: Vec<LanguageEntryView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BibleEntryView {
    pub id: String,
    pub language: Option<String>,
    pub name_local: Option<String>,
    pub name_english: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LanguageEntryView {
    pub id: String,
    pub local: String,
    pub english: String,
    pub direction: String,
}

impl CatalogueView {
    fn from_raw(c: Catalogue) -> Self {
        // The upstream manifest nests the bible name under
        // `name.{local,english,local_abbrev,english_abbrev}` and
        // never surfaces a `language` field on the bible entry —
        // language is derived from the id prefix (ISO 639-3) and
        // resolved against the `languages` map's `local` display.
        let lang_lookup: &HashMap<String, LanguageEntry> = &c.languages;
        let mut bibles: Vec<BibleEntryView> = c
            .bibles
            .iter()
            .map(|(id, v)| {
                let name = v.get("name");
                let non_empty = |s: &serde_json::Value| {
                    s.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
                };
                let name_local = name
                    .and_then(|n| n.get("local"))
                    .and_then(non_empty)
                    .or_else(|| name.and_then(|n| n.get("local_abbrev")).and_then(non_empty));
                let name_english = name
                    .and_then(|n| n.get("english"))
                    .and_then(non_empty)
                    .or_else(|| name.and_then(|n| n.get("english_abbrev")).and_then(non_empty));
                let language = id
                    .split('_')
                    .next()
                    .and_then(|iso| lang_lookup.get(iso))
                    .and_then(|l| {
                        l.local
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .or_else(|| {
                                l.english
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                    .map(str::to_string)
                            })
                    });
                BibleEntryView {
                    id: id.clone(),
                    language,
                    name_local,
                    name_english,
                }
            })
            .collect();
        // Merge in hardcoded extras — translations hosted by the
        // biblionlabs raw repo but absent from the top-level catalogue.
        // Skip any id already present (catalogue wins) so a future
        // upstream addition doesn't collide.
        for extra in EXTRAS {
            if bibles.iter().any(|b| b.id == extra.id) {
                continue;
            }
            bibles.push(BibleEntryView {
                id: extra.id.to_string(),
                language: Some(extra.language.to_string()),
                name_local: Some(extra.name_local.to_string()),
                name_english: Some(extra.name_english.to_string()),
            });
        }
        bibles.sort_by(|a, b| a.id.cmp(&b.id));

        let mut languages: Vec<LanguageEntryView> = c
            .languages
            .into_iter()
            .map(|(id, l)| LanguageEntryView {
                id,
                local: l.local.unwrap_or_default(),
                english: l.english.unwrap_or_default(),
                direction: l.direction.unwrap_or_else(|| "ltr".into()),
            })
            .collect();
        languages.sort_by(|a, b| a.id.cmp(&b.id));

        Self { bibles, languages }
    }
}

#[allow(dead_code)]
pub(crate) fn _ensure_cache_dir(cache_dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(cache_dir)?;
    Ok(cache_dir.to_path_buf())
}

// ─── Offline test — populate from an in-memory book doc + desc ───
// Exposed for `#[cfg(test)]` so unit tests can exercise the parser
// path without a network round-trip.

#[cfg(test)]
pub(crate) fn populate_from_fixtures(
    db: &Db,
    bible_id: &str,
    desc: &serde_json::Value,
    books: &[(String, serde_json::Value)],
) -> Result<usize> {
    let desc_parsed: BibleDesc = serde_json::from_value(desc.clone())
        .map_err(|e| ImportError::Json { context: "desc fixture".into(), source: e })?;

    let mut book_ids: Vec<&String> = desc_parsed.book_names.keys().collect();
    book_ids.sort();

    db.with_conn_mut(|conn| {
        upsert_bible(conn, bible_id, &desc_parsed).map_err(crate::db::DbError::from)?;
        let tx = conn.transaction().map_err(crate::db::DbError::from)?;
        for (order_num, book_id) in book_ids.iter().enumerate() {
            upsert_book(&tx, bible_id, book_id, &desc_parsed, order_num as u16)
                .map_err(crate::db::DbError::from)?;
        }
        tx.commit().map_err(crate::db::DbError::from)?;
        Ok(())
    })?;

    let mut inserted = 0usize;
    for (book_id, raw) in books {
        let book_doc: BookDoc = serde_json::from_value(raw.clone())
            .map_err(|e| ImportError::Json { context: format!("{book_id} fixture"), source: e })?;
        let n = db.with_conn_mut(|conn| {
            let tx = conn.transaction().map_err(crate::db::DbError::from)?;
            let n = insert_book_contents(&tx, bible_id, book_id, &book_doc)
                .map_err(crate::db::DbError::from)?;
            insert_headers(&tx, bible_id, book_id, &desc_parsed)
                .map_err(crate::db::DbError::from)?;
            tx.commit().map_err(crate::db::DbError::from)?;
            Ok(n)
        })?;
        inserted += n;
    }
    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn eph_fixture() -> (serde_json::Value, serde_json::Value) {
        let desc = json!({
            "language": "spa",
            "name_local": "Reina Valera 1960",
            "abbrev": "RVR1960",
            "book_names": {
                "EPH": {
                    "normal": "Efesios",
                    "long": "Carta a los Efesios",
                    "abbrev": "Ef"
                }
            },
            "chapter_headings": {
                // 1-based: slot 0 is placeholder, slot 6 is chapter 6.
                "EPH": ["", "", "", "", "", "", "La armadura de Dios"]
            },
            "sections": {
                "EPH": [
                    { "start_chapter": 6, "start_verse": 1, "end_chapter": 6, "end_verse": 4,
                      "heading": "Instrucciones a hijos y padres" },
                    { "start_chapter": 6, "start_verse": 5, "end_chapter": 6, "end_verse": 9,
                      "heading": "Siervos y amos" }
                ]
            }
        });
        let book = json!({
            "book": "EPH",
            "name": { "normal": "Efesios", "long": "Carta a los Efesios", "abbrev": "Ef" },
            // 1-based: contents[0] + verse[0] are empty placeholders.
            "contents": [
                [],
                [[], [ "En el principio …" ]],
                [[], [ "placeholder c2 v1" ]],
                [[], [ "placeholder c3 v1" ]],
                [[], [ "placeholder c4 v1" ]],
                [[], [ "placeholder c5 v1" ]],
                // chapter 6
                [
                    [],
                    [ "Hijos, obedeced en el Señor a vuestros padres, porque esto es justo." ],
                    [ "Honra a tu padre y a tu madre, que es el primer mandamiento con promesa;" ],
                    [ "para que te vaya bien, y seas de larga vida sobre la tierra." ],
                    [
                        { "type": "paragraph", "contents": "" },
                        "Y vosotros, padres, no provoquéis a ira a vuestros hijos."
                    ]
                ]
            ]
        });
        (desc, book)
    }

    #[test]
    fn populate_fixture_round_trips_verses_and_headers() {
        let db = Db::in_memory().unwrap();
        let (desc, book) = eph_fixture();
        let n = populate_from_fixtures(
            &db,
            "spa_rv1960",
            &desc,
            &[("EPH".into(), book)],
        )
        .expect("populate");
        assert_eq!(n, 9, "expected 9 verses inserted across chapters 1..6");

        let chap = db.load_chapter("spa_rv1960", "EPH", 6).unwrap();
        assert_eq!(chap.verses.len(), 4);
        assert_eq!(chap.verses[0].verse, 1);
        assert!(chap.verses[3].text.starts_with("Y vosotros"));
        assert!(chap.verses[3].paragraph_break);

        // Two source-section headings + one chapter-level heading.
        assert_eq!(chap.headers.len(), 3);
        assert!(chap.headers.iter().any(|h| h.before_verse == 0
            && h.text == "La armadura de Dios"));
        assert!(chap.headers.iter().any(|h| h.before_verse == 1
            && h.text == "Instrucciones a hijos y padres"));
        assert!(chap.headers.iter().any(|h| h.before_verse == 5
            && h.text == "Siervos y amos"));
    }

    #[test]
    fn fixture_search_works() {
        let db = Db::in_memory().unwrap();
        let (desc, book) = eph_fixture();
        populate_from_fixtures(&db, "spa_rv1960", &desc, &[("EPH".into(), book)]).unwrap();
        let hits = db.search(Some("spa_rv1960"), "senor", 10).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].book_id, "EPH");
    }
}
