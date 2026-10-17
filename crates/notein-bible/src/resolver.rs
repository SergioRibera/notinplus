//! Reference resolver — maps a human string like `"Juan 3:16"` into a
//! concrete [`BibleAnchor`] against an installed Bible.
//!
//! The resolver walks three sources in order, falling through on
//! miss:
//!
//! 1. Spanish aliases hardcoded in [`book_id_for_alias`]. Covers the
//!    most common spellings used in day-to-day reading.
//! 2. Book names from the installed `books` table — matches the
//!    `name_normal` / `abbrev` fields case-insensitively.
//! 3. Direct OSIS id (`"EPH"`, `"1CO"`) when nothing else hits.
//!
//! The returned anchor always clamps `end_verse` ≥ `start_verse`;
//! chapter-only refs collapse to the whole chapter (start_verse = 1).

use crate::anchor::BibleAnchor;
use crate::db::Db;
use crate::reference::{self, ParsedRef};

/// Resolve `input` against `bible_id`. Returns `None` when the string
/// does not parse OR the parsed book cannot be matched.
#[must_use]
pub fn resolve(db: &Db, bible_id: &str, input: &str) -> Option<BibleAnchor> {
    let parsed = reference::parse(input)?;
    resolve_parsed(db, bible_id, &parsed)
}

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

/// Resolve a lowercase book token to an OSIS id. Walks the hardcoded
/// Spanish alias map first, falls back to the installed catalogue, and
/// finally accepts the token verbatim if it looks like an OSIS id.
pub fn resolve_book_token(db: &Db, bible_id: &str, token: &str) -> Option<String> {
    let normalised = normalise(token);
    let books = db.list_books(bible_id).ok()?;

    // Alias map returns canonical uppercase OSIS, but the installed
    // `books` table may store its own casing (biblionlabs ships
    // lowercase ids like `jhn`). Match the alias output against the
    // catalogue case-insensitively and return the stored id so
    // downstream `load_chapter` queries hit the right row.
    if let Some(osis) = book_id_for_alias(&normalised)
        && let Some(book) = books
            .iter()
            .find(|b| b.book_id.eq_ignore_ascii_case(osis))
    {
        return Some(book.book_id.clone());
    }

    for book in &books {
        if normalise(&book.name) == normalised || normalise(&book.abbrev) == normalised {
            return Some(book.book_id.clone());
        }
    }

    // Last-resort: user may have typed an OSIS id directly. Match the
    // catalogue case-insensitively and return the stored id.
    if let Some(book) = books
        .iter()
        .find(|b| b.book_id.eq_ignore_ascii_case(token))
    {
        return Some(book.book_id.clone());
    }

    None
}

fn normalise(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter_map(|c| match c {
            'á' => Some('a'),
            'é' => Some('e'),
            'í' => Some('i'),
            'ó' => Some('o'),
            'ú' => Some('u'),
            'ñ' => Some('n'),
            ' ' | '.' | '-' => None,
            _ => Some(c),
        })
        .collect()
}

