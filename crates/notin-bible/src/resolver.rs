//! Reference resolver — maps a human string like `"Juan 3:16"` into a
//! concrete [`BibleAnchor`] against one installed Bible.
//!
//! Book names are data of the translation being referenced, never of
//! the interface language or of another installed Bible: "John" finds
//! nothing in a Spanish translation, "Juan" nothing in an English one.
//! A book token matches, in order:
//!
//! 1. The translation's book name or abbreviation.
//! 2. The OSIS id (`"EPH"`, `"1co"`), the same in every translation.
//! 3. The abbreviations the translation's language writes in notes and
//!    parallel passages (`"Lc."`, `"1 R."` in a Spanish one).
//! 4. The start of exactly one of the translation's book names
//!    (`"Efe"`, `"1 Cor"`); an ambiguous start matches nothing.
//!
//! Comparison ignores case, accents, spaces, dots and dashes. The
//! returned anchor always clamps `end_verse` ≥ `start_verse`;
//! chapter-only refs collapse to the whole chapter (`start_verse` = 1).

use notin_sdk::fold;

use crate::anchor::BibleAnchor;
use crate::db::Db;
use crate::reference::{self, ParsedRef};

/// Resolve `input` against `bible_id`. Returns `None` when the string
/// does not parse OR the parsed book cannot be matched.
#[must_use]
#[cfg_attr(feature = "sismo", sismo::function("bible::resolve"))]
pub fn resolve(db: &Db, bible_id: &str, input: &str) -> Option<BibleAnchor> {
    let parsed = reference::parse(input)?;
    resolve_parsed(db, bible_id, &parsed)
}

#[must_use]
#[cfg_attr(feature = "sismo", sismo::function("bible::resolve_parsed"))]
pub fn resolve_parsed(db: &Db, bible_id: &str, parsed: &ParsedRef) -> Option<BibleAnchor> {
    let book_id = resolve_book_token(db, bible_id, &parsed.book_token)?;
    let start_verse = parsed.start_verse.unwrap_or(1);
    let end_verse = parsed.end_verse.unwrap_or(start_verse);
    Some(BibleAnchor {
        bible_id: bible_id.into(),
        book_id,
        start_chapter: parsed.chapter,
        start_verse,
        end_chapter: parsed.chapter,
        end_verse,
        start_char: None,
        end_char: None,
    })
}

/// Stored id of the book of `bible_id` that `token` names. The id keeps
/// the translation's own casing (biblionlabs ships lowercase `jhn`) so
/// chapter queries hit the right row.
#[must_use]
#[cfg_attr(feature = "sismo", sismo::function("bible::resolve_book"))]
pub fn resolve_book_token(db: &Db, bible_id: &str, token: &str) -> Option<String> {
    let wanted = book_key(token);
    if wanted.is_empty() {
        return None;
    }
    let books = db.list_books(bible_id).ok()?;
    let exact = books.iter().find(|book| {
        book_key(&book.name) == wanted
            || book_key(&book.abbrev) == wanted
            || book.book_id.eq_ignore_ascii_case(&wanted)
    });
    if let Some(book) = exact {
        return Some(book.book_id.clone());
    }
    if let Some(id) = conventional_abbreviation(bible_id, &wanted)
        && let Some(book) = books
            .iter()
            .find(|book| book.book_id.eq_ignore_ascii_case(id))
    {
        return Some(book.book_id.clone());
    }
    let mut starts = books
        .iter()
        .filter(|book| book_key(&book.name).starts_with(&wanted));
    match (starts.next(), starts.next()) {
        (Some(book), None) => Some(book.book_id.clone()),
        _ => None,
    }
}

/// The book id `key` ([`book_key`] of an abbreviation) stands for in the
/// notes of a translation in the language of `bible_id` (its id starts
/// with the ISO 639-3 code).
fn conventional_abbreviation(bible_id: &str, key: &str) -> Option<&'static str> {
    // Reina-Valera usage, shared by most Spanish editions.
    const SPANISH: &[(&str, &str)] = &[
        ("gn", "gen"),
        ("ex", "exo"),
        ("lv", "lev"),
        ("nm", "num"),
        ("dt", "deu"),
        ("jos", "jos"),
        ("jue", "jdg"),
        ("rt", "rut"),
        ("1s", "1sa"),
        ("2s", "2sa"),
        ("1r", "1ki"),
        ("2r", "2ki"),
        ("1cr", "1ch"),
        ("2cr", "2ch"),
        ("esd", "ezr"),
        ("neh", "neh"),
        ("est", "est"),
        ("job", "job"),
        ("sal", "psa"),
        ("pr", "pro"),
        ("ec", "ecc"),
        ("cnt", "sng"),
        ("is", "isa"),
        ("jer", "jer"),
        ("lm", "lam"),
        ("ez", "ezk"),
        ("dn", "dan"),
        ("os", "hos"),
        ("jl", "jol"),
        ("am", "amo"),
        ("abd", "oba"),
        ("jon", "jon"),
        ("mi", "mic"),
        ("nah", "nam"),
        ("hab", "hab"),
        ("sof", "zep"),
        ("hag", "hag"),
        ("zac", "zec"),
        ("mal", "mal"),
        ("mt", "mat"),
        ("mr", "mrk"),
        ("lc", "luk"),
        ("jn", "jhn"),
        ("hch", "act"),
        ("ro", "rom"),
        ("1co", "1co"),
        ("2co", "2co"),
        ("ga", "gal"),
        ("ef", "eph"),
        ("fil", "php"),
        ("col", "col"),
        ("1ts", "1th"),
        ("2ts", "2th"),
        ("1ti", "1ti"),
        ("2ti", "2ti"),
        ("tit", "tit"),
        ("flm", "phm"),
        ("he", "heb"),
        ("stg", "jas"),
        ("1p", "1pe"),
        ("2p", "2pe"),
        ("1jn", "1jn"),
        ("2jn", "2jn"),
        ("3jn", "3jn"),
        ("jud", "jud"),
        ("ap", "rev"),
    ];
    let table = match bible_id.split('_').next()? {
        "spa" => SPANISH,
        _ => return None,
    };
    table
        .iter()
        .find(|(abbreviation, _)| *abbreviation == key)
        .map(|(_, id)| *id)
}

