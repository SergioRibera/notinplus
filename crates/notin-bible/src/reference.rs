//! Human reference parser — `"Juan 3:16"`, `"1 Co 13:4-7"`, `"Ef 6"`.
//!
//! Covers Spanish book names + canonical abbreviations. The parser is
//! permissive on whitespace + optional dots after abbreviations
//! (`"1 Co."` → `"1Co"`), case-insensitive on the book, and tolerates
//! both `":"` and `"."` between chapter and verse (`"Juan 3.16"` works
//! too — common in some `LatAm` typographic traditions).
//!
//! Returns a `ParsedRef` that the resolver can hand off to the DB layer
//! for anchor materialisation. The parser does not touch the DB; book
//! disambiguation happens later against the installed catalogue so that
//! unknown book names surface as a clean parse-failure, not a false
//! positive.

use regex::Regex;
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedRef {
    /// Raw book token as the user typed it (trimmed, lowercased). The
    /// resolver maps this against known abbreviations.
    pub book_token: String,
    pub chapter: u16,
    pub start_verse: Option<u16>,
    pub end_verse: Option<u16>,
}

/// `(\d+\s+)?([a-záéíóúñ.]+)\s+(\d+)(?:[:.](\d+)(?:\s*[-–]\s*(\d+))?)?`
/// `None` only if the pattern failed to build, which tests rule out.
fn re() -> Option<&'static Regex> {
    static R: OnceLock<Option<Regex>> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?xi)
             ^\s*
             (?P<prefix>\d+\s*)?          # optional ordinal (1, 2, 3) for 1 Co, 2 Reyes…
             (?P<book>[a-záéíóúñ.]+)      # book token (with optional trailing dot)
             \s+
             (?P<chapter>\d+)
             (?:
                [:.](?P<verse_start>\d+)   # verse separator : or .
                (?:
                    \s*[-–]\s*
                    (?P<verse_end>\d+)     # range end
                )?
             )?
             \s*$
            ",
        )
        .ok()
    })
    .as_ref()
}

#[must_use]
pub fn parse(input: &str) -> Option<ParsedRef> {
    let caps = re()?.captures(input)?;
    let prefix = caps.name("prefix").map(|m| m.as_str().trim());
    let book_raw = caps.name("book")?.as_str().trim_end_matches('.');
    let book_token = prefix.map_or_else(
        || book_raw.to_lowercase(),
        |p| format!("{p}{}", book_raw.to_lowercase()),
    );
    let chapter: u16 = caps.name("chapter")?.as_str().parse().ok()?;
    let start_verse = caps
        .name("verse_start")
        .and_then(|m| m.as_str().parse().ok());
    let end_verse = caps.name("verse_end").and_then(|m| m.as_str().parse().ok());
    Some(ParsedRef {
        book_token,
        chapter,
        start_verse,
        end_verse,
    })
}

/// The passages a heading names, when it is a list of parallels.
///
/// `(Mt. 26.59-66; Mr. 14.55-64)`, `(Lc. 3.1-9,15-17)`,
/// `(Nm. 28.16—29.40)`. Each comes back with its text, for showing,
/// and where it starts. `None` unless every entry reads as a reference,
/// so a psalm title such as `Salmo de David.` stays plain text.
#[must_use]
pub fn passages(heading: &str) -> Option<Vec<(String, ParsedRef)>> {
    let inner = heading.trim().strip_prefix('(')?.strip_suffix(')')?;
    let mut out = Vec::new();
    for entry in inner.split(';') {
        let entry = entry.trim();
        // Where it starts is enough to jump: drop further ranges and the
        // end of a range that crosses chapters.
        let start = entry
            .split(',')
            .next()?
            .split(['\u{2014}', '\u{2013}'])
            .next()?
            .trim();
        out.push((entry.to_owned(), parse(start)?));
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallels_under_a_heading() {
        let refs = passages("(Mt. 26.59-66; Mr. 14.55-64; 1 R. 3.3-15)").expect("parallels");
        let labels: Vec<_> = refs.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(labels, ["Mt. 26.59-66", "Mr. 14.55-64", "1 R. 3.3-15"]);
        assert_eq!(refs[1].1.book_token, "mr");
        assert_eq!((refs[1].1.chapter, refs[1].1.start_verse), (14, Some(55)));
        assert_eq!(refs[2].1.book_token, "1r");
        let crossing = passages("(Nm. 28.16\u{2014}29.40)").expect("one");
        assert_eq!(
            (crossing[0].1.chapter, crossing[0].1.start_verse),
            (28, Some(16))
        );
        let listed = passages("(Lc. 3.1-9,15-17)").expect("one");
        assert_eq!(listed[0].1.end_verse, Some(9));
        assert!(passages("Salmo de David.").is_none());
        assert!(passages("(al músico principal)").is_none());
    }

    #[test]
    fn single_verse() {
        let r = parse("Juan 3:16").unwrap();
        assert_eq!(r.book_token, "juan");
        assert_eq!(r.chapter, 3);
        assert_eq!(r.start_verse, Some(16));
        assert_eq!(r.end_verse, None);
    }

    #[test]
    fn ordinal_book_range() {
        let r = parse("1 Co 13:4-7").unwrap();
        assert_eq!(r.book_token, "1co");
        assert_eq!(r.chapter, 13);
        assert_eq!(r.start_verse, Some(4));
        assert_eq!(r.end_verse, Some(7));
    }

    #[test]
    fn chapter_only() {
        let r = parse("Ef 6").unwrap();
        assert_eq!(r.book_token, "ef");
        assert_eq!(r.chapter, 6);
        assert_eq!(r.start_verse, None);
    }

    #[test]
    fn dot_separator() {
        let r = parse("Juan 3.16").unwrap();
        assert_eq!(r.start_verse, Some(16));
    }

    #[test]
    fn trailing_dot_in_abbrev() {
        let r = parse("1 Co. 13:4").unwrap();
        assert_eq!(r.book_token, "1co");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("random words").is_none());
        assert!(parse("Juan 3:").is_none());
    }
}
