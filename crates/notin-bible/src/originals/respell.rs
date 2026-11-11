//! Pronunciation respellings a reader of the interface language can say
//! aloud: `be.re.Shit` → "bereshít", `μακάριος` → "makários".
//!
//! Spanish gets its own rules: the acute accent already marks the
//! stressed syllable, so no apostrophes or hyphens are needed. English
//! keeps Strong's respelling (`mak-ar'-ee-os`), which was written for
//! English readers.

use notin_i18n::Lang;

/// The respelling of a Hebrew word for `lang`.
///
/// `translit` is `STEPBible`'s syllabified transliteration, dots between
/// syllables and the stressed one capitalised (`'E.lo.Him`: a name also
/// capitalises its first syllable, so the last capital wins); when no
/// syllable is capitalised (the lexicon's dictionary forms), the stress
/// comes from `strong_pronunciation` (`kheh'-sed`, the apostrophe after
/// the stressed syllable) and, failing that, falls on the last syllable
/// as Hebrew mostly does.
#[must_use]
pub fn hebrew(lang: Lang, translit: &str, strong_pronunciation: &str) -> String {
    match lang {
        Lang::Es => hebrew_spanish(translit, strong_pronunciation),
        Lang::En => {
            if strong_pronunciation.is_empty() {
                translit.replace('.', "-")
            } else {
                strong_pronunciation.to_owned()
            }
        }
    }
}

/// The respelling of a Greek word for `lang`, from its accented
/// spelling.
#[must_use]
pub fn greek(lang: Lang, word: &str, strong_pronunciation: &str) -> String {
    match lang {
        Lang::Es => greek_spanish(word),
        Lang::En => {
            if strong_pronunciation.is_empty() {
                greek_spanish(word)
            } else {
                strong_pronunciation.to_owned()
            }
        }
    }
}

fn hebrew_spanish(translit: &str, strong_pronunciation: &str) -> String {
    let syllables: Vec<&str> = translit.split('.').filter(|s| !s.is_empty()).collect();
    if syllables.is_empty() {
        return String::new();
    }
    let stressed = syllables
        .iter()
        .rposition(|s| s.chars().any(char::is_uppercase))
        .or_else(|| strong_stress(strong_pronunciation, syllables.len()))
        .unwrap_or(syllables.len() - 1);
    let mut out = String::new();
    for (i, syllable) in syllables.iter().enumerate() {
        let mut spelled = hebrew_letters(&syllable.to_lowercase());
        // A final he after a vowel is silent (`tzi.Vah` → tsivá).
        let last = i + 1 == syllables.len();
        if last && (spelled.ends_with("ah") || spelled.ends_with("eh") || spelled.ends_with("oh")) {
            spelled.pop();
        }
        if i == stressed && syllables.len() > 1 {
            out.push_str(&accent_first_vowel(&spelled));
        } else {
            out.push_str(&spelled);
        }
    }
    out
}

/// Index of the syllable Strong marks with an apostrophe, when the
/// syllable count matches.
fn strong_stress(pronunciation: &str, count: usize) -> Option<usize> {
    let parts: Vec<&str> = pronunciation.split('-').collect();
    if parts.len() != count {
        return None;
    }
    parts.iter().position(|p| p.contains('\''))
}

/// `STEPBible`'s consonant digraphs in Spanish letters.
fn hebrew_letters(syllable: &str) -> String {
    let mut out = String::with_capacity(syllable.len());
    let chars: Vec<char> = syllable.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let pair = (chars[i], chars.get(i + 1).copied());
        let (text, used) = match pair {
            ('k' | 'c', Some('h')) => ("j", 2),
            ('s', Some('h')) => ("sh", 2),
            ('t', Some('z' | 's')) => ("ts", 2),
            ('t', Some('h')) => ("t", 2),
            ('p', Some('h')) => ("f", 2),
            ('\'' | '-', _) => ("", 1),
            ('w', _) => ("v", 1),
            ('q', _) => ("k", 1),
            (c, _) => {
                out.push(c);
                i += 1;
                continue;
            }
        };
        out.push_str(text);
        i += used;
    }
    out
}

/// The first vowel of a syllable takes the acute accent; a syllable
/// without one stays as it is.
fn accent_first_vowel(syllable: &str) -> String {
    let mut out = String::with_capacity(syllable.len() + 1);
    let mut done = false;
    for c in syllable.chars() {
        let accented = match c {
            'a' if !done => Some('á'),
            'e' if !done => Some('é'),
            'i' if !done => Some('í'),
            'o' if !done => Some('ó'),
            'u' if !done => Some('ú'),
            _ => None,
        };
        match accented {
            Some(a) => {
                out.push(a);
                done = true;
            }
            None => out.push(c),
        }
    }
    out
}