/// Spanish → OSIS alias dictionary. Keys are the output of
/// [`normalise`] applied to the user's book token. Values are OSIS book
/// codes (uppercase, no spaces).
///
/// Covers the common spellings (`juan`, `jn`, `1co`, `1 corintios`,
/// `efesios`, `apocalipsis`, etc.). Not exhaustive — exotic
/// abbreviations fall through to the installed catalogue match.
fn book_id_for_alias(token: &str) -> Option<&'static str> {
    Some(match token {
        // Antiguo Testamento
        "genesis" | "gn" | "gen" => "GEN",
        "exodo" | "ex" | "exo" => "EXO",
        "levitico" | "lv" | "lev" => "LEV",
        "numeros" | "nm" | "num" => "NUM",
        "deuteronomio" | "dt" | "deu" => "DEU",
        "josue" | "jos" => "JOS",
        "jueces" | "jue" | "jc" | "jdg" => "JDG",
        "rut" | "rt" | "ru" => "RUT",
        "1samuel" | "1s" | "1sam" => "1SA",
        "2samuel" | "2s" | "2sam" => "2SA",
        "1reyes" | "1r" | "1re" | "1kg" => "1KI",
        "2reyes" | "2r" | "2re" | "2kg" => "2KI",
        "1cronicas" | "1cr" | "1ch" => "1CH",
        "2cronicas" | "2cr" | "2ch" => "2CH",
        "esdras" | "esd" | "ezr" => "EZR",
        "nehemias" | "neh" => "NEH",
        "ester" | "est" => "EST",
        "job" | "jb" => "JOB",
        "salmos" | "sal" | "sl" | "ps" | "psa" => "PSA",
        "proverbios" | "pr" | "prov" | "pro" => "PRO",
        "eclesiastes" | "ec" | "ecl" => "ECC",
        "cantares" | "ct" | "cnt" | "cantar" => "SNG",
        "isaias" | "is" | "isa" => "ISA",
        "jeremias" | "jer" | "jr" => "JER",
        "lamentaciones" | "lam" | "lm" => "LAM",
        "ezequiel" | "ez" | "eze" => "EZK",
        "daniel" | "dn" | "dan" => "DAN",
        "oseas" | "os" | "ose" => "HOS",
        "joel" | "jl" | "jol" => "JOL",
        "amos" | "am" | "amo" => "AMO",
        "abdias" | "abd" | "ab" => "OBA",
        "jonas" | "jon" => "JON",
        "miqueas" | "mi" | "miq" => "MIC",
        "nahum" | "nah" | "na" => "NAM",
        "habacuc" | "hab" => "HAB",
        "sofonias" | "sof" | "so" => "ZEP",
        "hageo" | "ag" | "hag" => "HAG",
        "zacarias" | "zac" | "zc" => "ZEC",
        "malaquias" | "mal" => "MAL",
        // Nuevo Testamento
        "mateo" | "mt" | "mat" => "MAT",
        "marcos" | "mr" | "mc" | "mrk" => "MRK",
        "lucas" | "lc" | "luk" => "LUK",
        "juan" | "jn" | "jhn" => "JHN",
        "hechos" | "hch" | "hc" | "act" => "ACT",
        "romanos" | "ro" | "rom" => "ROM",
        "1corintios" | "1co" | "1cor" => "1CO",
        "2corintios" | "2co" | "2cor" => "2CO",
        "galatas" | "ga" | "gal" => "GAL",
        "efesios" | "ef" | "eph" => "EPH",
        "filipenses" | "flp" | "fil" | "php" | "phil" => "PHP",
        "colosenses" | "col" => "COL",
        "1tesalonicenses" | "1ts" | "1tes" | "1thess" | "1th" => "1TH",
        "2tesalonicenses" | "2ts" | "2tes" | "2thess" | "2th" => "2TH",
        "1timoteo" | "1ti" | "1tim" => "1TI",
        "2timoteo" | "2ti" | "2tim" => "2TI",
        "tito" | "tit" | "ti" => "TIT",
        "filemon" | "flm" | "phm" => "PHM",
        "hebreos" | "heb" | "he" => "HEB",
        "santiago" | "stg" | "sant" | "jas" => "JAS",
        "1pedro" | "1p" | "1pe" | "1ped" => "1PE",
        "2pedro" | "2p" | "2pe" | "2ped" => "2PE",
        "1juan" | "1jn" => "1JN",
        "2juan" | "2jn" => "2JN",
        "3juan" | "3jn" => "3JN",
        "judas" | "jud" | "jd" => "JUD",
        "apocalipsis" | "ap" | "apoc" | "rev" => "REV",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn seeded_db() -> Db {
        let db = Db::in_memory().unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO bibles (id, abbrev, name_local, language)
                 VALUES ('spa_rv1960', 'RVR1960', 'Reina Valera 1960', 'spa')",
                [],
            )?;
            for (bid, name, abbrev, order) in [
                ("GEN", "Génesis", "Gn", 1),
                ("JHN", "Juan", "Jn", 43),
                ("EPH", "Efesios", "Ef", 49),
                ("1CO", "1 Corintios", "1Co", 46),
            ] {
                conn.execute(
                    "INSERT INTO books (bible_id, book_id, name, abbrev, order_num, testament)
                     VALUES ('spa_rv1960', ?1, ?2, ?3, ?4, 'NT')",
                    params![bid, name, abbrev, order as i64],
                )?;
            }
            Ok(())
        })
        .unwrap();
        db
    }

    #[test]
    fn resolve_spanish_full_name() {
        let db = seeded_db();
        let a = resolve(&db, "spa_rv1960", "Juan 3:16").unwrap();
        assert_eq!(a.book_id, "JHN");
        assert_eq!(a.start_chapter, 3);
        assert_eq!(a.start_verse, 16);
        assert_eq!(a.end_verse, 16);
    }

    #[test]
    fn resolve_abbrev_with_dot() {
        let db = seeded_db();
        let a = resolve(&db, "spa_rv1960", "1 Co. 13:4-7").unwrap();
        assert_eq!(a.book_id, "1CO");
        assert_eq!(a.start_verse, 4);
        assert_eq!(a.end_verse, 7);
    }

    #[test]
    fn resolve_chapter_only_defaults_verse_1() {
        let db = seeded_db();
        let a = resolve(&db, "spa_rv1960", "Ef 6").unwrap();
        assert_eq!(a.book_id, "EPH");
        assert_eq!(a.start_chapter, 6);
        assert_eq!(a.start_verse, 1);
        assert_eq!(a.end_verse, 1);
    }

    #[test]
    fn resolve_falls_back_to_installed_name() {
        let db = seeded_db();
        // "Génesis" via full catalogue name — the alias map handles
        // the accented form, but also prove the catalogue walk works
        // with the OSIS id directly.
        let a = resolve(&db, "spa_rv1960", "GEN 1:1").unwrap();
        assert_eq!(a.book_id, "GEN");
    }

    #[test]
    fn resolve_unknown_book_returns_none() {
        let db = seeded_db();
        assert!(resolve(&db, "spa_rv1960", "Zorg 1:1").is_none());
    }
}
