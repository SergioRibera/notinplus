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
use crate::import::layout;
use crate::plugin_config::CrossRefLevel;

/// Default endpoints used by `install_bible` when the caller doesn't
/// supply a custom source. Mirror the biblionlabs/fetch.bible layout.
pub const DEFAULT_MANIFEST_URL: &str = "https://v1.fetch.bible/manifest.json";

/// fetch.bible cross-reference bundle, per set ([`CrossRefLevel`]) and
/// lowercase book id.
/// Shape: `{ chapter_str: { verse_str: [[tgt_book, chap, verse, …], …] } }`.
pub const CROSSREF_URL_TEMPLATE: &str = "https://v1.fetch.bible/crossref/{level}/{book}.json";

#[derive(Debug)]
pub enum ImportError {
    Http(reqwest::Error),
    Json {
        context: String,
        source: serde_json::Error,
    },
    Io(std::io::Error),
    Db(crate::db::DbError),
    Format {
        reason: String,
    },
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
    Started {
        bible_id: String,
    },
    ManifestFetched {
        total_books: usize,
    },
    BookDone {
        book_id: String,
        verses: usize,
    },
    /// Cross-references of `done` books of `total` are in.
    CrossRefs {
        done: usize,
        total: usize,
    },
    Finished {
        bible_id: String,
        inserted_verses: usize,
    },
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
    book_names: BookNames,
    #[serde(default)]
    chapter_headings: HashMap<String, Vec<String>>,
    #[serde(default)]
    sections: HashMap<String, Vec<SectionEntry>>,
    /// Optional language id — if absent we fall back to the `bible_id` prefix.
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    name_local: Option<String>,
    #[serde(default)]
    name_english: Option<String>,
    #[serde(default)]
    abbrev: Option<String>,
    /// Bumped by the source whenever the text or its layout changes;
    /// absent counts as 1.
    #[serde(default)]
    revision: Option<u32>,
}

/// Book names keyed by book id, in the order the manifest lists them:
/// reading order, deuterocanonical books where each tradition puts them.
#[derive(Debug, Default)]
struct BookNames(Vec<(String, BookName)>);

impl BookNames {
    fn get(&self, book_id: &str) -> Option<&BookName> {
        self.0
            .iter()
            .find(|(id, _)| id == book_id)
            .map(|(_, name)| name)
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    /// Book ids in reading order.
    fn ids(&self) -> Vec<&String> {
        self.0.iter().map(|(id, _)| id).collect()
    }
}

impl<'de> Deserialize<'de> for BookNames {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Ordered;

        impl<'de> serde::de::Visitor<'de> for Ordered {
            type Value = BookNames;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a map of book ids to names")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<BookNames, A::Error> {
                let mut books = Vec::with_capacity(map.size_hint().unwrap_or(0));
                while let Some(entry) = map.next_entry::<String, BookName>()? {
                    books.push(entry);
                }
                Ok(BookNames(books))
            }
        }

        deserializer.deserialize_map(Ordered)
    }
}

impl Serialize for BookNames {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (id, name) in &self.0 {
            map.serialize_entry(id, name)?;
        }
        map.end()
    }
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
        /// Heading level: 1 a major division, 2 a section, 3 a psalm
        /// title or a range.
        #[serde(default)]
        level: Option<u8>,
    },
    Raw(String),
}

/// Metadata overrides from the top-level catalogue.
///
/// The per-bible biblionlabs manifest only carries `book_names` /
/// `chapter_headings` / `sections` — the user-facing name, language, and
/// abbreviation live on the catalogue entry, so the caller resolves them there
/// and hands them in here. Any field left `None` falls back to a sensible
/// default (`bible_id` prefix for language, `bible_id` itself for name /
/// abbrev).
#[derive(Debug, Default, Clone)]
pub struct BibleMeta {
    pub name_local: Option<String>,
    pub name_english: Option<String>,
    pub language: Option<String>,
    pub abbrev: Option<String>,
}

/// One translation download: the catalogue entry `bible_id` lands as JSON
/// files under `cache_dir` and is then written to `db`.
///
/// Each file is fetched again only when its cached copy is older than
/// `cache_ttl`.
/// Where an install learns which books a translation has.
#[derive(Debug, Clone)]
pub enum Books<'a> {
    /// A per-Bible manifest at this URL (biblionlabs: book names in reading
    /// order, headings, a revision).
    Manifest(&'a str),
    /// These book ids, from the fetch.bible catalogue, which has no
    /// per-Bible manifest; each book file names itself.
    Listed(Vec<String>),
}