/// `text` folded for comparing book names: lower case, no accents,
/// letters and digits only.
fn book_key(text: &str) -> String {
    fold(text).chars().filter(|c| c.is_alphanumeric()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    /// A Spanish and an English translation, with abbreviations cut the
    /// way the biblionlabs manifests ship them.
    fn seeded_db() -> Db {
        let db = Db::in_memory().unwrap();
        db.with_conn(|conn| {
            for (id, abbrev, name, language) in [
                ("spa_rv1960", "RVR1960", "Reina Valera 1960", "spa"),
                ("eng_kjv", "KJV", "King James Version", "eng"),
            ] {
                conn.execute(
                    "INSERT INTO bibles (id, abbrev, name_local, language)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![id, abbrev, name, language],
                )?;
            }
            for (bible, bid, name, abbrev, order) in [
                ("spa_rv1960", "gen", "Génesis", "Génesi", 0),
                ("spa_rv1960", "jos", "Josué", "Josué", 5),
                ("spa_rv1960", "job", "Job", "Job", 17),
                ("spa_rv1960", "jhn", "Juan", "Juan", 42),
                ("spa_rv1960", "1co", "1 Corintios", "1 Cori", 45),
                ("spa_rv1960", "eph", "Efesios", "Efesio", 48),
                ("spa_rv1960", "1jn", "1 Juan", "1 Juan", 61),
                ("eng_kjv", "jhn", "John", "John", 42),
                ("eng_kjv", "eph", "Ephesians", "Ephesi", 48),
            ] {
                conn.execute(
                    "INSERT INTO books (bible_id, book_id, name, abbrev, order_num, testament)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'NT')",
                    params![bible, bid, name, abbrev, i64::from(order)],
                )?;
            }
            Ok(())
        })
        .unwrap();
        db
    }

    #[test]
    fn full_name_of_the_referenced_translation() {
        let db = seeded_db();
        let a = resolve(&db, "spa_rv1960", "Juan 3:16").unwrap();
        assert_eq!(a.book_id, "jhn");
        assert_eq!(a.start_chapter, 3);
        assert_eq!(a.start_verse, 16);
        assert_eq!(a.end_verse, 16);
        assert_eq!(resolve(&db, "eng_kjv", "John 3:16").unwrap().book_id, "jhn");
    }

    #[test]
    fn abbreviations_of_the_translations_language() {
        let db = seeded_db();
        let a = resolve(&db, "spa_rv1960", "Jn. 3.16").unwrap();
        assert_eq!(
            (a.book_id.as_str(), a.start_chapter, a.start_verse),
            ("jhn", 3, 16)
        );
        assert_eq!(
            resolve(&db, "spa_rv1960", "1 Co. 13.4").unwrap().book_id,
            "1co"
        );
        assert!(
            resolve(&db, "eng_kjv", "Jn. 3.16").is_none(),
            "Spanish usage only"
        );
    }

    #[test]
    fn names_never_cross_translations() {
        let db = seeded_db();
        assert!(resolve(&db, "spa_rv1960", "John 3:16").is_none());
        assert!(resolve(&db, "eng_kjv", "Juan 3:16").is_none());
    }

    #[test]
    fn accents_case_and_dots_are_ignored() {
        let db = seeded_db();
        assert_eq!(
            resolve(&db, "spa_rv1960", "genesis 1:1").unwrap().book_id,
            "gen"
        );
        let a = resolve(&db, "spa_rv1960", "1 Cori. 13:4-7").unwrap();
        assert_eq!(a.book_id, "1co");
        assert_eq!(a.start_verse, 4);
        assert_eq!(a.end_verse, 7);
    }

    #[test]
    fn osis_id_works_in_any_translation() {
        let db = seeded_db();
        assert_eq!(
            resolve(&db, "spa_rv1960", "EPH 2:8").unwrap().book_id,
            "eph"
        );
        assert_eq!(resolve(&db, "eng_kjv", "1co 13").map(|a| a.book_id), None);
        assert_eq!(resolve(&db, "spa_rv1960", "1co 13").unwrap().book_id, "1co");
    }

    #[test]
    fn unique_start_of_a_name_matches() {
        let db = seeded_db();
        let a = resolve(&db, "spa_rv1960", "Ef 6").unwrap();
        assert_eq!(a.book_id, "eph");
        assert_eq!(a.start_chapter, 6);
        assert_eq!(a.start_verse, 1);
        assert_eq!(
            resolve(&db, "spa_rv1960", "1 Cor 13").unwrap().book_id,
            "1co"
        );
        assert_eq!(resolve(&db, "eng_kjv", "Eph 2").unwrap().book_id, "eph");
    }

    #[test]
    fn ambiguous_start_matches_nothing() {
        let db = seeded_db();
        // Josué, Job and Juan all start with "j".
        assert!(resolve(&db, "spa_rv1960", "J 1").is_none());
        assert!(resolve(&db, "spa_rv1960", "Jo 1").is_none());
    }

    #[test]
    fn unknown_book_returns_none() {
        let db = seeded_db();
        assert!(resolve(&db, "spa_rv1960", "Zorg 1:1").is_none());
    }
}
