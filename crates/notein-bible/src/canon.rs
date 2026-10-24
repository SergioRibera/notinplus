//! The Protestant canon, keyed by OSIS 3-letter book id.
//!
//! The importer stores books alphabetically, so `BookRow::order_num` is
//! not the canonical order; any list of books sorts by
//! [`canonical_order`] instead. Deuterocanonical and publisher-invented
//! ids are not in the table: they sort last and keep their id as name.

/// `(OSIS id, Spanish short name)` in canonical order.
const BOOKS: [(&str, &str); 66] = [
    ("GEN", "Gn"),
    ("EXO", "Ex"),
    ("LEV", "Lv"),
    ("NUM", "Nm"),
    ("DEU", "Dt"),
    ("JOS", "Jos"),
    ("JDG", "Jue"),
    ("RUT", "Rt"),
    ("1SA", "1 Sm"),
    ("2SA", "2 Sm"),
    ("1KI", "1 R"),
    ("2KI", "2 R"),
    ("1CH", "1 Cr"),
    ("2CH", "2 Cr"),
    ("EZR", "Esd"),
    ("NEH", "Neh"),
    ("EST", "Est"),
    ("JOB", "Job"),
    ("PSA", "Sal"),
    ("PRO", "Pr"),
    ("ECC", "Ec"),
    ("SNG", "Cnt"),
    ("ISA", "Is"),
    ("JER", "Jer"),
    ("LAM", "Lm"),
    ("EZK", "Ez"),
    ("DAN", "Dn"),
    ("HOS", "Os"),
    ("JOL", "Jl"),
    ("AMO", "Am"),
    ("OBA", "Abd"),
    ("JON", "Jon"),
    ("MIC", "Miq"),
    ("NAM", "Nah"),
    ("HAB", "Hab"),
    ("ZEP", "Sof"),
    ("HAG", "Hag"),
    ("ZEC", "Zac"),
    ("MAL", "Mal"),
    ("MAT", "Mt"),
    ("MRK", "Mc"),
    ("LUK", "Lc"),
    ("JHN", "Jn"),
    ("ACT", "Hch"),
    ("ROM", "Ro"),
    ("1CO", "1 Co"),
    ("2CO", "2 Co"),
    ("GAL", "Gá"),
    ("EPH", "Ef"),
    ("PHP", "Flp"),
    ("COL", "Col"),
    ("1TH", "1 Ts"),
    ("2TH", "2 Ts"),
    ("1TI", "1 Ti"),
    ("2TI", "2 Ti"),
    ("TIT", "Tit"),
    ("PHM", "Flm"),
    ("HEB", "Heb"),
    ("JAS", "Stg"),
    ("1PE", "1 P"),
    ("2PE", "2 P"),
    ("1JN", "1 Jn"),
    ("2JN", "2 Jn"),
    ("3JN", "3 Jn"),
    ("JUD", "Jud"),
    ("REV", "Ap"),
];

/// Index of `book_id` (any case) in [`BOOKS`].
fn position(book_id: &str) -> Option<usize> {
    BOOKS
        .iter()
        .position(|(id, _)| id.eq_ignore_ascii_case(book_id))
}

/// Canonical 1-based position of `book_id` (any case), or `u16::MAX`
/// for ids outside the Protestant canon.
#[must_use]
pub fn canonical_order(book_id: &str) -> u16 {
    position(book_id)
        .and_then(|i| u16::try_from(i + 1).ok())
        .unwrap_or(u16::MAX)
}

/// Short Spanish name for references ("Jn 3:16"); unknown ids are
/// returned as given.
#[must_use]
pub fn short_name(book_id: &str) -> &str {
    position(book_id).map_or(book_id, |i| BOOKS[i].1)
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
        assert_eq!(short_name("jhn"), "Jn");
        assert_eq!(short_name("1CO"), "1 Co");
        assert_eq!(short_name("TOB"), "TOB");
    }
}