#[derive(Debug)]
pub struct Install<'a> {
    pub db: &'a Db,
    pub client: &'a Client,
    pub bible_id: &'a str,
    pub cache_dir: &'a Path,
    /// Where the list of books comes from.
    pub books: Books<'a>,
    /// Book URL with `{bible_id}` and `{book}` placeholders.
    pub books_url_template: &'a str,
    pub cache_ttl: Duration,
    pub meta: &'a BibleMeta,
    /// `Some(stored revision)` refreshes an installed translation instead:
    /// the manifest and books download again, the translation stays
    /// listed meanwhile, and nothing changes unless the source's revision
    /// is newer.
    pub refresh: Option<u32>,
}

/// What an install stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub inserted: usize,
    /// Revision of the source (1 when it gives none).
    pub revision: u32,
    /// The source brings its own headings / footnotes.
    pub headings: bool,
    pub notes: bool,
    /// A refresh found nothing newer and stored nothing.
    pub unchanged: bool,
    /// Books that could not be fetched or read, and were skipped.
    pub missing: usize,
}

impl Install<'_> {
    /// How old a cached file may be; a refresh downloads everything again.
    const fn ttl(&self) -> Duration {
        if self.refresh.is_some() {
            Duration::ZERO
        } else {
            self.cache_ttl
        }
    }

    /// Download and store the translation; returns how many verses
    /// were inserted.
    #[cfg_attr(feature = "sismo", sismo::function("bible::import_run"))]
    pub fn run(&self, mut on_progress: impl FnMut(Progress)) -> Result<Report> {
        let bible_id = self.bible_id;
        on_progress(Progress::Started {
            bible_id: bible_id.into(),
        });
        let bible_dir = self.cache_dir.join("bibles").join(bible_id);
        let desc = self.manifest(&bible_dir)?;
        let revision = desc.revision.unwrap_or(1);
        if self.refresh.is_some_and(|stored| revision <= stored) {
            return Ok(Report {
                revision,
                unchanged: true,
                ..Report::default()
            });
        }
        let books_dir = bible_dir.join("books");
        std::fs::create_dir_all(&books_dir)?;
        on_progress(Progress::ManifestFetched {
            total_books: desc.book_names.len(),
        });

        let book_ids = desc.book_names.ids();
        self.register_books(&desc, &book_ids)?;

        let mut report = Report {
            revision,
            ..Report::default()
        };
        for book_id in &book_ids {
            let Some((verses, extras)) = self.install_book(&books_dir, book_id, &desc)? else {
                report.missing += 1;
                continue;
            };
            report.inserted += verses;
            report.headings |= extras.headings;
            report.notes |= extras.notes;
            #[cfg(feature = "sismo")]
            sismo::counter!("bible.import_verses", report.inserted);
            on_progress(Progress::BookDone {
                book_id: (**book_id).clone(),
                verses,
            });
        }
        on_progress(Progress::Finished {
            bible_id: bible_id.into(),
            inserted_verses: report.inserted,
        });
        Ok(report)
    }

    /// The per-Bible manifest, with the catalogue's name, language and
    /// abbreviation filling what it leaves out (it usually does).
    #[cfg_attr(feature = "sismo", sismo::function("bible::import_manifest"))]
    fn manifest(&self, bible_dir: &Path) -> Result<BibleDesc> {
        std::fs::create_dir_all(bible_dir)?;
        let mut desc: BibleDesc = match &self.books {
            Books::Manifest(url) => {
                let desc_path = bible_dir.join("manifest.json");
                fetch_if_stale(self.client, url, &desc_path, self.ttl())?;
                let desc_bytes = std::fs::read(&desc_path)?;
                serde_json::from_slice(&desc_bytes).map_err(|e| ImportError::Json {
                    context: format!("{} desc", self.bible_id),
                    source: e,
                })?
            }
            Books::Listed(ids) => BibleDesc {
                book_names: BookNames(
                    ids.iter()
                        .map(|id| (id.clone(), BookName::default()))
                        .collect(),
                ),
                ..BibleDesc::default()
            },
        };
        let meta = self.meta;
        if desc.name_local.is_none() {
            desc.name_local.clone_from(&meta.name_local);
            if desc.name_local.is_none() {
                desc.name_local.clone_from(&meta.name_english);
            }
        }
        if desc.name_english.is_none() {
            desc.name_english.clone_from(&meta.name_english);
        }
        if desc.language.is_none() {
            desc.language.clone_from(&meta.language);
        }
        if desc.abbrev.is_none() {
            desc.abbrev.clone_from(&meta.abbrev);
        }
        Ok(desc)
    }

    /// The Bible row and one row per book, in `book_ids` order. The Bible
    /// stays pending, out of the lists, until the caller finishes it.
    #[cfg_attr(feature = "sismo", sismo::function("bible::import_register_books"))]
    fn register_books(&self, desc: &BibleDesc, book_ids: &[&String]) -> Result<()> {
        self.db.with_conn_mut(|conn| {
            upsert_bible(conn, self.bible_id, desc).map_err(crate::db::DbError::from)?;
            // A refresh keeps the translation listed while it runs.
            if self.refresh.is_none() {
                conn.execute(
                    "INSERT OR IGNORE INTO installs_pending (bible_id) VALUES (?1)",
                    [self.bible_id],
                )
                .map_err(crate::db::DbError::from)?;
            }
            let tx = conn.transaction().map_err(crate::db::DbError::from)?;
            for (order_num, book_id) in book_ids.iter().enumerate() {
                let order_num = u16::try_from(order_num).unwrap_or(u16::MAX);
                upsert_book(&tx, self.bible_id, book_id, desc, order_num)
                    .map_err(crate::db::DbError::from)?;
            }
            tx.commit().map_err(crate::db::DbError::from)?;
            Ok(())
        })?;
        Ok(())
    }

    /// Verses and section headings of one book; `None` when the book
    /// could not be fetched or parsed, which skips it without failing
    /// the install.
    #[cfg_attr(feature = "sismo", sismo::function("bible::import_book"))]
    fn install_book(
        &self,
        books_dir: &Path,
        book_id: &str,
        desc: &BibleDesc,
    ) -> Result<Option<(usize, BookExtras)>> {
        let book_path = books_dir.join(format!("{book_id}.json"));
        let url = self
            .books_url_template
            .replace("{bible_id}", self.bible_id)
            .replace("{book}", book_id);
        if let Err(e) = fetch_if_stale(self.client, &url, &book_path, self.ttl()) {
            tracing::warn!("biblionlabs: skipping {book_id}: {e}");
            return Ok(None);
        }
        let book_bytes = std::fs::read(&book_path)?;
        let book_doc: BookDoc = {
            #[cfg(feature = "sismo")]
            sismo::scope!("bible::import_parse_book");
            match serde_json::from_slice(&book_bytes) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("biblionlabs: {book_id} parse failed: {e}");
                    return Ok(None);
                }
            }
        };
        let verses = self.db.with_conn_mut(|conn| {
            let tx = conn.transaction().map_err(crate::db::DbError::from)?;
            let (n, extras) = insert_book_contents(&tx, self.bible_id, book_id, &book_doc)
                .map_err(crate::db::DbError::from)?;
            insert_headers(&tx, self.bible_id, book_id, desc).map_err(crate::db::DbError::from)?;
            // A listed book had no name before its file came: take the
            // file's own.
            if desc
                .book_names
                .get(book_id)
                .is_none_or(|name| name.normal.is_empty())
                && !book_doc.name.normal.is_empty()
            {
                name_book(&tx, self.bible_id, book_id, &book_doc.name)
                    .map_err(crate::db::DbError::from)?;
            }
            tx.commit().map_err(crate::db::DbError::from)?;
            Ok((n, extras))
        })?;
        Ok(Some(verses))
    }
}

