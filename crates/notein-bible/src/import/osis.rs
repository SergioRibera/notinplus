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
/// header and paragraph break discovered. Returns the number of verses
/// inserted. All rows land under `bible_id` — the caller controls that
/// id (OSIS files don't always carry a stable translation code).
pub fn import_osis_xml(db: &Db, bible_id: &str, xml_bytes: &[u8]) -> Result<usize> {
    let parsed = parse(xml_bytes)?;
    materialise(db, bible_id, parsed)
}

/// Same as [`import_osis_xml`] but reads from any `BufRead`. Good for
/// large files you want to stream from disk without a full `Vec<u8>`.
pub fn import_osis_reader<R: BufRead>(db: &Db, bible_id: &str, reader: R) -> Result<usize> {
    let parsed = parse_from_reader(reader)?;
    materialise(db, bible_id, parsed)
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

/// Core XML walk. State machine over the subset of OSIS we support.
fn walk<R: BufRead>(reader: &mut Reader<R>) -> Result<Parsed> {
    reader.config_mut().trim_text(false);

    let mut out = Parsed::default();
    let mut current_book: Option<ParsedBook> = None;
    let mut current_chapter: u16 = 0;
    let mut active_verse: Option<ActiveVerse> = None;
    let mut pending_headers: Vec<ParsedHeader> = Vec::new();
    let mut collecting_title: Option<String> = None;
    let mut collecting_name: bool = false;
    let mut skip_depth: u32 = 0;
    let mut pending_paragraph: bool = false;

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf)? {
            Event::Start(ref e) => {
                if skip_depth > 0 {
                    skip_depth += 1;
                    continue;
                }
                let name = e.name();
                match name_bytes(name) {
                    b"div" => {
                        if attr(e, b"type")?.as_deref() == Some("book") {
                            if let Some(book) = current_book.take() {
                                out.books.push(book);
                            }
                            let id = attr(e, b"osisID")?.unwrap_or_default();
                            current_book = Some(ParsedBook { id, ..Default::default() });
                            current_chapter = 0;
                            active_verse = None;
                            pending_headers.clear();
                            pending_paragraph = false;
                        }
                    }
                    b"chapter" => {
                        // Container form; osisID like `Gen.1`.
                        if let Some(id) = attr(e, b"osisID")? {
                            current_chapter = parse_chapter_num(&id).unwrap_or(current_chapter);
                        } else if let Some(n) = attr(e, b"n")? {
                            current_chapter = n.parse().unwrap_or(current_chapter);
                        }
                        active_verse = None;
                    }
                    b"verse" => {
                        if let Some(id) = attr(e, b"osisID")? {
                            if let Some((chap, verse)) = parse_verse_num(&id) {
                                current_chapter = chap;
                                active_verse = Some(ActiveVerse {
                                    chapter: chap,
                                    verse,
                                    buf: String::new(),
                                    paragraph_break: pending_paragraph,
                                });
                                pending_paragraph = false;
                            }
                        }
                    }
                    b"title" if active_verse.is_some() => {
                        // Inline title inside a verse container — rare but
                        // valid. Capture into pending_headers but still mark
                        // active to flush when the next verse opens.
                        collecting_title = Some(String::new());
                    }
                    b"title" => {
                        let kind = attr(e, b"type")?;
                        if kind.as_deref() == Some("section")
                            || kind.as_deref() == Some("chapter")
                            || kind.is_none()
                        {
                            collecting_title = Some(String::new());
                        }
                    }
                    b"p" => {
                        pending_paragraph = true;
                    }
                    b"note" => {
                        // Footnote apparatus — skip entire subtree.
                        skip_depth = 1;
                    }
                    b"name" if current_book.is_some() && current_book.as_ref().unwrap().name.is_none() => {
                        collecting_name = true;
                    }
                    _ => {}
                }
            }
            Event::Empty(ref e) => {
                if skip_depth > 0 {
                    continue;
                }
                match name_bytes(e.name()) {
                    b"chapter" => {
                        // Milestone shape — sID starts a chapter, eID ends it.
                        if let Some(id) = attr(e, b"sID")?.or(attr(e, b"osisID")?) {
                            current_chapter =
                                parse_chapter_num(&id).unwrap_or(current_chapter);
                        }
                    }
                    b"verse" => {
                        if let Some(sid) = attr(e, b"sID")? {
                            if let Some((chap, verse)) = parse_verse_num(&sid) {
                                current_chapter = chap;
                                active_verse = Some(ActiveVerse {
                                    chapter: chap,
                                    verse,
                                    buf: String::new(),
                                    paragraph_break: pending_paragraph,
                                });
                                pending_paragraph = false;
                            }
                        } else if let Some(eid) = attr(e, b"eID")? {
                            close_verse(&mut current_book, &mut active_verse, &mut pending_headers, eid.as_str());
                        } else if let Some(oid) = attr(e, b"osisID")? {
                            // Pure milestone-less self-close is rare but
                            // valid when a verse carries no body.
                            if let Some((chap, verse)) = parse_verse_num(&oid) {
                                if let Some(book) = current_book.as_mut() {
                                    book.verses.push(ParsedVerse {
                                        chapter: chap,
                                        verse,
                                        text: String::new(),
                                        paragraph_break: pending_paragraph,
                                    });
                                }
                                pending_paragraph = false;
                            }
                        }
                    }
                    b"lb" => {
                        let kind = attr(e, b"type")?;
                        if kind.as_deref() == Some("x-begin-paragraph") {
                            pending_paragraph = true;
                        }
                    }
                    b"p" => {
                        pending_paragraph = true;
                    }
                    _ => {}
                }
            }
            Event::End(ref e) => {
                if skip_depth > 0 {
                    skip_depth -= 1;
                    continue;
                }
                match name_bytes(e.name()) {
                    b"verse" => {
                        close_verse(&mut current_book, &mut active_verse, &mut pending_headers, "");
                    }
                    b"title" => {
                        if let Some(text) = collecting_title.take() {
                            let trimmed = text.trim().to_string();
                            if !trimmed.is_empty() {
                                pending_headers.push(ParsedHeader {
                                    chapter: current_chapter,
                                    before_verse: 0,
                                    text: trimmed,
                                });
                            }
                        }
                    }
                    b"name" => {
                        collecting_name = false;
                    }
                    b"div" => {
                        if let Some(book) = current_book.take() {
                            out.books.push(book);
                        }
                        current_chapter = 0;
                        active_verse = None;
                        pending_headers.clear();
                        pending_paragraph = false;
                    }
                    _ => {}
                }
            }
            Event::Text(e) => {
                if skip_depth > 0 {
                    continue;
                }
                let text = e.unescape()?.into_owned();
                if collecting_name {
                    if let Some(book) = current_book.as_mut() {
                        let name = book.name.get_or_insert_with(String::new);
                        name.push_str(text.trim());
                    }
                    continue;
                }
                if let Some(buf) = collecting_title.as_mut() {
                    buf.push_str(&text);
                    continue;
                }
                if let Some(active) = active_verse.as_mut() {
                    append_text(&mut active.buf, &text);
                }
            }
            Event::CData(e) => {
                if skip_depth > 0 {
                    continue;
                }
                let text = String::from_utf8_lossy(e.as_ref()).into_owned();
                if let Some(active) = active_verse.as_mut() {
                    append_text(&mut active.buf, &text);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    if let Some(book) = current_book.take() {
        out.books.push(book);
    }

    Ok(out)
}

#[derive(Debug)]
struct ActiveVerse {
    chapter: u16,
    verse: u16,
    buf: String,
    paragraph_break: bool,
}

fn close_verse(
    current_book: &mut Option<ParsedBook>,
    active_verse: &mut Option<ActiveVerse>,
    pending_headers: &mut Vec<ParsedHeader>,
    _eid: &str,
) {
    let Some(av) = active_verse.take() else { return };
    let Some(book) = current_book.as_mut() else { return };
    for mut hdr in pending_headers.drain(..) {
        hdr.before_verse = av.verse;
        hdr.chapter = av.chapter;
        book.headers.push(hdr);
    }
    book.verses.push(ParsedVerse {
        chapter: av.chapter,
        verse: av.verse,
        text: av.buf.trim().to_string(),
        paragraph_break: av.paragraph_break,
    });
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

fn materialise(db: &Db, bible_id: &str, parsed: Parsed) -> Result<usize> {
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
                    order_num as i64,
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
                        v.chapter as i64,
                        v.verse as i64,
                        &v.text,
                        v.paragraph_break as i64,
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
                        h.chapter as i64,
                        h.before_verse as i64,
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
