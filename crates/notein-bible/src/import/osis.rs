//! OSIS XML import.
//!
//! OSIS files come in two broad shapes:
//!
//! - **Container**: `<verse osisID="Gen.1.1">In the beginning …</verse>`
//! - **Milestone**: `<verse sID="Gen.1.1"/>In the beginning …<verse eID="Gen.1.1"/>`
//!
//! The parser handles both. Everything is a stream of events — large
//! files (>10 MB is common) never fully materialise in memory. State is
//! kept per (book, chapter, verse); text runs between the active open +
//! close verse markers are concatenated into one row per verse.
//!
//! Supported element whitelist:
//!
//! - `osisText` / `div type="book"` — book container, osisID = book code.
//! - `chapter` — container or milestone shape, osisID like `Gen.1`.
//! - `verse` — container or milestone shape, osisID like `Gen.1.1`.
//! - `title type="section"` / `title type="chapter"` → headers table.
//! - `p` / `lb type="x-begin-paragraph"` → paragraph break flag.
//! - `note` → footnotes, skipped entirely (reader apparatus).
//! - Anything else: text content is kept, tags are ignored.
//!
//! Books not reachable from `osisID` fall through — they'd store under
//! an empty id, which is useless. The parser logs + skips.

use std::io::BufRead;

use quick_xml::events::{BytesStart, Event};
use quick_xml::name::QName;
use quick_xml::Reader;
use rusqlite::params;

use crate::db::Db;

#[derive(Debug)]
pub enum OsisError {
    Io(std::io::Error),
    Xml(quick_xml::Error),
    Db(crate::db::DbError),
    Format { reason: String },
}

impl std::fmt::Display for OsisError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "osis io: {e}"),
            Self::Xml(e) => write!(f, "osis xml: {e}"),
            Self::Db(e) => write!(f, "osis db: {e}"),
            Self::Format { reason } => write!(f, "osis format: {reason}"),
        }
    }
}

impl std::error::Error for OsisError {}

impl From<std::io::Error> for OsisError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<quick_xml::Error> for OsisError {
    fn from(e: quick_xml::Error) -> Self {
        Self::Xml(e)
    }
}

impl From<quick_xml::events::attributes::AttrError> for OsisError {
    fn from(e: quick_xml::events::attributes::AttrError) -> Self {
        Self::Xml(quick_xml::Error::InvalidAttr(e))
    }
}

impl From<crate::db::DbError> for OsisError {
    fn from(e: crate::db::DbError) -> Self {
        Self::Db(e)
    }
}

pub type Result<T> = std::result::Result<T, OsisError>;

/// Parse a complete OSIS XML blob + populate `db` with every verse,
/// header and paragraph break discovered.
///
/// Returns the number of verses inserted. All rows land under `bible_id` —
/// the caller controls that id (OSIS files don't always carry a stable
/// translation code).
pub fn import_osis_xml(db: &Db, bible_id: &str, xml_bytes: &[u8]) -> Result<usize> {
    let parsed = parse(xml_bytes)?;
    materialise(db, bible_id, &parsed)
}