/// The cross-reference set at one [`CrossRefLevel`].
///
/// One set serves every translation: fetch.bible keys it by book, chapter
/// and verse. Runs after the first translation installs and again when
/// the user picks another level.
#[derive(Debug)]
pub struct CrossRefs<'a> {
    pub db: &'a Db,
    pub client: &'a Client,
    pub cache_dir: &'a Path,
    pub cache_ttl: Duration,
    pub level: CrossRefLevel,
}

impl CrossRefs<'_> {
    /// Cross-references for every book, replacing the set stored so far.
    /// They are optional: a book without them is logged and skipped.
    #[cfg_attr(feature = "sismo", sismo::function("bible::import_crossrefs"))]
    pub fn run(&self, book_ids: &[String], on_progress: &mut impl FnMut(Progress)) -> Result<()> {
        let xref_cache = self.cache_dir.join("crossrefs").join(self.level.as_str());
        std::fs::create_dir_all(&xref_cache)?;
        let total = book_ids.len();
        for (done, book_id) in book_ids.iter().enumerate() {
            #[cfg(feature = "sismo")]
            sismo::scope!("bible::import_crossrefs_book");
            on_progress(Progress::CrossRefs { done, total });
            let xref_path = xref_cache.join(format!("{book_id}.json"));
            let xref_url = CROSSREF_URL_TEMPLATE
                .replace("{level}", self.level.as_str())
                .replace("{book}", book_id);
            if let Err(e) = fetch_if_stale(self.client, &xref_url, &xref_path, self.cache_ttl) {
                tracing::debug!("biblionlabs: no cross-refs for {book_id}: {e}");
                continue;
            }
            let value: serde_json::Value = match std::fs::read(&xref_path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))
            {
                Ok(v) => v,
                Err(e) => {
                    tracing::debug!("biblionlabs: read {book_id} xrefs: {e}");
                    continue;
                }
            };
            if let Err(e) = self.db.with_conn_mut(|conn| {
                let tx = conn.transaction().map_err(crate::db::DbError::from)?;
                tx.execute(
                    "DELETE FROM cross_refs WHERE src_book = ?1",
                    rusqlite::params![book_id],
                )
                .map_err(crate::db::DbError::from)?;
                insert_crossrefs_for_book(&tx, book_id, &value)
                    .map_err(crate::db::DbError::from)?;
                tx.commit().map_err(crate::db::DbError::from)?;
                Ok(())
            }) {
                tracing::debug!("biblionlabs: insert {book_id} xrefs: {e}");
            }
        }
        Ok(())
    }
}

