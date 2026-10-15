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
    mut on_progress: impl FnMut(Progress),
) -> Result<usize> {
    on_progress(Progress::Started { bible_id: bible_id.into() });

    let bible_dir = cache_dir.join("bibles").join(bible_id);
    std::fs::create_dir_all(&bible_dir)?;

    let desc_path = bible_dir.join("manifest.json");
    fetch_if_stale(client, desc_url, &desc_path, cache_ttl)?;
    let desc_bytes = std::fs::read(&desc_path)?;
    let desc: BibleDesc = serde_json::from_slice(&desc_bytes)
        .map_err(|e| ImportError::Json { context: format!("{bible_id} desc"), source: e })?;

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

    on_progress(Progress::Finished {
        bible_id: bible_id.into(),
        inserted_verses: inserted,
    });
    Ok(inserted)
}

/// Download `url` into `dest` when the on-disk copy is missing or older
/// than `ttl`. Returns `Ok(())` on cache hit without touching the file.
fn fetch_if_stale(client: &Client, url: &str, dest: &Path, ttl: Duration) -> Result<()> {
    if dest.exists() {
        if let Ok(meta) = dest.metadata() {
            if let Ok(modified) = meta.modified() {
                if modified.elapsed().map(|e| e <= ttl).unwrap_or(false) {
                    return Ok(());
                }
            }
        }
    }
    let bytes = client.get(url).send()?.error_for_status()?.bytes()?;
    std::fs::write(dest, &bytes)?;
    Ok(())
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
        let chapter_num = (chapter_idx as u16) + 1;
        let mut pending_header: Option<String> = None;
        for (verse_idx, verse_contents) in chapter.iter().enumerate() {
            let verse_num = (verse_idx as u16) + 1;
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
            if heading.is_empty() {
                continue;
            }
            let chapter_num = (chapter_idx as u16) + 1;
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

#[derive(Debug, Clone)]
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
        let mut bibles: Vec<BibleEntryView> = c
            .bibles
            .into_iter()
            .map(|(id, v)| BibleEntryView {
                id,
                language: v
                    .get("language")
                    .and_then(|x| x.as_str())
                    .map(str::to_string),
                name_local: v
                    .get("name_local")
                    .and_then(|x| x.as_str())
                    .map(str::to_string),
                name_english: v
                    .get("name_english")
                    .and_then(|x| x.as_str())
                    .map(str::to_string),
            })
            .collect();
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
                "EPH": ["", "", "", "", "", "La armadura de Dios"]
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
            "contents": [
                // chapter 1 → chapters 2..5 omitted for test brevity
                [[ "En el principio …" ]],
                [[ "placeholder c2 v1" ]],
                [[ "placeholder c3 v1" ]],
                [[ "placeholder c4 v1" ]],
                [[ "placeholder c5 v1" ]],
                // chapter 6
                [
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