/// Greek letters in the Erasmian reading Spanish grammars teach, the
/// accented vowel marked with a Spanish acute.
fn greek_spanish(word: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    // Decompose so accents become separate marks after their vowel.
    let decomposed: Vec<char> = word.nfd().collect();
    let mut letters: Vec<(char, bool)> = Vec::new();
    for c in decomposed {
        match c {
            // acute, grave, circumflex (perispomeni) mark the stress
            '\u{0301}' | '\u{0300}' | '\u{0342}' | '\u{0344}' => {
                if let Some(last) = letters.last_mut() {
                    last.1 = true;
                }
            }
            // breathings, diaeresis, iota subscript: no sound a reader needs
            '\u{0313}' | '\u{0314}' | '\u{0308}' | '\u{0345}' => {}
            c => letters.push((c.to_lowercase().next().unwrap_or(c), false)),
        }
    }
    let mut out = String::with_capacity(word.len());
    let mut i = 0;
    while i < letters.len() {
        let (c, stressed) = letters[i];
        let next = letters.get(i + 1).copied();
        let (text, used): (String, usize) = match (c, next) {
            ('ο', Some(('υ', s2))) => (vowel("u", stressed || s2), 2),
            ('α', Some(('υ', s2))) => (vowel("au", stressed || s2), 2),
            ('ε' | 'η', Some(('υ', s2))) => (vowel("eu", stressed || s2), 2),
            ('α', Some(('ι', s2))) => (vowel("ai", stressed || s2), 2),
            ('ε', Some(('ι', s2))) => (vowel("ei", stressed || s2), 2),
            ('ο', Some(('ι', s2))) => (vowel("oi", stressed || s2), 2),
            ('υ', Some(('ι', s2))) => (vowel("ui", stressed || s2), 2),
            ('γ', Some(('γ', _))) => ("ng".to_owned(), 2),
            ('γ', Some(('κ', _))) => ("nk".to_owned(), 2),
            ('γ', Some(('ξ', _))) => ("nx".to_owned(), 2),
            ('γ', Some(('χ', _))) => ("nj".to_owned(), 2),
            (c, _) => (
                match c {
                    'α' => vowel("a", stressed),
                    'ε' | 'η' => vowel("e", stressed),
                    'ι' | 'υ' => vowel("i", stressed),
                    'ο' | 'ω' => vowel("o", stressed),
                    'β' => "b".into(),
                    'γ' => "g".into(),
                    'δ' => "d".into(),
                    'ζ' => "dz".into(),
                    'θ' => "z".into(),
                    'κ' => "k".into(),
                    'λ' => "l".into(),
                    'μ' => "m".into(),
                    'ν' => "n".into(),
                    'ξ' => "x".into(),
                    'π' => "p".into(),
                    'ρ' => "r".into(),
                    'σ' | 'ς' => "s".into(),
                    'τ' => "t".into(),
                    'φ' => "f".into(),
                    'χ' => "j".into(),
                    'ψ' => "ps".into(),
                    other if other.is_alphabetic() => other.to_string(),
                    _ => String::new(),
                },
                1,
            ),
        };
        out.push_str(&text);
        i += used;
    }
    out
}

/// A vowel group, its first vowel accented when stressed.
fn vowel(group: &str, stressed: bool) -> String {
    if stressed {
        accent_first_vowel(group)
    } else {
        group.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hebrew_words_in_spanish() {
        assert_eq!(hebrew(Lang::Es, "be.re.Shit", ""), "bereshít");
        assert_eq!(hebrew(Lang::Es, "'E.lo.Him", ""), "elohím");
        assert_eq!(hebrew(Lang::Es, "ha.sha.Ma.yim", ""), "hashamáyim");
        assert_eq!(hebrew(Lang::Es, "che.sed", "kheh'-sed"), "jésed");
        assert_eq!(hebrew(Lang::Es, "ba.ra", "baw-raw'"), "bará");
        assert_eq!(
            hebrew(Lang::Es, "miz.mor", ""),
            "mizmór",
            "last syllable by default"
        );
        assert_eq!(hebrew(Lang::Es, "'et", ""), "et");
        assert_eq!(hebrew(Lang::Es, "tzi.Vah", ""), "tsivá");
    }

    #[test]
    fn hebrew_words_in_english_keep_strong() {
        assert_eq!(hebrew(Lang::En, "che.sed", "kheh'-sed"), "kheh'-sed");
        assert_eq!(hebrew(Lang::En, "be.re.Shit", ""), "be-re-Shit");
    }

    #[test]
    fn greek_words_in_spanish() {
        assert_eq!(greek(Lang::Es, "μακάριος", ""), "makários");
        assert_eq!(greek(Lang::Es, "συμμαρτυρέω", ""), "simmartiréo");
        assert_eq!(greek(Lang::Es, "πνεῦμα", ""), "pnéuma");
        assert_eq!(greek(Lang::Es, "Ἰησοῦς", ""), "iesús");
        assert_eq!(greek(Lang::Es, "ἄγγελος", ""), "ángelos");
        assert_eq!(greek(Lang::Es, "χάρις", ""), "járis");
        assert_eq!(greek(Lang::Es, "θεός", ""), "zeós");
        assert_eq!(greek(Lang::Es, "οὐρανός", ""), "uranós");
        assert_eq!(greek(Lang::Es, "ψυχή", ""), "psijé");
        assert_eq!(
            greek(Lang::En, "μακάριος", "mak-ar'-ee-os"),
            "mak-ar'-ee-os"
        );
    }
}