/// Walk the fetch.bible crossref payload for one book + emit rows. The
/// JSON shape is `{ chapter_str: { verse_str: [[tgt_book, chap, verse,
/// end_chap?, end_verse?], …] } }`. We insert one row per target
/// (collapsing ranges to their start since the reader only needs a
/// clickable anchor, not a span).
#[cfg_attr(feature = "sismo", sismo::function("bible::import_crossrefs_insert"))]
fn insert_crossrefs_for_book(
    conn: &rusqlite::Connection,
    src_book: &str,
    value: &serde_json::Value,
) -> rusqlite::Result<()> {
    let Some(chapters) = value.as_object() else {
        return Ok(());
    };
    let mut stmt = conn.prepare_cached(
        "INSERT INTO cross_refs
         (src_book, src_chap, src_verse, tgt_book, tgt_chap, tgt_verse)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for (chap_key, verses) in chapters {
        let Ok(src_chap) = chap_key.parse::<i64>() else {
            continue;
        };
        let Some(verses) = verses.as_object() else {
            continue;
        };
        for (verse_key, targets) in verses {
            let Ok(src_verse) = verse_key.parse::<i64>() else {
                continue;
            };
            let Some(targets) = targets.as_array() else {
                continue;
            };
            for target in targets {
                let Some(items) = target.as_array() else {
                    continue;
                };
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
                    src_book, src_chap, src_verse, tgt_book, tgt_chap, tgt_verse,
                ])?;
            }
        }
    }
    Ok(())
}

