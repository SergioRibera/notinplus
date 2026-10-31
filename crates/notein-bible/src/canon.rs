//! Short book names, keyed by OSIS 3-letter book id.
//!
//! Used for references to books the open translation lacks; ids
//! outside this table keep their id as name. Book order always comes
//! from the translation (`BookRow::order_num`, the manifest order).

/// OSIS ids with a short name.
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
    fn short_names_fall_back_to_the_id() {
        notein_i18n::Lang::Es.make_current();
        assert_eq!(short_name("jhn"), "Jn");
        assert_eq!(short_name("1CO"), "1 Co");
        assert_eq!(short_name("TOB"), "TOB");
    }
}
