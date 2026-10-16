//! Canonical book ordering keyed by OSIS 3-letter book id.
//!
//! The importer sorts books alphabetically when writing them into the
//! DB, so `BookRow::order_num` reflects alphabetical rather than
//! canonical order (Genesis, Exodus, Leviticus… → actually ends up as
//! 1CH, 1CO, 1JN, … under the alphabetical path). Any UI that wants
//! the familiar Protestant canon ordering sorts by [`canonical_order`]
//! instead.
//!
//! The table covers the 66 Protestant OSIS ids. Deuterocanonical /
//! apocryphal books, and any publisher-invented ids, fall back to
//! `u16::MAX` so they trail the canonical sort — present but after
//! the recognised books.

/// Canonical 1-based position for a Protestant-canon OSIS book id,
/// or `u16::MAX` for unknown / deuterocanonical ids.
#[must_use]
pub fn canonical_order(book_id: &str) -> u16 {
    // ASCII-uppercase so publisher-cased ids ("Gen", "gen") resolve.
    let upper: [u8; 8] = {
        let mut buf = [0u8; 8];
        let bytes = book_id.as_bytes();
        for (i, b) in bytes.iter().take(8).enumerate() {
            buf[i] = b.to_ascii_uppercase();
        }
        buf
    };
    let key = std::str::from_utf8(&upper)
        .unwrap_or(book_id)
        .trim_end_matches('\0');
    match key {
        "GEN" => 1,
        "EXO" => 2,
        "LEV" => 3,
        "NUM" => 4,
        "DEU" => 5,
        "JOS" => 6,
        "JDG" => 7,
        "RUT" => 8,
        "1SA" => 9,
        "2SA" => 10,
        "1KI" => 11,
        "2KI" => 12,
        "1CH" => 13,
        "2CH" => 14,
        "EZR" => 15,
        "NEH" => 16,
        "EST" => 17,
        "JOB" => 18,
        "PSA" => 19,
        "PRO" => 20,
        "ECC" => 21,
        "SNG" => 22,
        "ISA" => 23,
        "JER" => 24,
        "LAM" => 25,
        "EZK" => 26,
        "DAN" => 27,
        "HOS" => 28,
        "JOL" => 29,
        "AMO" => 30,
        "OBA" => 31,
        "JON" => 32,
        "MIC" => 33,
        "NAM" => 34,
        "HAB" => 35,
        "ZEP" => 36,
        "HAG" => 37,
        "ZEC" => 38,
        "MAL" => 39,
        "MAT" => 40,
        "MRK" => 41,
        "LUK" => 42,
        "JHN" => 43,
        "ACT" => 44,
        "ROM" => 45,
        "1CO" => 46,
        "2CO" => 47,
        "GAL" => 48,
        "EPH" => 49,
        "PHP" => 50,
        "COL" => 51,
        "1TH" => 52,
        "2TH" => 53,
        "1TI" => 54,
        "2TI" => 55,
        "TIT" => 56,
        "PHM" => 57,
        "HEB" => 58,
        "JAS" => 59,
        "1PE" => 60,
        "2PE" => 61,
        "1JN" => 62,
        "2JN" => 63,
        "3JN" => 64,
        "JUD" => 65,
        "REV" => 66,
        _ => u16::MAX,
    }
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
}