/// Download `url` into `dest` when the on-disk copy is missing, older
/// than `ttl`, or does not start with a JSON prefix (handles the case
/// where a previous attempt cached an HTML error page from a 200 OK).
#[cfg_attr(feature = "sismo", sismo::function("bible::import_fetch"))]
fn fetch_if_stale(client: &Client, url: &str, dest: &Path, ttl: Duration) -> Result<()> {
    if dest.exists() {
        if let Ok(meta) = dest.metadata() {
            if let Ok(modified) = meta.modified() {
                let fresh = modified.elapsed().is_ok_and(|e| e <= ttl);
                if fresh && cache_looks_like_json(dest) {
                    #[cfg(feature = "sismo")]
                    sismo::instant!("bible::import_fetch_cached");
                    return Ok(());
                }
            }
        }
    }
    let bytes = {
        #[cfg(feature = "sismo")]
        sismo::scope!("bible::import_download");
        client.get(url).send()?.error_for_status()?.bytes()?
    };
    #[cfg(feature = "sismo")]
    sismo::counter!("bible.download_bytes", bytes.len());
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

#[cfg_attr(feature = "sismo", sismo::function("bible::import_upsert_bible"))]
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

/// Give `book_id` of `bible_id` the name its own file carries.
fn name_book(
    conn: &rusqlite::Connection,
    bible_id: &str,
    book_id: &str,
    name: &BookName,
) -> rusqlite::Result<()> {
    let long = if name.long.is_empty() {
        &name.normal
    } else {
        &name.long
    };
    let abbrev = if name.abbrev.is_empty() {
        &name.normal
    } else {
        &name.abbrev
    };
    conn.execute(
        "UPDATE books SET name = ?3, abbrev = ?4 WHERE bible_id = ?1 AND book_id = ?2",
        params![bible_id, book_id, long, abbrev],
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
    let name = desc.book_names.get(book_id).map_or_else(
        || book_id.to_string(),
        |n| {
            if n.long.is_empty() {
                n.normal.clone()
            } else {
                n.long.clone()
            }
        },
    );
    let abbrev = desc
        .book_names
        .get(book_id)
        .map_or_else(|| book_id.to_string(), |n| n.abbrev.clone());
    let testament = testament_for(book_id);
    conn.execute(
        "INSERT INTO books (bible_id, book_id, name, abbrev, order_num, testament)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(bible_id, book_id) DO UPDATE SET
             name = excluded.name,
             abbrev = excluded.abbrev,
             order_num = excluded.order_num,
             testament = excluded.testament",
        params![
            bible_id,
            book_id,
            name,
            abbrev,
            i64::from(order_num),
            testament
        ],
    )?;
    Ok(())
}

/// What a book brought besides its verses, so the installer knows not to
/// borrow headings or notes from another translation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct BookExtras {
    pub headings: bool,
    pub notes: bool,
}

/// Walk the 3-level `contents` array + emit one row per verse.
///
/// Every row of the book from an earlier import (headings, footnotes) goes
/// first, so a refresh replaces them. Each verse keeps its line layout
/// ([`layout`]): a `\n\n` before it marks `paragraph_break`, a `\n` marks
/// `line_start`, breaks inside it land in `line_breaks`. Inline headings
/// (`type: "heading"`, with an optional `level`) become `headers` rows in
/// order before the verse they precede; notes (`type: "note"`) become
/// `footnotes` rows.
#[cfg_attr(feature = "sismo", sismo::function("bible::import_insert_verses"))]
fn insert_book_contents(
    conn: &rusqlite::Connection,
    bible_id: &str,
    book_id: &str,
    book: &BookDoc,
) -> rusqlite::Result<(usize, BookExtras)> {
    for table in ["headers", "footnotes"] {
        conn.execute(
            &format!("DELETE FROM {table} WHERE bible_id = ?1 AND book_id = ?2"),
            params![bible_id, book_id],
        )?;
    }
    let mut insert = conn.prepare_cached(
        "INSERT INTO verses
             (bible_id, book_id, chapter, verse, text, paragraph_break, line_start, line_breaks)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(bible_id, book_id, chapter, verse) DO UPDATE SET
             text = excluded.text,
             paragraph_break = excluded.paragraph_break,
             line_start = excluded.line_start,
             line_breaks = excluded.line_breaks",
    )?;
    let mut insert_header = conn.prepare_cached(
        "INSERT INTO headers (bible_id, book_id, chapter, before_verse, text, level)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    let mut insert_note = conn.prepare_cached(
        "INSERT OR REPLACE INTO footnotes
             (bible_id, book_id, chapter, verse, note_id, caller, text)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;

    let mut count = 0usize;
    let mut extras = BookExtras::default();

    for (chapter_idx, chapter) in book.contents.iter().enumerate() {
        // biblionlabs uses 1-based indexing on `contents` — index 0 is
        // always an empty placeholder so `contents[1]` is chapter 1.
        if chapter_idx == 0 {
            continue;
        }
        let chapter_num = i64::try_from(chapter_idx).unwrap_or(i64::MAX);
        let mut pending_headers: Vec<(String, i64)> = Vec::new();
        let mut previous_end: Option<layout::Break> = None;
        let mut notes = 0i64;
        for (verse_idx, verse_contents) in chapter.iter().enumerate() {
            if verse_idx == 0 {
                // Same 1-based convention for verses.
                continue;
            }
            let verse_num = i64::try_from(verse_idx).unwrap_or(i64::MAX);
            let mut typed_paragraph = false;
            let mut verse_notes = Vec::new();
            for item in verse_contents {
                if let ContentItem::Typed {
                    r#type,
                    contents,
                    level,
                } = item
                {
                    match r#type.as_str() {
                        "heading" => {
                            let text = contents.trim();
                            if !text.is_empty() {
                                pending_headers.push((
                                    text.to_owned(),
                                    i64::from(level.unwrap_or(DEFAULT_HEADING_LEVEL)),
                                ));
                            }
                        }
                        "paragraph" | "p" => typed_paragraph = true,
                        "note" => {
                            let text = layout::note_text(contents);
                            if !text.is_empty() {
                                verse_notes.push(text.to_owned());
                            }
                        }
                        _ => {}
                    }
                }
            }
            let laid = layout::lay_out(verse_contents.iter().filter_map(|item| match item {
                ContentItem::Raw(text) => Some(text.as_str()),
                ContentItem::Typed { .. } => None,
            }));
            if laid.text.is_empty() {
                // A verse the source leaves out: its headings wait for
                // the next one.
                continue;
            }
            let opens = previous_end.or(laid.starts);
            previous_end = laid.ends;
            let headed = !pending_headers.is_empty();
            for (text, level) in std::mem::take(&mut pending_headers) {
                extras.headings = true;
                insert_header.execute(params![
                    bible_id,
                    book_id,
                    chapter_num,
                    verse_num,
                    text,
                    level
                ])?;
            }
            for text in verse_notes {
                extras.notes = true;
                notes += 1;
                insert_note.execute(params![
                    bible_id,
                    book_id,
                    chapter_num,
                    verse_num,
                    notes,
                    note_caller(notes),
                    text
                ])?;
            }
            let paragraph = typed_paragraph || headed || opens == Some(layout::Break::Paragraph);
            insert.execute(params![
                bible_id,
                book_id,
                chapter_num,
                verse_num,
                laid.text,
                i64::from(paragraph),
                i64::from(opens == Some(layout::Break::Line)),
                layout::encode_breaks(&laid.breaks),
            ])?;
            count += 1;
        }
    }
    Ok((count, extras))
}

/// Level of a heading the source gives none for: a section heading.
const DEFAULT_HEADING_LEVEL: u8 = 2;

/// `a` … `z`, then `aa`, `ab` …: the mark of the `n`th note of a chapter.
fn note_caller(n: i64) -> String {
    let mut n = n.max(1) - 1;
    let mut caller = Vec::new();
    loop {
        caller.push(b'a' + u8::try_from(n % 26).unwrap_or(0));
        n /= 26;
        if n == 0 {
            break;
        }
        n -= 1;
    }
    caller.reverse();
    String::from_utf8(caller).unwrap_or_default()
}

/// Insert chapter-level headings (`chapter_headings`) + fine-grained
/// section ranges (`sections`) from the Bible description. Chapter
/// headings land at `before_verse = 0`; section headings at the start
/// verse of each range.
#[cfg_attr(feature = "sismo", sismo::function("bible::import_insert_headers"))]
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
            let chapter_num = u16::try_from(chapter_idx).unwrap_or(u16::MAX);
            insert_header.execute(params![
                bible_id,
                book_id,
                i64::from(chapter_num),
                0i64,
                heading
            ])?;
        }
    }

    if let Some(sections) = desc.sections.get(book_id) {
        for section in sections {
            if let SectionEntry::Range {
                start_chapter,
                start_verse,
                heading,
                ..
            } = section
            {
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
        "MAT", "MRK", "LUK", "JHN", "ACT", "ROM", "1CO", "2CO", "GAL", "EPH", "PHP", "COL", "1TH",
        "2TH", "1TI", "2TI", "TIT", "PHM", "HEB", "JAS", "1PE", "2PE", "1JN", "2JN", "3JN", "JUD",
        "REV",
    ];
    // Ids come lowercase from biblionlabs and fetch.bible.
    if NT.iter().any(|nt| nt.eq_ignore_ascii_case(book_id)) {
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
    /// Revision of its published files this build expects; an install
    /// at an older one refreshes in the background.
    pub revision: u32,
    pub name_local: &'static str,
    pub name_english: &'static str,
    pub local_abbrev: &'static str,
    pub language: &'static str,
}

pub const EXTRAS: &[ExtraBible] = &[ExtraBible {
    id: "spa_rv1960",
    // 2: text, headings, poetry lines and footnotes from the official USX.
    revision: 2,
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

/// Book file of a fetch.bible translation: `{bible_id}` and `{book}` filled
/// in by the installer.
pub const FETCH_BIBLE_BOOK_URL: &str = "https://v1.fetch.bible/bibles/{bible_id}/txt/{book}.json";

/// Books of `bible_id` in reading order, from the fetch.bible catalogue
/// cached in `cache_dir` by [`load_catalogue`].
///
/// # Errors
/// The catalogue is not cached or does not list `bible_id`.
pub fn catalogue_books(cache_dir: &Path, bible_id: &str) -> Result<Vec<String>> {
    let bytes = std::fs::read(cache_dir.join("manifest.json"))?;
    let catalogue: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| ImportError::Json {
            context: "catalogue".into(),
            source: e,
        })?;
    books_listed(&catalogue, bible_id).ok_or_else(|| ImportError::Format {
        reason: format!("{bible_id} is not in the catalogue"),
    })
}

/// `books_ordered`, kept to what `bibles.<id>.books_ot` / `books_nt` say:
/// `true` for the whole testament, or the ids it has. The first 39 of the
/// order are the Old Testament.
fn books_listed(catalogue: &serde_json::Value, bible_id: &str) -> Option<Vec<String>> {
    const OLD_TESTAMENT: usize = 39;
    let entry = catalogue.get("bibles")?.get(bible_id)?;
    let has = |key: &str, book: &str| match entry.get(key) {
        Some(serde_json::Value::Bool(all)) => *all,
        Some(serde_json::Value::Array(ids)) => ids.iter().any(|id| id.as_str() == Some(book)),
        _ => false,
    };
    let books: Vec<String> = catalogue
        .get("books_ordered")?
        .as_array()?
        .iter()
        .enumerate()
        .filter_map(|(index, book)| {
            let book = book.as_str()?;
            let testament = if index < OLD_TESTAMENT {
                "books_ot"
            } else {
                "books_nt"
            };
            has(testament, book).then(|| book.to_owned())
        })
        .collect();
    (!books.is_empty()).then_some(books)
}

/// Load the top-level manifest from `cache_dir/manifest.json`, fetching
/// via HTTP when the cache is stale. Returns the parsed value so the UI
/// can list languages / bibles for the picker.
#[cfg_attr(feature = "sismo", sismo::function("bible::import_load_catalogue"))]
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
    let cat: Catalogue = serde_json::from_slice(&bytes).map_err(|e| ImportError::Json {
        context: "catalogue".into(),
        source: e,
    })?;
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
    /// ISO 639-3 code of the language, the prefix of the id (`spa_rvr` is
    /// `spa`). What filters and defaults compare.
    pub language_code: String,
    /// Name of the language to show, in itself when known.
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

/// The ISO 639-3 prefix of a catalogue id.
fn language_code(id: &str) -> String {
    id.split('_').next().unwrap_or_default().to_owned()
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
                    s.as_str()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                };
                let name_local = name
                    .and_then(|n| n.get("local"))
                    .and_then(non_empty)
                    .or_else(|| name.and_then(|n| n.get("local_abbrev")).and_then(non_empty));
                let name_english = name
                    .and_then(|n| n.get("english"))
                    .and_then(non_empty)
                    .or_else(|| {
                        name.and_then(|n| n.get("english_abbrev"))
                            .and_then(non_empty)
                    });
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
                    language_code: language_code(id),
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
                language_code: language_code(extra.id),
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
#[cfg_attr(feature = "sismo", sismo::function("bible::import_fixtures"))]
pub(crate) fn populate_from_fixtures(
    db: &Db,
    bible_id: &str,
    desc: &serde_json::Value,
    books: &[(String, serde_json::Value)],
) -> Result<usize> {
    let desc_parsed: BibleDesc =
        serde_json::from_value(desc.clone()).map_err(|e| ImportError::Json {
            context: "desc fixture".into(),
            source: e,
        })?;

    let book_ids = desc_parsed.book_names.ids();

    db.with_conn_mut(|conn| {
        upsert_bible(conn, bible_id, &desc_parsed).map_err(crate::db::DbError::from)?;
        let tx = conn.transaction().map_err(crate::db::DbError::from)?;
        for (order_num, book_id) in book_ids.iter().enumerate() {
            upsert_book(
                &tx,
                bible_id,
                book_id,
                &desc_parsed,
                u16::try_from(order_num).unwrap_or(u16::MAX),
            )
            .map_err(crate::db::DbError::from)?;
        }
        tx.commit().map_err(crate::db::DbError::from)?;
        Ok(())
    })?;

    let mut inserted = 0usize;
    for (book_id, raw) in books {
        let book_doc: BookDoc =
            serde_json::from_value(raw.clone()).map_err(|e| ImportError::Json {
                context: format!("{book_id} fixture"),
                source: e,
            })?;
        let n = db.with_conn_mut(|conn| {
            let tx = conn.transaction().map_err(crate::db::DbError::from)?;
            let (n, _) = insert_book_contents(&tx, bible_id, book_id, &book_doc)
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

    #[test]
    fn books_keep_the_order_the_manifest_lists_them_in() {
        let desc: BibleDesc = serde_json::from_str(
            r#"{"book_names": {
                "gen": {"normal": "Génesis"},
                "exo": {"normal": "Éxodo"},
                "tob": {"normal": "Tobías"},
                "1ch": {"normal": "1 Crónicas"}
            }}"#,
        )
        .unwrap();
        assert_eq!(desc.book_names.ids(), ["gen", "exo", "tob", "1ch"]);
        assert_eq!(
            desc.book_names.get("tob").map(|b| b.normal.as_str()),
            Some("Tobías")
        );
        let written = serde_json::to_string(&desc.book_names).unwrap();
        assert!(written.find("gen") < written.find("1ch"), "{written}");
    }

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
        let n = populate_from_fixtures(&db, "spa_rv1960", &desc, &[("EPH".into(), book)])
            .expect("populate");
        assert_eq!(n, 9, "expected 9 verses inserted across chapters 1..6");

        let chap = db.load_chapter("spa_rv1960", "EPH", 6).unwrap();
        assert_eq!(chap.verses.len(), 4);
        assert_eq!(chap.verses[0].verse, 1);
        assert!(chap.verses[3].text.starts_with("Y vosotros"));
        assert!(chap.verses[3].paragraph_break);

        // Two source-section headings + one chapter-level heading.
        assert_eq!(chap.headers.len(), 3);
        assert!(
            chap.headers
                .iter()
                .any(|h| h.before_verse == 0 && h.text == "La armadura de Dios")
        );
        assert!(
            chap.headers
                .iter()
                .any(|h| h.before_verse == 1 && h.text == "Instrucciones a hijos y padres")
        );
        assert!(
            chap.headers
                .iter()
                .any(|h| h.before_verse == 5 && h.text == "Siervos y amos")
        );
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

    #[test]
    fn layout_levels_and_notes_from_a_rich_source() {
        let db = Db::in_memory().unwrap();
        let desc = json!({ "book_names": { "PSA": { "normal": "Salmos", "long": "Salmos", "abbrev": "Sal" } } });
        let mut chapters = vec![json!([])];
        for _ in 1..23 {
            chapters.push(json!([[], ["x"]]));
        }
        chapters.push(json!([
            [],
            [
                { "type": "heading", "contents": "Jehová es mi pastor", "level": 2 },
                { "type": "heading", "contents": "Salmo de David.", "level": 3 },
                "Jehová es mi pastor; nada me faltará.\n"
            ],
            ["En lugares de delicados pastos me hará descansar;\nJunto a aguas de reposo me pastoreará.", { "type": "note", "contents": "23.2 O, reposar." }, "\n"],
            ["Confortará mi alma;\nMe guiará por sendas de justicia por amor de su nombre.\n\n"],
            ["Aunque ande en valle de sombra de muerte,\nNo temeré mal alguno"]
        ]));
        let book = json!({ "book": "psa", "contents": chapters });
        populate_from_fixtures(&db, "spa_rv1960", &desc, &[("PSA".into(), book.clone())])
            .expect("populate");

        let chap = db.load_chapter("spa_rv1960", "PSA", 23).unwrap();
        let levels: Vec<_> = chap
            .headers
            .iter()
            .map(|h| (h.text.as_str(), h.level))
            .collect();
        assert_eq!(
            levels,
            [("Jehová es mi pastor", 2), ("Salmo de David.", 3)],
            "both, in order"
        );
        let v2 = &chap.verses[1];
        assert_eq!(
            v2.text,
            "En lugares de delicados pastos me hará descansar; Junto a aguas de reposo me pastoreará."
        );
        assert!(v2.line_start, "a poetry line after verse 1");
        assert_eq!(v2.line_breaks.len(), 1);
        assert_eq!(&v2.text[v2.line_breaks[0].at + 1..][..5], "Junto");
        assert!(chap.verses[3].paragraph_break, "a stanza after verse 3");
        let notes = db.footnotes_for_chapter("spa_rv1960", "PSA", 23).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].text, "O, reposar.");

        // Importing again replaces, never piles up.
        populate_from_fixtures(&db, "spa_rv1960", &desc, &[("PSA".into(), book)]).expect("again");
        assert_eq!(
            db.load_chapter("spa_rv1960", "PSA", 23)
                .unwrap()
                .headers
                .len(),
            2
        );
    }

    #[test]
    fn catalogue_books_follow_each_testament() {
        let mut order: Vec<String> = (0..39).map(|i| format!("ot{i}")).collect();
        order.extend(["mat".to_owned(), "mrk".to_owned()]);
        let catalogue = json!({
            "books_ordered": order,
            "bibles": {
                "eng_bsb": { "books_ot": true, "books_nt": true },
                "aab_wft": { "books_ot": [], "books_nt": ["mrk"] }
            }
        });
        assert_eq!(books_listed(&catalogue, "eng_bsb").unwrap().len(), 41);
        assert_eq!(books_listed(&catalogue, "aab_wft").unwrap(), ["mrk"]);
        assert!(books_listed(&catalogue, "missing").is_none());
    }

    /// Downloads from fetch.bible: `cargo test -p notin-bible --all-features -- --ignored fetch_bible`.
    #[test]
    #[ignore = "network"]
    fn fetch_bible_books_install_with_their_layout() {
        let db = Db::in_memory().unwrap();
        let cache = std::env::temp_dir().join(format!("notin-bible-fetch-{}", std::process::id()));
        let client = Client::builder()
            .user_agent("notin-bible/test")
            .build()
            .unwrap();
        let meta = BibleMeta::default();
        let report = Install {
            db: &db,
            client: &client,
            bible_id: "eng_bsb",
            cache_dir: &cache,
            books: Books::Listed(vec!["psa".into()]),
            books_url_template: FETCH_BIBLE_BOOK_URL,
            cache_ttl: Duration::from_secs(60),
            meta: &meta,
            refresh: None,
        }
        .run(|_| {})
        .expect("install");
        assert!(report.inserted > 2000 && report.headings && report.notes);
        let chapter = db.load_chapter("eng_bsb", "psa", 23).unwrap();
        assert!(
            chapter.headers.iter().any(|h| h.level == 3),
            "the psalm title"
        );
        assert!(chapter.verses.iter().any(|v| !v.line_breaks.is_empty()));
        assert_eq!(db.list_books("eng_bsb").unwrap()[0].name, "Psalm");
        let _ = std::fs::remove_dir_all(cache);
    }
}
