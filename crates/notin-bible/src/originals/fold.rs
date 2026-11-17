//! Text of the original languages folded for search.
//!
//! Lowercase, without accents, breathings, vowel points or cantillation,
//! so «makar», «μακαρ» and «μακάρ» all find μακάριος, and «bara» finds
//! בָּרָא through its transliteration.

/// `text` folded for matching (see the module docs).
#[must_use]
pub fn fold(text: &str) -> String {
    text.chars()
        .flat_map(char::to_lowercase)
        .filter_map(base)
        .collect()
}

/// The letter `ch` stands for, `None` for marks that carry no letter.
fn base(ch: char) -> Option<char> {
    let code = u32::from(ch);
    // Hebrew points and cantillation; the letters themselves stay.
    if (0x0591..=0x05C7).contains(&code) {
        return None;
    }
    if (0x1F00..=0x1FFF).contains(&code) {
        return greek_extended(code);
    }
    Some(match ch {
        // Monotonic Greek with tonos or diaeresis.
        'ά' => 'α',
        'έ' => 'ε',
        'ή' => 'η',
        'ί' | 'ϊ' | 'ΐ' => 'ι',
        'ό' => 'ο',
        'ύ' | 'ϋ' | 'ΰ' => 'υ',
        'ώ' => 'ω',
        'ς' => 'σ',
        // Transliterations.
        'á' | 'à' | 'â' | 'ä' | 'ā' | 'ă' => 'a',
        'é' | 'è' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ə' => 'e',
        'í' | 'ì' | 'î' | 'ï' | 'ī' | 'ĭ' => 'i',
        'ó' | 'ò' | 'ô' | 'ö' | 'ō' | 'ŏ' => 'o',
        'ú' | 'ù' | 'û' | 'ü' | 'ū' | 'ŭ' => 'u',
        'š' | 'ś' | 'ṣ' => 's',
        'ṭ' | 'ṯ' => 't',
        'ḥ' | 'ḫ' => 'h',
        'ḇ' => 'b',
        'ḡ' => 'g',
        'ḏ' => 'd',
        'ḵ' => 'k',
        'ṗ' => 'p',
        'ñ' => 'n',
        // Aleph, ayin and syllable marks carry no letter to type.
        'ʾ' | 'ʿ' | '\'' | '’' | '.' | '-' | '\u{0301}' | '\u{0300}' | '\u{0342}' | '\u{0313}'
        | '\u{0314}' | '\u{0345}' | '\u{0308}' => return None,
        other => other,
    })
}

/// The bare letter of a character of the Greek Extended block (polytonic
/// letters with breathings, accents and iota subscript).
const fn greek_extended(code: u32) -> Option<char> {
    let letter = match code {
        0x1F00..=0x1F0F | 0x1F70..=0x1F71 | 0x1F80..=0x1F8F | 0x1FB0..=0x1FBC => 'α',
        0x1F10..=0x1F1D | 0x1F72..=0x1F73 | 0x1FC8..=0x1FC9 => 'ε',
        0x1F20..=0x1F2F | 0x1F74..=0x1F75 | 0x1F90..=0x1F9F | 0x1FC2..=0x1FC7 | 0x1FCA..=0x1FCC => {
            'η'
        }
        0x1F30..=0x1F3F | 0x1F76..=0x1F77 | 0x1FD0..=0x1FDB => 'ι',
        0x1F40..=0x1F4D | 0x1F78..=0x1F79 | 0x1FF8..=0x1FF9 => 'ο',
        0x1F50..=0x1F5F | 0x1F7A..=0x1F7B | 0x1FE0..=0x1FE3 | 0x1FE6..=0x1FEB => 'υ',
        0x1FE4..=0x1FE5 | 0x1FEC => 'ρ',
        0x1F60..=0x1F6F | 0x1F7C..=0x1F7D | 0x1FA0..=0x1FAF | 0x1FF2..=0x1FF7 | 0x1FFA..=0x1FFC => {
            'ω'
        }
        // Lone accents and breathings.
        _ => return None,
    };
    Some(letter)
}

#[cfg(test)]
mod tests {
    use super::fold;

    #[test]
    fn greek_with_its_marks_folds_to_bare_letters() {
        assert_eq!(fold("Μακάριοι"), "μακαριοι");
        assert_eq!(fold("ἀγάπη"), "αγαπη");
        assert_eq!(fold("οὐρανῶν"), "ουρανων");
        assert_eq!(fold("τῷ"), "τω");
        assert_eq!(fold("λόγος"), "λογοσ");
    }

    #[test]
    fn hebrew_loses_its_points() {
        assert_eq!(fold("בָּרָא"), "ברא");
        assert_eq!(fold("אֱלֹהִים"), "אלהים");
    }

    #[test]
    fn transliterations_fold_to_what_one_types() {
        assert_eq!(fold("makários"), "makarios");
        assert_eq!(fold("bārāʾ"), "bara");
        assert_eq!(fold("ʾĕlōhîm"), "elohim");
        assert_eq!(fold("haššāmayim"), "hassamayim");
    }
}
