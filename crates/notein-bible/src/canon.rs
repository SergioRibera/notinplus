//! The Protestant canon, keyed by OSIS 3-letter book id.
//!
//! The importer stores books alphabetically, so `BookRow::order_num` is
//! not the canonical order; any list of books sorts by
//! [`canonical_order`] instead. Deuterocanonical and publisher-invented
//! ids are not in the table: they sort last and keep their id as name.

/// OSIS ids in canonical order.
const BOOKS: [&str; 66] = [
    "GEN", "EXO", "LEV", "NUM", "DEU", "JOS", "JDG", "RUT", "1SA", "2SA", "1KI", "2KI", "1CH",
    "2CH", "EZR", "NEH", "EST", "JOB", "PSA", "PRO", "ECC", "SNG", "ISA", "JER", "LAM", "EZK",
    "DAN", "HOS", "JOL", "AMO", "OBA", "JON", "MIC", "NAM", "HAB", "ZEP", "HAG", "ZEC", "MAL",
    "MAT", "MRK", "LUK", "JHN", "ACT", "ROM", "1CO", "2CO", "GAL", "EPH", "PHP", "COL", "1TH",
    "2TH", "1TI", "2TI", "TIT", "PHM", "HEB", "JAS", "1PE", "2PE", "1JN", "2JN", "3JN", "JUD",
    "REV",
];

/// Index of `book_id` (any case) in [`BOOKS`].
fn position(book_id: &str) -> Option<usize> {
    BOOKS.iter().position(|id| id.eq_ignore_ascii_case(book_id))
}

/// Canonical 1-based position of `book_id` (any case), or `u16::MAX`
/// for ids outside the Protestant canon.
#[must_use]
pub fn canonical_order(book_id: &str) -> u16 {
    position(book_id)
        .and_then(|i| u16::try_from(i + 1).ok())
        .unwrap_or(u16::MAX)
}

/// Short name for references ("Jn 3:16") in the interface language;
/// unknown ids are returned as given.
#[must_use]
pub fn short_name(book_id: &str) -> String {
    position(book_id).map_or_else(
        || book_id.to_owned(),
        |i| {
            let key = format!("canon-short-{}", BOOKS[i].to_ascii_lowercase());
            crate::LOCALES.text(&key)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_order_matches_protestant_canon() {
        assert_eq!(canonical_order("GEN"), 1);
        assert_eq!(canonical_order("REV"), 66);
        assert_eq!(canonical_order("MAT"), 40);
        assert_eq!(canonical_order("PSA"), 19);
    }

    #[test]
    fn canonical_order_case_insensitive() {
        assert_eq!(canonical_order("gen"), 1);
        assert_eq!(canonical_order("Gen"), 1);
    }

    #[test]
    fn canonical_order_unknown_trails() {
        assert_eq!(canonical_order("TOB"), u16::MAX);
        assert_eq!(canonical_order("XYZ"), u16::MAX);
    }

    #[test]
    fn short_names_fall_back_to_the_id() {
        notein_i18n::Lang::Es.make_current();
        assert_eq!(short_name("jhn"), "Jn");
        assert_eq!(short_name("1CO"), "1 Co");
        assert_eq!(short_name("TOB"), "TOB");
    }
}