/// Same as [`import_osis_xml`] but reads from any `BufRead`. Good for
/// large files you want to stream from disk without a full `Vec<u8>`.
pub fn import_osis_reader<R: BufRead>(db: &Db, bible_id: &str, reader: R) -> Result<usize> {
    let parsed = parse_from_reader(reader)?;
    materialise(db, bible_id, &parsed)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Parsed {
    books: Vec<ParsedBook>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ParsedBook {
    id: String,
    name: Option<String>,
    verses: Vec<ParsedVerse>,
    headers: Vec<ParsedHeader>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ParsedVerse {
    chapter: u16,
    verse: u16,
    text: String,
    paragraph_break: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ParsedHeader {
    chapter: u16,
    before_verse: u16,
    text: String,
}

fn parse(bytes: &[u8]) -> Result<Parsed> {
    let mut reader = Reader::from_reader(bytes);
    walk(&mut reader)
}

fn parse_from_reader<R: BufRead>(reader: R) -> Result<Parsed> {
    let mut reader = Reader::from_reader(reader);
    walk(&mut reader)
}

/// Core XML walk over the subset of OSIS we support.
fn walk<R: BufRead>(reader: &mut Reader<R>) -> Result<Parsed> {
    reader.config_mut().trim_text(false);
    let mut walker = Walker::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(ref e) => walker.start(e)?,
            Event::Empty(ref e) => walker.empty(e)?,
            Event::End(ref e) => walker.end(name_bytes(e.name())),
            Event::Text(e) if walker.skip_depth == 0 => walker.text(&e.unescape()?),
            Event::CData(e) if walker.skip_depth == 0 => {
                walker.cdata(&String::from_utf8_lossy(e.as_ref()));
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(walker.finish())
}

/// State of the walk between XML events.
#[derive(Debug, Default)]
struct Walker {
    out: Parsed,
    book: Option<ParsedBook>,
    chapter: u16,
    verse: Option<ActiveVerse>,
    /// Headings seen since the last verse; they attach to the next one.
    pending_headers: Vec<ParsedHeader>,
    /// Text of the `<title>` being read.
    title: Option<String>,
    /// Reading the book's `<name>`.
    in_name: bool,
    /// Depth inside a skipped subtree (footnotes).
    skip_depth: u32,
    /// The next verse starts a paragraph.
    pending_paragraph: bool,
}

impl Walker {
    fn start(&mut self, e: &BytesStart<'_>) -> Result<()> {
        if self.skip_depth > 0 {
            self.skip_depth += 1;
            return Ok(());
        }
        match name_bytes(e.name()) {
            b"div" if attr(e, b"type")?.as_deref() == Some("book") => {
                self.finish_book();
                let id = attr(e, b"osisID")?.unwrap_or_default();
                self.book = Some(ParsedBook {
                    id,
                    ..Default::default()
                });
            }
            b"chapter" => {
                // Container form; osisID like `Gen.1`.
                if let Some(id) = attr(e, b"osisID")? {
                    self.chapter = parse_chapter_num(&id).unwrap_or(self.chapter);
                } else if let Some(n) = attr(e, b"n")? {
                    self.chapter = n.parse().unwrap_or(self.chapter);
                }
                self.verse = None;
            }
            b"verse" => {
                if let Some((chapter, verse)) =
                    attr(e, b"osisID")?.and_then(|id| parse_verse_num(&id))
                {
                    self.open_verse(chapter, verse);
                }
            }
            // A title inside a verse is rare but valid; it is kept as a
            // heading of the next verse.
            b"title" if self.verse.is_some() => self.title = Some(String::new()),
            b"title" => {
                let kind = attr(e, b"type")?;
                if matches!(kind.as_deref(), None | Some("section" | "chapter")) {
                    self.title = Some(String::new());
                }
            }
            b"p" => self.pending_paragraph = true,
            // Footnote apparatus: skip the whole subtree.
            b"note" => self.skip_depth = 1,
            b"name" if self.book.as_ref().is_some_and(|b| b.name.is_none()) => self.in_name = true,
            _ => {}
        }
        Ok(())
    }

    /// Self-closing elements: milestone chapters and verses, line breaks.
    fn empty(&mut self, e: &BytesStart<'_>) -> Result<()> {
        if self.skip_depth > 0 {
            return Ok(());
        }
        match name_bytes(e.name()) {
            b"chapter" => {
                if let Some(id) = attr(e, b"sID")?.or(attr(e, b"osisID")?) {
                    self.chapter = parse_chapter_num(&id).unwrap_or(self.chapter);
                }
            }
            b"verse" => {
                if let Some(sid) = attr(e, b"sID")? {
                    if let Some((chapter, verse)) = parse_verse_num(&sid) {
                        self.open_verse(chapter, verse);
                    }
                } else if attr(e, b"eID")?.is_some() {
                    self.close_verse();
                } else if let Some((chapter, verse)) =
                    attr(e, b"osisID")?.and_then(|id| parse_verse_num(&id))
                {
                    // A verse with no body.
                    if let Some(book) = self.book.as_mut() {
                        book.verses.push(ParsedVerse {
                            chapter,
                            verse,
                            text: String::new(),
                            paragraph_break: self.pending_paragraph,
                        });
                    }
                    self.pending_paragraph = false;
                }
            }
            b"lb" if attr(e, b"type")?.as_deref() == Some("x-begin-paragraph") => {
                self.pending_paragraph = true;
            }
            b"p" => self.pending_paragraph = true,
            _ => {}
        }
        Ok(())
    }

    fn end(&mut self, name: &[u8]) {
        if self.skip_depth > 0 {
            self.skip_depth -= 1;
            return;
        }
        match name {
            b"verse" => self.close_verse(),
            b"title" => {
                if let Some(text) = self.title.take() {
                    let text = text.trim();
                    if !text.is_empty() {
                        self.pending_headers.push(ParsedHeader {
                            chapter: self.chapter,
                            before_verse: 0,
                            text: text.to_owned(),
                        });
                    }
                }
            }
            b"name" => self.in_name = false,
            b"div" => self.finish_book(),
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        if self.in_name {
            if let Some(book) = self.book.as_mut() {
                book.name.get_or_insert_with(String::new).push_str(text.trim());
            }
        } else if let Some(title) = self.title.as_mut() {
            title.push_str(text);
        } else {
            self.cdata(text);
        }
    }

    fn cdata(&mut self, text: &str) {
        if let Some(active) = self.verse.as_mut() {
            append_text(&mut active.buf, text);
        }
    }

    fn open_verse(&mut self, chapter: u16, verse: u16) {
        self.chapter = chapter;
        self.verse = Some(ActiveVerse {
            chapter,
            verse,
            buf: String::new(),
            paragraph_break: self.pending_paragraph,
        });
        self.pending_paragraph = false;
    }

    /// Store the open verse, with the headings that precede it.
    fn close_verse(&mut self) {
        let Some(active) = self.verse.take() else {
            return;
        };
        let Some(book) = self.book.as_mut() else {
            return;
        };
        for mut header in self.pending_headers.drain(..) {
            header.before_verse = active.verse;
            header.chapter = active.chapter;
            book.headers.push(header);
        }
        book.verses.push(ParsedVerse {
            chapter: active.chapter,
            verse: active.verse,
            text: active.buf.trim().to_owned(),
            paragraph_break: active.paragraph_break,
        });
    }

    fn finish_book(&mut self) {
        if let Some(book) = self.book.take() {
            self.out.books.push(book);
        }
        self.chapter = 0;
        self.verse = None;
        self.pending_headers.clear();
        self.pending_paragraph = false;
    }

    fn finish(mut self) -> Parsed {
        if let Some(book) = self.book.take() {
            self.out.books.push(book);
        }
        self.out
    }
}

#[derive(Debug)]
struct ActiveVerse {
    chapter: u16,
    verse: u16,
    buf: String,
    paragraph_break: bool,
}

fn append_text(buf: &mut String, text: &str) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        if !buf.is_empty() && !buf.ends_with(' ') {
            buf.push(' ');
        }
        return;
    }
    if !buf.is_empty() && !buf.ends_with(' ') {
        buf.push(' ');
    }
    buf.push_str(trimmed);
}

fn name_bytes(name: QName<'_>) -> &[u8] {
    name.local_name().into_inner()
}

fn attr(e: &BytesStart<'_>, key: &[u8]) -> Result<Option<String>> {
    for attribute in e.attributes() {
        let a = attribute?;
        if a.key.local_name().into_inner() == key {
            let v = a.unescape_value()?.into_owned();
            return Ok(Some(v));
        }
    }
    Ok(None)
}

fn parse_chapter_num(osis_id: &str) -> Option<u16> {
    // osisID forms: `Gen.1` (book.chapter) or `Gen.1.1` (book.chapter.verse)
    let mut parts = osis_id.split('.');
    let _book = parts.next()?;
    parts.next()?.parse().ok()
}

fn parse_verse_num(osis_id: &str) -> Option<(u16, u16)> {
    let mut parts = osis_id.split('.');
    let _book = parts.next()?;
    let chap = parts.next()?.parse().ok()?;
    let verse_token = parts.next()?;
    // Handle verse ranges like `Gen.1.1-Gen.1.3` → use the first number.
    let verse = verse_token
        .split('-')
        .next()?
        .parse()
        .ok()?;
    Some((chap, verse))
}

fn materialise(db: &Db, bible_id: &str, parsed: &Parsed) -> Result<usize> {
    let mut inserted = 0usize;

    db.with_conn_mut(|conn| {
        conn.execute(
            "INSERT INTO bibles (id, abbrev, name_local, language) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET abbrev = excluded.abbrev,
                                           name_local = excluded.name_local,
                                           language = excluded.language",
            params![bible_id, bible_id, bible_id, "und"],
        )
        .map_err(crate::db::DbError::from)?;

        let tx = conn.transaction().map_err(crate::db::DbError::from)?;
        for (order_num, book) in parsed.books.iter().enumerate() {
            if book.id.is_empty() {
                log::warn!("osis: skipping book with empty osisID");
                continue;
            }
            let name = book.name.clone().unwrap_or_else(|| book.id.clone());
            tx.execute(
                "INSERT INTO books (bible_id, book_id, name, abbrev, order_num, testament)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(bible_id, book_id) DO UPDATE SET
                     name = excluded.name,
                     abbrev = excluded.abbrev,
                     order_num = excluded.order_num,
                     testament = excluded.testament",
                params![
                    bible_id,
                    &book.id,
                    &name,
                    &book.id,
                    i64::try_from(order_num).unwrap_or(i64::MAX),
                    testament_for(&book.id),
                ],
            )
            .map_err(crate::db::DbError::from)?;

            for v in &book.verses {
                tx.execute(
                    "INSERT INTO verses (bible_id, book_id, chapter, verse, text, paragraph_break)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(bible_id, book_id, chapter, verse) DO UPDATE SET
                         text = excluded.text,
                         paragraph_break = excluded.paragraph_break",
                    params![
                        bible_id,
                        &book.id,
                        i64::from(v.chapter),
                        i64::from(v.verse),
                        &v.text,
                        i64::from(v.paragraph_break),
                    ],
                )
                .map_err(crate::db::DbError::from)?;
                inserted += 1;
            }
            for h in &book.headers {
                tx.execute(
                    "INSERT INTO headers (bible_id, book_id, chapter, before_verse, text)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        bible_id,
                        &book.id,
                        i64::from(h.chapter),
                        i64::from(h.before_verse),
                        &h.text,
                    ],
                )
                .map_err(crate::db::DbError::from)?;
            }
        }
        tx.commit().map_err(crate::db::DbError::from)?;
        Ok(())
    })?;

    Ok(inserted)
}

/// Minimal OSIS → testament lookup. Mirrors the biblionlabs side but
/// kept local to avoid pulling the HTTP-feature-gated module into the
/// always-on path.
fn testament_for(book_id: &str) -> &'static str {
    const NT: &[&str] = &[
        "Matt", "Mark", "Luke", "John", "Acts", "Rom", "1Cor", "2Cor", "Gal", "Eph", "Phil",
        "Col", "1Thess", "2Thess", "1Tim", "2Tim", "Titus", "Phlm", "Heb", "Jas", "1Pet",
        "2Pet", "1John", "2John", "3John", "Jude", "Rev",
    ];
    if NT.contains(&book_id) {
        "NT"
    } else {
        "OT"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL_OSIS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<osis>
<osisText osisIDWork="Smoke">
  <div type="book" osisID="Eph">
    <title type="section">La armadura de Dios</title>
    <chapter osisID="Eph.6">
      <title type="section">Instrucciones a hijos y padres</title>
      <verse osisID="Eph.6.1">Hijos, obedeced en el Señor a vuestros padres, porque esto es justo.</verse>
      <verse osisID="Eph.6.2">Honra a tu padre y a tu madre, que es el primer mandamiento con promesa;</verse>
      <verse osisID="Eph.6.3">para que te vaya bien, y seas de larga vida sobre la tierra.</verse>
      <p/>
      <verse osisID="Eph.6.4">Y vosotros, padres, no provoquéis a ira a vuestros hijos<note>Footnote dropped</note>.</verse>
    </chapter>
  </div>
</osisText>
</osis>"#;

    const MILESTONE_OSIS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<osis>
<osisText>
  <div type="book" osisID="Gen">
    <chapter sID="Gen.1"/>
    <verse sID="Gen.1.1"/>En el principio creó Dios los cielos y la tierra.<verse eID="Gen.1.1"/>
    <verse sID="Gen.1.2"/>Y la tierra estaba desordenada y vacía.<verse eID="Gen.1.2"/>
    <chapter eID="Gen.1"/>
  </div>
</osisText>
</osis>"#;

    #[test]
    fn container_shape_round_trips() {
        let db = Db::in_memory().unwrap();
        let n = import_osis_xml(&db, "spa_test", SMALL_OSIS.as_bytes()).unwrap();
        assert_eq!(n, 4);
        let chap = db.load_chapter("spa_test", "Eph", 6).unwrap();
        assert_eq!(chap.verses.len(), 4);
        assert!(chap.verses[0].text.starts_with("Hijos"));
        assert!(chap.verses[3].text.contains("provoquéis"));
        assert!(
            !chap.verses[3].text.contains("Footnote"),
            "note subtree must be dropped"
        );
        assert!(chap.verses[3].paragraph_break);
        assert!(chap.headers.iter().any(|h| h.text == "La armadura de Dios"));
        assert!(chap.headers.iter().any(|h| h.text == "Instrucciones a hijos y padres"));
    }

    #[test]
    fn milestone_shape_round_trips() {
        let db = Db::in_memory().unwrap();
        let n = import_osis_xml(&db, "spa_test", MILESTONE_OSIS.as_bytes()).unwrap();
        assert_eq!(n, 2);
        let chap = db.load_chapter("spa_test", "Gen", 1).unwrap();
        assert_eq!(chap.verses.len(), 2);
        assert!(chap.verses[0].text.starts_with("En el principio"));
    }

    #[test]
    fn fts_search_hits_osis_imported_verse() {
        let db = Db::in_memory().unwrap();
        import_osis_xml(&db, "spa_test", SMALL_OSIS.as_bytes()).unwrap();
        let hits = db.search(Some("spa_test"), "senor", 10).unwrap();
        assert!(!hits.is_empty());
    }
}
